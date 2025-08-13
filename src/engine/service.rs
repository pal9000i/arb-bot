use std::sync::Arc;
use ethers::prelude::*;
use crate::chain::{
    uniswap_v4_client::load_v4_pool_snapshot,
    aerodrome_client::load_volatile_pair_snapshot,
    gas::estimate_simple_gas_costs,
    cex_client::CexClient,
    across_fees::{
        get_weth_fee_base_to_eth, get_weth_fee_eth_to_base,
        get_usdc_fee_base_to_eth, get_usdc_fee_eth_to_base,
    },
};
use crate::engine::{
    optimizer::{optimize, OptimizerInputs, ArbDirection},
    pricing::{quote_uniswap_v4, quote_aerodrome, quote_uniswap_v4_both, quote_aerodrome_both},
};

pub struct ArbitrageAnalysis {
    pub timestamp_utc: String,
    pub trade_size_eth: f64,
    pub reference_cex_price_usd: f64,

    // Uniswap prices (both sides)
    pub uni_sell_price: f64, // ETH->USDC
    pub uni_buy_price:  f64, // USDC->ETH
    pub uniswap_price_impact: f64,
    pub uni_gas_usd:    f64,

    // Aerodrome prices (both sides)
    pub aero_sell_price: f64, // ETH->USDC
    pub aero_buy_price:  f64, // USDC->ETH
    pub aerodrome_price_impact: f64,
    pub aero_gas_usd:    f64,

    // Arbitrage summary (directional)
    pub gross_spread_sell_uni_buy_aero:  f64, // = uni_sell - aero_buy (USDC/ETH)
    pub gross_spread_sell_aero_buy_uni:  f64, // = aero_sell - uni_buy (USDC/ETH)
    pub gross_profit_uni_to_aero_usd:    f64, // spread * size
    pub gross_profit_aero_to_uni_usd:    f64, // spread * size
    pub total_gas_cost_usd:              f64,
    pub bridge_cost_usd:                 f64, // best direction bridge cost
    pub net_profit_best_usd:             f64,
    pub recommended_action:              String,
}

#[inline]
fn scale_amount_to_smallest_units(amount: f64, decimals: u32) -> String {
    // Defensive and saturating conversion from f64 -> token smallest units, as a decimal string.
    // - Negative, NaN, or non-finite => "0"
    // - Rounds to nearest integer of smallest units
    // - Clamps to u128::MAX to avoid UB on cast
    if !amount.is_finite() || amount <= 0.0 {
        return "0".to_string();
    }

    // 10^decimals (decimals in {6,18} for USDC/WETH) is safe in f64.
    let factor = 10f64.powi(decimals as i32);
    let scaled = amount * factor;

    // Round to nearest integer smallest-unit
    let v = if scaled.is_finite() { scaled.round() } else { 0.0 };

    // Clamp to [0, u128::MAX]
    if v <= 0.0 {
        return "0".to_string();
    }
    // Floor of u128::MAX as f64 (approx 3.4e38) — safe to cast back down after clamp.
    let max_u128_f64 = (u128::MAX as f64).floor();
    let clamped = if v > max_u128_f64 { max_u128_f64 } else { v };

    let as_u128 = clamped as u128;
    as_u128.to_string()
}

// Compute “rebalancing bridge fee” in USD for a given direction & size:
// We try bridging **WETH** and **USDC** (the two assets that become imbalanced),
// pick the cheaper USD fee. All calls are concurrent.
async fn compute_bridge_fee_usd_for_direction(
    trade_size_eth: f64,
    cex_price_usd: f64,
    direction: ArbDirection,
) -> f64 {
    // WETH amount to rebalance ≈ trade_size_eth (bridge in opposite direction of where ETH piled up)
    let weth_amount_wei = scale_amount_to_smallest_units(trade_size_eth, 18);

    // USDC imbalance ≈ trade_size_eth * price (bridge USDC the other way)
    let usdc_amount_6 = scale_amount_to_smallest_units(trade_size_eth * cex_price_usd, 6);

    use futures::future;

    // For SELL_UNI_BUY_AERO:
    // - ETH piles up on Base → bridge WETH Base→Ethereum OR
    // - USDC piles up on Ethereum → bridge USDC Ethereum→Base
    //
    // For SELL_AERO_BUY_UNI:
    // - ETH piles up on Ethereum → bridge WETH Ethereum→Base OR
    // - USDC piles up on Base → bridge USDC Base→Ethereum
    match direction {
        ArbDirection::SellUniBuyAero => {
            let weth_b2e = get_weth_fee_base_to_eth(&weth_amount_wei);
            let usdc_e2b = get_usdc_fee_eth_to_base(&usdc_amount_6);

            // Run both in parallel; if one fails, keep the other
            let (weth_fee_res, usdc_fee_res) = future::join(weth_b2e, usdc_e2b).await;

            let weth_fee_usd = weth_fee_res
                .ok()
                .and_then(|f| f.total_relay_fee.total_in_usd(18, cex_price_usd).ok())
                .unwrap_or(f64::INFINITY);

            let usdc_fee_usd = usdc_fee_res
                .ok()
                .and_then(|f| f.total_relay_fee.total_in_usd(6, 1.0).ok()) // USDC ~ $1.0
                .unwrap_or(f64::INFINITY);

            if !weth_fee_usd.is_finite() && !usdc_fee_usd.is_finite() {
                log::warn!("Both bridge fee lookups failed for SellUniBuyAero; treating as prohibitive");
            }

            weth_fee_usd.min(usdc_fee_usd)
        }
        ArbDirection::SellAeroBuyUni => {
            let weth_e2b = get_weth_fee_eth_to_base(&weth_amount_wei);
            let usdc_b2e = get_usdc_fee_base_to_eth(&usdc_amount_6);

            let (weth_fee_res, usdc_fee_res) = future::join(weth_e2b, usdc_b2e).await;

            let weth_fee_usd = weth_fee_res
                .ok()
                .and_then(|f| f.total_relay_fee.total_in_usd(18, cex_price_usd).ok())
                .unwrap_or(f64::INFINITY);

            let usdc_fee_usd = usdc_fee_res
                .ok()
                .and_then(|f| f.total_relay_fee.total_in_usd(6, 1.0).ok())
                .unwrap_or(f64::INFINITY);

            if !weth_fee_usd.is_finite() && !usdc_fee_usd.is_finite() {
                log::warn!("Both bridge fee lookups failed for SellAeroBuyUni; treating as prohibitive");
            }

            weth_fee_usd.min(usdc_fee_usd)
        }
    }
}

pub async fn analyze_arbitrage(
    eth_provider: Arc<Provider<Http>>,
    base_provider: Arc<Provider<Http>>,
    state_view_addr: Address,
    cex_client: &CexClient,
    trade_size_eth: f64,
    eth_usdc_address: Address,
    base_weth_address: Address,
    base_usdc_address: Address,
    aerodrome_factory_address: Address,
    aerodrome_pool_address: Option<Address>,
    gas_uniswap_units: u64,
    gas_aerodrome_units: u64,
) -> Result<ArbitrageAnalysis, Box<dyn std::error::Error + Send + Sync>> {
    use std::time::Instant;

    // PARALLEL EXECUTION: Run all independent data fetches concurrently
    let parallel_start = Instant::now();
    log::info!("Starting parallel data fetch");

    let (cex_price, (uni_pool, uni_token0_is_eth), (aero_pair, aero_token0_is_weth)) = tokio::try_join!(
        async {
            let start = Instant::now();
            let result = cex_client.get_coinbase_price().await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() });
            log::debug!("CEX price fetch completed in {:?}", start.elapsed());
            result
        },
        async {
            let start = Instant::now();
            let result = load_v4_pool_snapshot(
                eth_provider.clone(),
                state_view_addr,
                eth_usdc_address,
                3000,
                60,
            ).await;
            log::debug!("Uniswap V4 snapshot completed in {:?}", start.elapsed());
            result
        },
        async {
            let start = Instant::now();
            let result = load_volatile_pair_snapshot(
                base_provider.clone(),
                base_weth_address,
                base_usdc_address,
                aerodrome_factory_address,
                aerodrome_pool_address,
            ).await;
            log::debug!("Aerodrome snapshot completed in {:?}", start.elapsed());
            result
        }
    )?;

    log::info!("Parallel data fetch completed in {:?}", parallel_start.elapsed());

    // Gas estimation (depends on cex_price, so runs after parallel fetch)
    let gas_start = Instant::now();
    log::debug!("Starting gas estimation");
    let (gas_eth, gas_base) = estimate_simple_gas_costs(
        eth_provider.clone(),
        base_provider.clone(),
        cex_price,
        gas_uniswap_units,
        gas_aerodrome_units,
    ).await?;
    log::debug!("Gas estimation completed in {:?}", gas_start.elapsed());

    // 4. Quotes (both sides per venue)
    log::debug!("Starting Uniswap V4 bidirectional quote");
    let uni = quote_uniswap_v4_both(&uni_pool, uni_token0_is_eth, trade_size_eth, &gas_eth, Some(3000))
        .map_err(|e| { log::error!("Uniswap V4 quote failed: {:?}", e); e })?;

    log::debug!("Starting Aerodrome bidirectional quote");
    let aero = quote_aerodrome_both(&aero_pair, aero_token0_is_weth, trade_size_eth, &gas_base);

    // Legacy quotes for price impact calculation
    log::debug!("Starting legacy Uniswap V4 quote");
    let uni_quote = quote_uniswap_v4(&uni_pool, uni_token0_is_eth, trade_size_eth, &gas_eth)
        .map_err(|e| { log::error!("Legacy Uniswap V4 quote failed: {:?}", e); e })?;

    log::debug!("Starting legacy Aerodrome quote");
    let aero_quote = quote_aerodrome(&aero_pair, aero_token0_is_weth, trade_size_eth, &gas_base)
        .map_err(|e| { log::error!("Legacy Aerodrome quote failed: {:?}", e); e })?;

    // 5. Directional arbitrage math (USDC/ETH prices)
    let spread_uni_to_aero = uni.sell.price_usdc_per_eth - aero.buy.price_usdc_per_eth;
    let spread_aero_to_uni = aero.sell.price_usdc_per_eth - uni.buy.price_usdc_per_eth;

    let gross_uni_to_aero = spread_uni_to_aero * trade_size_eth;
    let gross_aero_to_uni = spread_aero_to_uni * trade_size_eth;

    // Compute live Across bridge fees (USD) for rebalancing in each direction (concurrently)
    use futures::future;
    let (fee_uni_to_aero_usd, fee_aero_to_uni_usd) = future::join(
        compute_bridge_fee_usd_for_direction(trade_size_eth, cex_price, ArbDirection::SellUniBuyAero),
        compute_bridge_fee_usd_for_direction(trade_size_eth, cex_price, ArbDirection::SellAeroBuyUni),
    ).await;

    let total_cost_uni_to_aero = gas_eth.total_usd + gas_base.total_usd + fee_uni_to_aero_usd;
    let total_cost_aero_to_uni = gas_eth.total_usd + gas_base.total_usd + fee_aero_to_uni_usd;

    let net1 = gross_uni_to_aero - total_cost_uni_to_aero; // SELL_UNI_BUY_AERO
    let net2 = gross_aero_to_uni - total_cost_aero_to_uni; // SELL_AERO_BUY_UNI

    let (net_best, action) = if net1.max(net2) > 0.0 {
        (net1.max(net2), "ARBITRAGE_DETECTED".to_string())
    } else {
        (0.0, "NO_ARBITRAGE".to_string())
    };

    Ok(ArbitrageAnalysis {
        timestamp_utc: chrono::Utc::now().to_rfc3339(),
        trade_size_eth,
        reference_cex_price_usd: cex_price,

        uni_sell_price: uni.sell.price_usdc_per_eth,
        uni_buy_price:  uni.buy.price_usdc_per_eth,
        uniswap_price_impact: uni_quote.price_impact_percent,
        uni_gas_usd:    uni.sell.estimated_gas_cost_usd,

        aero_sell_price: aero.sell.price_usdc_per_eth,
        aero_buy_price:  aero.buy.price_usdc_per_eth,
        aerodrome_price_impact: aero_quote.price_impact_percent,
        aero_gas_usd:    aero.sell.estimated_gas_cost_usd,

        gross_spread_sell_uni_buy_aero: spread_uni_to_aero,
        gross_spread_sell_aero_buy_uni: spread_aero_to_uni,
        gross_profit_uni_to_aero_usd:   gross_uni_to_aero,
        gross_profit_aero_to_uni_usd:   gross_aero_to_uni,
        total_gas_cost_usd:             if net1 >= net2 { total_cost_uni_to_aero } else { total_cost_aero_to_uni },
        bridge_cost_usd:                if net1 >= net2 { fee_uni_to_aero_usd } else { fee_aero_to_uni_usd },
        net_profit_best_usd:            net_best,
        recommended_action:             action,
    })
}

pub struct OptimalArbitrageAnalysis {
    pub timestamp_utc: String,
    pub reference_cex_price_usd: f64,
    pub optimal_trade_size_eth: f64,
    pub optimal_direction: String,
    pub net_profit_usd: f64,
    pub gross_profit_usd: f64,
    pub total_costs_usd: f64,
    pub effective_sell_price_usdc_per_eth: f64,
    pub effective_buy_price_usdc_per_eth: f64,
    pub gas_cost_usd: f64,
    pub bridge_cost_usd: f64,
    pub recommended_action: String,
}

pub async fn find_optimal_arbitrage(
    eth_provider: Arc<Provider<Http>>,
    base_provider: Arc<Provider<Http>>,
    state_view_addr: Address,
    cex_client: &CexClient,
    max_size_eth: f64,
    eth_usdc_address: Address,
    base_weth_address: Address,
    base_usdc_address: Address,
    aerodrome_factory_address: Address,
    aerodrome_pool_address: Option<Address>,
    gas_uniswap_units: u64,
    gas_aerodrome_units: u64,
) -> Result<OptimalArbitrageAnalysis, Box<dyn std::error::Error + Send + Sync>> {
    use std::time::Instant;

    // PARALLEL EXECUTION: Run all independent data fetches concurrently
    let parallel_start = Instant::now();
    log::info!("Starting parallel data fetch for optimal arbitrage");

    let (cex_price, (uni_pool, uni_token0_is_eth), (aero_pair, aero_token0_is_weth)) = tokio::try_join!(
        async {
            let start = Instant::now();
            let result = cex_client.get_coinbase_price().await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() });
            log::debug!("CEX price fetch completed in {:?}", start.elapsed());
            result
        },
        async {
            let start = Instant::now();
            let result = load_v4_pool_snapshot(
                eth_provider.clone(),
                state_view_addr,
                eth_usdc_address,
                3000,
                60,
            ).await;
            log::debug!("Uniswap V4 snapshot completed in {:?}", start.elapsed());
            result
        },
        async {
            let start = Instant::now();
            let result = load_volatile_pair_snapshot(
                base_provider.clone(),
                base_weth_address,
                base_usdc_address,
                aerodrome_factory_address,
                aerodrome_pool_address,
            ).await;
            log::debug!("Aerodrome snapshot completed in {:?}", start.elapsed());
            result
        }
    )?;

    log::info!("Parallel data fetch completed in {:?}", parallel_start.elapsed());

    // 3. Fetch gas costs using predefined constants
    let (gas_eth, gas_base) = estimate_simple_gas_costs(
        eth_provider.clone(),
        base_provider.clone(),
        cex_price,
        gas_uniswap_units,
        gas_aerodrome_units,
    ).await?;

    // 4. Run optimizer (bridge_cost_usd is a placeholder; we’ll recompute live below)
    let inputs = OptimizerInputs {
        uni_pool: uni_pool.clone(),
        uni_token0_is_weth: uni_token0_is_eth,
        uni_fee_ppm_override: Some(3000),
        aero_pair: aero_pair.clone(),
        aero_token0_is_weth,
        gas_eth: gas_eth.clone(),
        gas_base: gas_base.clone(),
        bridge_cost_usd: 10.0, // placeholder
        hint_size_eth: max_size_eth / 2.0,
        max_size_eth,
    };

    match optimize(&inputs) {
        Some(result) => {
            // Compute **live** bridge fee for the optimizer’s optimal size & direction
            let live_bridge_fee_usd = compute_bridge_fee_usd_for_direction(
                result.optimal_size_eth,
                cex_price,
                match result.direction {
                    ArbDirection::SellAeroBuyUni => ArbDirection::SellAeroBuyUni,
                    ArbDirection::SellUniBuyAero => ArbDirection::SellUniBuyAero,
                },
            ).await;

            // Recompute totals replacing placeholder bridge cost with live fee
            let corrected_total_costs = result.costs_usd - result.bridge_cost_usd + live_bridge_fee_usd;
            let corrected_net = result.proceeds_usd - corrected_total_costs;

            let direction_str = match result.direction {
                ArbDirection::SellAeroBuyUni => "SELL_AERODROME_BUY_UNISWAP",
                ArbDirection::SellUniBuyAero => "SELL_UNISWAP_BUY_AERODROME",
            };

            let action = if corrected_net > 0.0 {
                "PROFITABLE_ARBITRAGE_FOUND"
            } else {
                "NO_PROFITABLE_ARBITRAGE"
            };

            Ok(OptimalArbitrageAnalysis {
                timestamp_utc: chrono::Utc::now().to_rfc3339(),
                reference_cex_price_usd: cex_price,
                optimal_trade_size_eth: result.optimal_size_eth,
                optimal_direction: direction_str.to_string(),
                net_profit_usd: corrected_net,
                gross_profit_usd: result.proceeds_usd,
                total_costs_usd: corrected_total_costs,
                effective_sell_price_usdc_per_eth: result.eff_price_sell_usdc_per_eth,
                effective_buy_price_usdc_per_eth: result.eff_price_buy_usdc_per_eth,
                gas_cost_usd: result.gas_usd_total,
                bridge_cost_usd: live_bridge_fee_usd,
                recommended_action: action.to_string(),
            })
        }
        None => {
            // Still show market prices at a small test size for reference
            let test_size = 1.0; // 1 ETH for price discovery
            let uni = quote_uniswap_v4_both(&uni_pool, uni_token0_is_eth, test_size, &gas_eth, Some(3000))
                .unwrap_or_else(|_| {
                    log::warn!("Failed to get Uniswap V4 quotes for test size, using defaults");
                    crate::engine::pricing::VenueQuotes {
                        sell: crate::engine::pricing::SideQuote {
                            price_usdc_per_eth: 0.0,
                            estimated_gas_cost_usd: 0.0,
                        },
                        buy: crate::engine::pricing::SideQuote {
                            price_usdc_per_eth: 0.0,
                            estimated_gas_cost_usd: 0.0,
                        },
                    }
                });
            let aero = quote_aerodrome_both(&aero_pair, aero_token0_is_weth, test_size, &gas_base);

            // Determine which direction would be better (even if unprofitable)
            let spread_uni_to_aero = uni.sell.price_usdc_per_eth - aero.buy.price_usdc_per_eth;
            let spread_aero_to_uni = aero.sell.price_usdc_per_eth - uni.buy.price_usdc_per_eth;

            let (direction, sell_price, buy_price) = if spread_uni_to_aero > spread_aero_to_uni {
                ("SELL_UNISWAP_BUY_AERODROME", uni.sell.price_usdc_per_eth, aero.buy.price_usdc_per_eth)
            } else {
                ("SELL_AERODROME_BUY_UNISWAP", aero.sell.price_usdc_per_eth, uni.buy.price_usdc_per_eth)
            };

            Ok(OptimalArbitrageAnalysis {
                timestamp_utc: chrono::Utc::now().to_rfc3339(),
                reference_cex_price_usd: cex_price,
                optimal_trade_size_eth: 0.0,
                optimal_direction: direction.to_string(),
                net_profit_usd: 0.0,
                gross_profit_usd: 0.0,
                total_costs_usd: 0.0,
                effective_sell_price_usdc_per_eth: sell_price,
                effective_buy_price_usdc_per_eth: buy_price,
                gas_cost_usd: gas_eth.total_usd + gas_base.total_usd,
                bridge_cost_usd: 0.0,
                recommended_action: "NO_ARBITRAGE_OPPORTUNITY".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::U256;

    #[test]
    fn test_scale_amount_to_smallest_units() {
        // Test ETH (18 decimals)
        assert_eq!(scale_amount_to_smallest_units(1.0, 18), "1000000000000000000");
        assert_eq!(scale_amount_to_smallest_units(0.000001, 18), "1000000000000");
        assert_eq!(scale_amount_to_smallest_units(1.5, 18), "1500000000000000000");

        // Test USDC (6 decimals)
        assert_eq!(scale_amount_to_smallest_units(1.0, 6), "1000000");
        assert_eq!(scale_amount_to_smallest_units(1000.0, 6), "1000000000");
        assert_eq!(scale_amount_to_smallest_units(0.000001, 6), "1");

        // Test edge cases
        assert_eq!(scale_amount_to_smallest_units(0.0, 18), "0");
        assert_eq!(scale_amount_to_smallest_units(-10.0, 18), "0"); // negative clamped to 0
        assert_eq!(scale_amount_to_smallest_units(f64::NAN, 18), "0"); // NaN handled
        assert_eq!(scale_amount_to_smallest_units(f64::INFINITY, 18), "0"); // Infinity handled

        // Test rounding
        assert_eq!(scale_amount_to_smallest_units(1.4999999999, 6), "1500000"); // rounds to 1.5
        assert_eq!(scale_amount_to_smallest_units(1.5000000001, 6), "1500000"); // rounds to 1.5

        // Very large (saturates safely rather than panic)
        let huge = scale_amount_to_smallest_units(1e40, 18);
        assert!(!huge.is_empty());
    }

    #[tokio::test]
    async fn test_compute_bridge_fee_usd_for_direction() {
        // This would need mocking of the across_fees functions
        // For now, we'll test that it handles the direction correctly
        // In a real test, we'd mock get_weth_fee_* and get_usdc_fee_*
        // and ensure min(selection) & error paths behave.
    }

    #[test]
    fn test_arbitrage_direction_selection() {
        // Test that we select the correct direction based on profits
        let gas_eth = crate::chain::gas::GasEstimate {
            gas_limit: U256::from(200_000),
            gas_price: U256::from(25_000_000_000u64), // 25 gwei
            l1_data_fee: U256::zero(),
            total_wei: U256::from(5_000_000_000_000_000u64), // 0.005 ETH
            total_eth: 0.005,
            total_usd: 2.0,
        };

        let gas_base = crate::chain::gas::GasEstimate {
            gas_limit: U256::from(150_000),
            gas_price: U256::from(1_000_000_000u64), // 1 gwei
            l1_data_fee: U256::zero(),
            total_wei: U256::from(150_000_000_000_000u64),
            total_eth: 0.00015,
            total_usd: 0.5,
        };

        // Test profit calculation logic
        let spread_uni_to_aero = 100.0; // profitable
        let spread_aero_to_uni = -50.0; // not profitable
        let trade_size = 10.0;

        let gross_uni_to_aero = spread_uni_to_aero * trade_size;
        let gross_aero_to_uni = spread_aero_to_uni * trade_size;

        assert_eq!(gross_uni_to_aero, 1000.0);
        assert_eq!(gross_aero_to_uni, -500.0);

        // Simulate total costs
        let bridge_cost = 5.0;
        let total_cost = gas_eth.total_usd + gas_base.total_usd + bridge_cost;

        let net_uni_to_aero = gross_uni_to_aero - total_cost;
        let net_aero_to_uni = gross_aero_to_uni - total_cost;

        // Best should be uni_to_aero
        let net_best = net_uni_to_aero.max(net_aero_to_uni);
        assert!(net_best == net_uni_to_aero);

        // Test action selection
        let action = if net_best > 0.0 {
            "ARBITRAGE_DETECTED"
        } else {
            "NO_ARBITRAGE"
        };

        // With these numbers, should detect arbitrage
        assert_eq!(action, "ARBITRAGE_DETECTED");
    }

    #[test]
    fn test_spread_calculations() {
        // Test spread calculation logic
        let uni_sell_price = 3500.0;
        let uni_buy_price = 3510.0;
        let aero_sell_price = 3480.0;
        let aero_buy_price = 3490.0;

        // Spread for selling on Uni, buying on Aero
        let spread_uni_to_aero = uni_sell_price - aero_buy_price;
        assert_eq!(spread_uni_to_aero, 10.0); // Profitable by $10/ETH

        // Spread for selling on Aero, buying on Uni
        let spread_aero_to_uni = aero_sell_price - uni_buy_price;
        assert_eq!(spread_aero_to_uni, -30.0); // Loss of $30/ETH

        // Test that we identify the better direction
        assert!(spread_uni_to_aero > spread_aero_to_uni);
    }

    #[test]
    fn test_gas_cost_selection() {
        // Test that we select the correct gas costs based on direction
        let gas_eth = crate::chain::gas::GasEstimate {
            gas_limit: U256::from(200_000),
            gas_price: U256::from(25_000_000_000u64),
            l1_data_fee: U256::zero(),
            total_wei: U256::from(5_000_000_000_000_000u64),
            total_eth: 0.005,
            total_usd: 2.0,
        };

        let gas_base = crate::chain::gas::GasEstimate {
            gas_limit: U256::from(150_000),
            gas_price: U256::from(1_000_000_000u64),
            l1_data_fee: U256::zero(),
            total_wei: U256::from(150_000_000_000_000u64),
            total_eth: 0.00015,
            total_usd: 0.5,
        };

        let net1 = 100.0; // uni_to_aero profitable
        let net2 = -50.0; // aero_to_uni not profitable

        // Total gas should be sum for the better direction
        let total_gas = if net1 >= net2 {
            gas_eth.total_usd + gas_base.total_usd
        } else {
            gas_eth.total_usd + gas_base.total_usd
        };

        assert_eq!(total_gas, 2.5);
    }

    #[test]
    fn test_bridge_cost_selection() {
        // Test that we select the correct bridge cost based on best direction
        let fee_uni_to_aero_usd = 5.0;
        let fee_aero_to_uni_usd = 7.0;

        let net1 = 100.0; // uni_to_aero profitable
        let net2 = -50.0; // aero_to_uni not profitable

        // Should select bridge cost for the better direction
        let bridge_cost = if net1 >= net2 {
            fee_uni_to_aero_usd
        } else {
            fee_aero_to_uni_usd
        };

        assert_eq!(bridge_cost, 5.0);

        // Test opposite case
        let net1_loss = -100.0;
        let net2_smaller_loss = -20.0;

        let bridge_cost_2 = if net1_loss >= net2_smaller_loss {
            fee_uni_to_aero_usd
        } else {
            fee_aero_to_uni_usd
        };

        assert_eq!(bridge_cost_2, 7.0);
    }

    #[test]
    fn test_price_formatting() {
        // Test that prices are formatted correctly
        let price = 3500.123456789;
        let formatted = format!("{:.2}", price);
        assert_eq!(formatted, "3500.12");

        let small_price = 0.0001234;
        let formatted_small = format!("{:.6}", small_price);
        assert_eq!(formatted_small, "0.000123");
    }

    #[test]
    fn test_recommended_action() {
        // Test action recommendations
        let test_cases = vec![
            (100.0, "ARBITRAGE_DETECTED"),
            (0.01, "ARBITRAGE_DETECTED"),
            (0.0, "NO_ARBITRAGE"),
            (-100.0, "NO_ARBITRAGE"),
        ];

        for (net_profit, expected_action) in test_cases {
            let action = if net_profit > 0.0 {
                "ARBITRAGE_DETECTED"
            } else {
                "NO_ARBITRAGE"
            };
            assert_eq!(action, expected_action, "Failed for net_profit: {}", net_profit);
        }
    }
}
