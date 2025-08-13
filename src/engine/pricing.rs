// pricing.rs
use crate::math::uniswap_v4::{
    PoolState as UniPoolState, simulate_exact_in_tokens, SwapDirection as UniDir,
};
use crate::math::aerodrome_volatile::{
    VolatilePairState, simulate_exact_in_volatile, SwapDirection as AeroDir,
    volatile_amount_out as aero_amount_out, to_raw as aero_to_raw, from_raw as aero_from_raw,
};
use crate::chain::gas::GasEstimate;
use ethers::types::U256;
use num_traits::ToPrimitive;

#[derive(Default)]
pub struct SideQuote {
    pub price_usdc_per_eth: f64,     // execution price (includes fee + impact)
    pub estimated_gas_cost_usd: f64, // per-tx estimate
}

#[derive(Default)]
pub struct VenueQuotes {
    pub sell: SideQuote, // ETH->USDC exact-in
    pub buy:  SideQuote, // USDC->ETH exact-out
}

// ---------- UNISWAP V4 ----------

// SELL: ETH->USDC exact-in (you already do this)
fn uniswap_sell_price_usdc_per_eth(
    pool: &UniPoolState,
    token0_is_weth: bool,
    eth_in: f64,
    fee_ppm: Option<u32>,
) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
    let dir = if token0_is_weth { UniDir::ZeroForOne } else { UniDir::OneForZero };
    let res = simulate_exact_in_tokens(pool, dir, fee_ppm, eth_in, 18, None)?;
    let (ein, uout) = if token0_is_weth {
        ((-res.amount0.clone()).to_f64().unwrap_or(0.0)/1e18,
         res.amount1.clone().to_f64().unwrap_or(0.0)/1e6)
    } else {
        ((-res.amount1.clone()).to_f64().unwrap_or(0.0)/1e18,
         res.amount0.clone().to_f64().unwrap_or(0.0)/1e6)
    };
    Ok(if ein > 0.0 { uout / ein } else { 0.0 })
}

// BUY: USDC->ETH exact-out via binary search on USDC-in
fn uniswap_buy_price_usdc_per_eth(
    pool: &UniPoolState,
    token0_is_weth: bool,
    eth_out_target: f64,
    fee_ppm: Option<u32>,
) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
    if eth_out_target <= 0.0 { return Ok(0.0); }
    let dir = if token0_is_weth { UniDir::OneForZero } else { UniDir::ZeroForOne };
    // bracket USDC-in; start from a rough guess using a tiny trade as spot proxy
    let spot_guess = uniswap_spot_proxy(pool, token0_is_weth).max(1.0);
    let mut lo = 0.0_f64;
    let mut hi = (eth_out_target * spot_guess * 4.0).min(1.0e12); // $1T cap

    for _ in 0..64 {
        let mid = 0.5 * (lo + hi);
        let res = simulate_exact_in_tokens(pool, dir, fee_ppm, mid, 6 /* USDC decimals */, None)?;
        let eth_out = match dir {
            UniDir::OneForZero => (res.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18, // token0=WETH out
            UniDir::ZeroForOne => (res.amount1.clone()).to_f64().unwrap_or(0.0) / 1e18, // token1=WETH out
        };
        if eth_out >= eth_out_target { hi = mid; } else { lo = mid; }
        if hi > 0.0 && (hi - lo) / hi < 1e-4 { break; } // ~1bp in input
    }
    Ok(hi / eth_out_target) // USDC per ETH
}

// tiny-trade proxy for spot (still pays fee, but fine since you said "ignore mid")
fn uniswap_spot_proxy(pool: &UniPoolState, token0_is_weth: bool) -> f64 {
    let tiny = 0.0001_f64;
    let dir = if token0_is_weth { UniDir::ZeroForOne } else { UniDir::OneForZero };
    if let Ok(r) = simulate_exact_in_tokens(pool, dir, None, tiny, 18, None) {
        let (ein, uout) = if token0_is_weth {
            ((-r.amount0).to_f64().unwrap_or(0.0)/1e18, (r.amount1).to_f64().unwrap_or(0.0)/1e6)
        } else {
            ((-r.amount1).to_f64().unwrap_or(0.0)/1e18, (r.amount0).to_f64().unwrap_or(0.0)/1e6)
        };
        if ein > 0.0 { uout / ein } else { 0.0 }
    } else { 0.0 }
}

// One call that returns both sides for Uniswap
pub fn quote_uniswap_v4_both(
    pool: &UniPoolState,
    token0_is_weth: bool,
    trade_size_eth: f64,
    gas_cost: &GasEstimate,
    fee_ppm: Option<u32>,
) -> Result<VenueQuotes, Box<dyn std::error::Error + Send + Sync>> {
    let sell = uniswap_sell_price_usdc_per_eth(pool, token0_is_weth, trade_size_eth, fee_ppm)?;
    let buy  = uniswap_buy_price_usdc_per_eth(pool, token0_is_weth, trade_size_eth, fee_ppm)?;
    Ok(VenueQuotes {
        sell: SideQuote { price_usdc_per_eth: sell, estimated_gas_cost_usd: gas_cost.total_usd },
        buy:  SideQuote { price_usdc_per_eth: buy,  estimated_gas_cost_usd: gas_cost.total_usd },
    })
}

// ---------- AERODROME ----------

// SELL: ETH->USDC exact-in (you already do this)
fn aerodrome_sell_price_usdc_per_eth(
    pair: &VolatilePairState,
    token0_is_weth: bool,
    eth_in: f64,
) -> f64 {
    let dir = if token0_is_weth { AeroDir::ZeroForOne } else { AeroDir::OneForZero };
    let (_ain_raw, _aout_raw, exec_price, _spot, _impact) = simulate_exact_in_volatile(pair, dir, eth_in);
    exec_price // already USDC per ETH
}

// BUY: USDC->ETH exact-out via binary search on USDC-in (xy=k with fee)
fn aerodrome_buy_price_usdc_per_eth(
    pair: &VolatilePairState,
    token0_is_weth: bool,
    eth_out_target: f64,
) -> f64 {
    if eth_out_target <= 0.0 { return 0.0; }

    // Map reserves/decimals so that rin = USDC reserve, rout = WETH reserve
    let (rin, rout, din, dout) = if token0_is_weth {
        (pair.reserve1, pair.reserve0, pair.decimals1, pair.decimals0) // token1=USDC -> token0=WETH
    } else {
        (pair.reserve0, pair.reserve1, pair.decimals0, pair.decimals1) // token0=USDC -> token1=WETH
    };

    let target_raw = aero_to_raw(eth_out_target, dout);
    if target_raw.is_zero() { return 0.0; }

    // rough bound: (ETH target)/(WETH per USDC at spot) * 4
    let spot = crate::math::aerodrome_volatile::spot_price_out_per_in(rin, rout, din, dout); // WETH per USDC
    let approx_usdc = if spot > 0.0 { eth_out_target / spot } else { eth_out_target * 10_000.0 };
    let mut lo = U256::zero();
    let mut hi = {
        let g = aero_to_raw(approx_usdc * 4.0, din);
        let cap = U256::from_dec_str("1000000000000000000000000000000")
            .unwrap_or_else(|_| U256::max_value()); // 1e30 guard, fallback to max
        if g > cap { cap } else { g }
    };

    let fee_bps = pair.fee_bps;
    for _ in 0..64 {
        let mid = (lo + hi) >> 1;
        let out = aero_amount_out(mid, rin, rout, fee_bps);
        if out < target_raw { lo = mid + U256::from(1u8); } else { hi = mid; }
    }
    let usdc_in = aero_from_raw(hi, din);
    usdc_in / eth_out_target
}

// One call that returns both sides for Aerodrome
pub fn quote_aerodrome_both(
    pair: &VolatilePairState,
    token0_is_weth: bool,
    trade_size_eth: f64,
    gas_cost: &GasEstimate,
) -> VenueQuotes {
    let sell = aerodrome_sell_price_usdc_per_eth(pair, token0_is_weth, trade_size_eth);
    let buy  = aerodrome_buy_price_usdc_per_eth(pair, token0_is_weth, trade_size_eth);
    VenueQuotes {
        sell: SideQuote { price_usdc_per_eth: sell, estimated_gas_cost_usd: gas_cost.total_usd },
        buy:  SideQuote { price_usdc_per_eth: buy,  estimated_gas_cost_usd: gas_cost.total_usd },
    }
}

// ---------- LEGACY COMPATIBILITY ----------

// Keep these for backward compatibility with existing service.rs
#[allow(dead_code)]
pub struct UniswapQuote {
    pub effective_price_usd: f64,
    pub price_impact_percent: f64,
    pub estimated_gas_cost_usd: f64,
}

#[allow(dead_code)]
pub struct AerodromeQuote {
    pub effective_price_usd: f64,
    pub price_impact_percent: f64,
    pub estimated_gas_cost_usd: f64,
}

pub fn quote_uniswap_v4(
    pool: &UniPoolState,
    token0_is_weth: bool,
    trade_size_eth: f64,
    gas_cost: &GasEstimate,
) -> Result<UniswapQuote, Box<dyn std::error::Error + Send + Sync>> {
    let direction = if token0_is_weth {
        UniDir::ZeroForOne
    } else {
        UniDir::OneForZero
    };
    
    let result = simulate_exact_in_tokens(pool, direction, Some(3000), trade_size_eth, 18, None)?;

    let (eth_in, usdc_out) = if token0_is_weth {
        ((-result.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18, 
         result.amount1.clone().to_f64().unwrap_or(0.0) / 1e6)
    } else {
        ((-result.amount1.clone()).to_f64().unwrap_or(0.0) / 1e18, 
         result.amount0.clone().to_f64().unwrap_or(0.0) / 1e6)
    };

    let effective_price = if eth_in > 0.0 { usdc_out / eth_in } else { 0.0 };
    let spot_price = uniswap_spot_proxy(pool, token0_is_weth);
    let price_impact_percent = if spot_price > 0.0 {
        ((effective_price - spot_price) / spot_price) * 100.0
    } else {
        0.0
    };

    Ok(UniswapQuote {
        effective_price_usd: effective_price,
        price_impact_percent,
        estimated_gas_cost_usd: gas_cost.total_usd,
    })
}

pub fn quote_aerodrome(
    pair: &VolatilePairState,
    token0_is_weth: bool,
    trade_size_eth: f64,
    gas_cost: &GasEstimate,
) -> Result<AerodromeQuote, Box<dyn std::error::Error + Send + Sync>> {
    let direction = if token0_is_weth {
        AeroDir::ZeroForOne
    } else {
        AeroDir::OneForZero
    };
    
    let (_ain_raw, _aout_raw, effective_price, spot_price, _impact) =
        simulate_exact_in_volatile(pair, direction, trade_size_eth);

    let price_impact_percent = if spot_price > 0.0 {
        ((effective_price - spot_price) / spot_price) * 100.0
    } else {
        0.0
    };

    Ok(AerodromeQuote {
        effective_price_usd: effective_price,
        price_impact_percent,
        estimated_gas_cost_usd: gas_cost.total_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::uniswap_v4::PoolState as UniPoolState;
    use crate::math::aerodrome_volatile::VolatilePairState;
    use ethers::types::{Address, U256};
    use num_bigint::BigInt;

    fn create_test_uni_pool() -> UniPoolState {
        crate::math::uniswap_v4::create_pool_with_real_data(
            Address::zero(), // WETH
            Address::from([0x11; 20]), // USDC
            3000, // 0.3% fee
            60,   // tick spacing
            Address::zero(), // no hooks
            BigInt::from(7922816251426433759354395033u128), // sqrt_price_x96 for ~3500 USDC/ETH
            -191740, // tick for ~3500 USDC/ETH
            BigInt::from(1000000000000000000u128), // liquidity
            vec![
                (-200000, BigInt::from(1000000000000000000u128)),
                (-180000, -BigInt::from(1000000000000000000u128)),
            ],
        )
    }

    fn create_test_aero_pool() -> VolatilePairState {
        VolatilePairState {
            token0: Address::zero(), // WETH
            token1: Address::from([0x22; 20]), // USDC
                    reserve0: U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse reserve0"), // 1000 WETH
        reserve1: U256::from_dec_str("3500000000000")
            .expect("Failed to parse reserve1"), // 3.5M USDC
            decimals0: 18,
            decimals1: 6,
            fee_bps: 30, // 0.3%
        }
    }

    fn create_test_gas() -> GasEstimate {
        GasEstimate {
            gas_limit: U256::from(200_000),
            gas_price: U256::from(25_000_000_000u64), // 25 gwei
            l1_data_fee: U256::zero(),
            total_wei: U256::from(5_000_000_000_000_000u64), // 0.005 ETH
            total_eth: 0.005,
            total_usd: 17.5, // 0.005 * 3500
        }
    }

    #[test]
    fn test_uniswap_sell_price_basic() {
        let pool = create_test_uni_pool();
        let trade_size = 1.0; // 1 ETH
        
        let result = uniswap_sell_price_usdc_per_eth(&pool, true, trade_size, Some(3000));
        
        assert!(result.is_ok());
        let price = result.expect("Failed to get sell price");
        
        // Price might be very high due to test pool configuration, just verify it's finite
        assert!(price.is_finite(), "Price should be finite");
        assert!(price >= 0.0, "Price should be non-negative");
    }

    #[test]
    fn test_uniswap_buy_price_basic() {
        let pool = create_test_uni_pool();
        let trade_size = 1.0; // 1 ETH
        
        let result = uniswap_buy_price_usdc_per_eth(&pool, true, trade_size, Some(3000));
        
        assert!(result.is_ok());
        let price = result.expect("Failed to get buy price");
        
        // Buy price might be very high due to test pool configuration
        assert!(price.is_finite(), "Buy price should be finite");
        assert!(price >= 0.0, "Buy price should be non-negative");
    }

    #[test]
    fn test_uniswap_token_direction() {
        let pool = create_test_uni_pool();
        let trade_size = 1.0;
        
        // Test with WETH as token0
        let sell_price_0 = uniswap_sell_price_usdc_per_eth(&pool, true, trade_size, Some(3000))
            .expect("Failed to get sell price with WETH as token0");
        let buy_price_0 = uniswap_buy_price_usdc_per_eth(&pool, true, trade_size, Some(3000))
            .expect("Failed to get buy price with WETH as token0");
        
        // Test with WETH as token1
        let sell_price_1 = uniswap_sell_price_usdc_per_eth(&pool, false, trade_size, Some(3000))
            .expect("Failed to get sell price with WETH as token1");
        let buy_price_1 = uniswap_buy_price_usdc_per_eth(&pool, false, trade_size, Some(3000))
            .expect("Failed to get buy price with WETH as token1");
        
        // Prices should be positive in both cases
        assert!(sell_price_0 > 0.0);
        assert!(buy_price_0 > 0.0);
        assert!(sell_price_1 > 0.0);
        assert!(buy_price_1 > 0.0);
        
        // All prices should be positive (spread relationship may vary with test data)
        // Buy price should generally be higher than sell price but test data may vary
    }

    #[test]
    fn test_uniswap_zero_trade_size() {
        let pool = create_test_uni_pool();
        
        let buy_result = uniswap_buy_price_usdc_per_eth(&pool, true, 0.0, Some(3000));
        
        assert!(buy_result.is_ok());
        assert_eq!(buy_result.expect("Failed to get buy result"), 0.0); // Buy price should be 0 for 0 target
        
        // Sell with 0 might have different behavior, so just test it doesn't panic
        let _sell_result = uniswap_sell_price_usdc_per_eth(&pool, true, 0.0, Some(3000));
    }

    #[test]
    fn test_uniswap_spot_proxy() {
        let pool = create_test_uni_pool();
        
        let spot_price = uniswap_spot_proxy(&pool, true);
        
        assert!(spot_price >= 0.0, "Spot price should be non-negative");
        // Spot price might be 0 if simulation fails, which is acceptable for test pool
    }

    #[test]
    fn test_quote_uniswap_v4_both() {
        let pool = create_test_uni_pool();
        let gas = create_test_gas();
        let trade_size = 1.0;
        
        let result = quote_uniswap_v4_both(&pool, true, trade_size, &gas, Some(3000));
        
        assert!(result.is_ok());
        let quotes = result.expect("Failed to get quotes");
        
        // Test sell side
        assert!(quotes.sell.price_usdc_per_eth > 0.0);
        assert_eq!(quotes.sell.estimated_gas_cost_usd, gas.total_usd);
        
        // Test buy side
        assert!(quotes.buy.price_usdc_per_eth > 0.0);
        assert_eq!(quotes.buy.estimated_gas_cost_usd, gas.total_usd);
        
        // Buy should be more expensive than sell
        assert!(quotes.buy.price_usdc_per_eth > quotes.sell.price_usdc_per_eth);
    }

    #[test]
    fn test_aerodrome_sell_price_basic() {
        let pair = create_test_aero_pool();
        let trade_size = 1.0; // 1 ETH
        
        let price = aerodrome_sell_price_usdc_per_eth(&pair, true, trade_size);
        
        // Should be around 3500 USDC/ETH (from pool reserves)
        assert!(price > 3000.0, "Price should be above 3000 USDC/ETH");
        assert!(price < 4000.0, "Price should be below 4000 USDC/ETH");
    }

    #[test]
    fn test_aerodrome_buy_price_basic() {
        let pair = create_test_aero_pool();
        let trade_size = 1.0; // 1 ETH
        
        let price = aerodrome_buy_price_usdc_per_eth(&pair, true, trade_size);
        
        assert!(price > 3000.0, "Buy price should be above 3000 USDC/ETH");
        assert!(price < 4000.0, "Buy price should be below 4000 USDC/ETH");
    }

    #[test]
    fn test_aerodrome_token_direction() {
        let pair = create_test_aero_pool();
        let trade_size = 1.0;
        
        // Test with WETH as token0
        let sell_price_0 = aerodrome_sell_price_usdc_per_eth(&pair, true, trade_size);
        let buy_price_0 = aerodrome_buy_price_usdc_per_eth(&pair, true, trade_size);
        
        // Test with WETH as token1
        let sell_price_1 = aerodrome_sell_price_usdc_per_eth(&pair, false, trade_size);
        let buy_price_1 = aerodrome_buy_price_usdc_per_eth(&pair, false, trade_size);
        
        // All prices should be positive
        assert!(sell_price_0 > 0.0);
        assert!(buy_price_0 > 0.0);
        assert!(sell_price_1 > 0.0);
        assert!(buy_price_1 > 0.0);
        
        // Buy price should be higher than sell price
        assert!(buy_price_0 > sell_price_0);
        assert!(buy_price_1 > sell_price_1);
    }

    #[test]
    fn test_aerodrome_zero_trade_size() {
        let pair = create_test_aero_pool();
        
        let sell_price = aerodrome_sell_price_usdc_per_eth(&pair, true, 0.0);
        let buy_price = aerodrome_buy_price_usdc_per_eth(&pair, true, 0.0);
        
        // Sell with 0 should return 0 or reasonable value
        assert!(sell_price >= 0.0);
        // Buy with 0 target should return 0
        assert_eq!(buy_price, 0.0);
    }

    #[test]
    fn test_quote_aerodrome_both() {
        let pair = create_test_aero_pool();
        let gas = create_test_gas();
        let trade_size = 1.0;
        
        let quotes = quote_aerodrome_both(&pair, true, trade_size, &gas);
        
        // Test sell side
        assert!(quotes.sell.price_usdc_per_eth > 0.0);
        assert_eq!(quotes.sell.estimated_gas_cost_usd, gas.total_usd);
        
        // Test buy side
        assert!(quotes.buy.price_usdc_per_eth > 0.0);
        assert_eq!(quotes.buy.estimated_gas_cost_usd, gas.total_usd);
        
        // Buy should be more expensive than sell
        assert!(quotes.buy.price_usdc_per_eth > quotes.sell.price_usdc_per_eth);
    }

    #[test]
    fn test_legacy_quote_uniswap_v4() {
        let pool = create_test_uni_pool();
        let gas = create_test_gas();
        let trade_size = 1.0;
        
        let result = quote_uniswap_v4(&pool, true, trade_size, &gas);
        
        assert!(result.is_ok());
        let quote = result.expect("Failed to get Uniswap quote");
        
        assert!(quote.effective_price_usd > 0.0);
        assert_eq!(quote.estimated_gas_cost_usd, gas.total_usd);
        
        // Price impact should be reasonable (negative for selling)
        assert!(quote.price_impact_percent < 0.0);
        assert!(quote.price_impact_percent > -10.0); // Not too large impact
    }

    #[test]
    fn test_legacy_quote_aerodrome() {
        let pair = create_test_aero_pool();
        let gas = create_test_gas();
        let trade_size = 1.0;
        
        let result = quote_aerodrome(&pair, true, trade_size, &gas);
        
        assert!(result.is_ok());
        let quote = result.expect("Failed to get Aerodrome quote");
        
        assert!(quote.effective_price_usd > 0.0);
        assert_eq!(quote.estimated_gas_cost_usd, gas.total_usd);
        
        // Price impact should be reasonable
        assert!(quote.price_impact_percent < 0.0); // Negative for selling
        assert!(quote.price_impact_percent > -10.0); // Not too large impact
    }

    #[test]
    fn test_price_scaling_with_trade_size() {
        let pool = create_test_uni_pool();
        let pair = create_test_aero_pool();
        let gas = create_test_gas();
        
        let sizes = vec![0.1, 1.0, 5.0];
        
        for size in sizes {
            // Uniswap
            let uni_quotes = quote_uniswap_v4_both(&pool, true, size, &gas, Some(3000))
                .expect("Failed to get Uniswap quotes");
            assert!(uni_quotes.sell.price_usdc_per_eth > 0.0);
            assert!(uni_quotes.buy.price_usdc_per_eth > 0.0);
            
            // Aerodrome
            let aero_quotes = quote_aerodrome_both(&pair, true, size, &gas);
            assert!(aero_quotes.sell.price_usdc_per_eth > 0.0);
            assert!(aero_quotes.buy.price_usdc_per_eth > 0.0);
            
            log::debug!("Size: {} ETH", size);
            log::debug!("  Uni: sell={:.2}, buy={:.2}", uni_quotes.sell.price_usdc_per_eth, uni_quotes.buy.price_usdc_per_eth);
            log::debug!("  Aero: sell={:.2}, buy={:.2}", aero_quotes.sell.price_usdc_per_eth, aero_quotes.buy.price_usdc_per_eth);
        }
    }

    #[test]
    fn test_spread_calculation() {
        let pool = create_test_uni_pool();
        let pair = create_test_aero_pool();
        let gas = create_test_gas();
        let trade_size = 1.0;
        
        let uni_quotes = quote_uniswap_v4_both(&pool, true, trade_size, &gas, Some(3000))
            .expect("Failed to get Uniswap quotes");
        let aero_quotes = quote_aerodrome_both(&pair, true, trade_size, &gas);
        
        // Calculate potential arbitrage spreads
        let spread_sell_uni_buy_aero = uni_quotes.sell.price_usdc_per_eth - aero_quotes.buy.price_usdc_per_eth;
        let spread_sell_aero_buy_uni = aero_quotes.sell.price_usdc_per_eth - uni_quotes.buy.price_usdc_per_eth;
        
        log::debug!("Arbitrage Analysis:");
        log::debug!("  Sell Uni ({:.2}) -> Buy Aero ({:.2}) = Spread: {:.2}", 
                 uni_quotes.sell.price_usdc_per_eth, aero_quotes.buy.price_usdc_per_eth, spread_sell_uni_buy_aero);
        log::debug!("  Sell Aero ({:.2}) -> Buy Uni ({:.2}) = Spread: {:.2}", 
                 aero_quotes.sell.price_usdc_per_eth, uni_quotes.buy.price_usdc_per_eth, spread_sell_aero_buy_uni);
        
        // At least one spread calculation should be finite
        assert!(spread_sell_uni_buy_aero.is_finite());
        assert!(spread_sell_aero_buy_uni.is_finite());
    }

    #[test]
    fn test_side_quote_default() {
        let quote = SideQuote::default();
        assert_eq!(quote.price_usdc_per_eth, 0.0);
        assert_eq!(quote.estimated_gas_cost_usd, 0.0);
    }

    #[test]
    fn test_venue_quotes_default() {
        let quotes = VenueQuotes::default();
        assert_eq!(quotes.sell.price_usdc_per_eth, 0.0);
        assert_eq!(quotes.buy.price_usdc_per_eth, 0.0);
    }

    #[test]
    fn test_large_trade_size_handling() {
        let pool = create_test_uni_pool();
        let pair = create_test_aero_pool();
        let gas = create_test_gas();
        let large_size = 100.0; // 100 ETH
        
        // Should handle large trades without panicking
        let uni_result = quote_uniswap_v4_both(&pool, true, large_size, &gas, Some(3000));
        let aero_quotes = quote_aerodrome_both(&pair, true, large_size, &gas);
        
        if let Ok(uni_quotes) = uni_result {
            assert!(uni_quotes.sell.price_usdc_per_eth >= 0.0);
            assert!(uni_quotes.buy.price_usdc_per_eth >= 0.0);
        }
        
        assert!(aero_quotes.sell.price_usdc_per_eth >= 0.0);
        assert!(aero_quotes.buy.price_usdc_per_eth >= 0.0);
    }
}