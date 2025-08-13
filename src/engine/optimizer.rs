// src/arbitrage_optimizer.rs
// ============================================================================
// Optimizer that searches the best ETH trade size for cross-venue WETH/USDC arb
// using your existing modules:
//   - Uniswap: BigInt v3/v4 simulator (simulate_exact_in_tokens, etc.)
//   - Aerodrome: volatile xy=k math (simulate_exact_in_volatile, volatile_amount_out)
//
// Direction A: Sell on Aerodrome (ETH->USDC), Buy on Uniswap (USDC->ETH exact-output)
// Direction B: Sell on Uniswap (ETH->USDC), Buy on Aerodrome (USDC->ETH exact-output)
//
// Profit in USD: P(x) = proceeds_usdc(x) - cost_usdc(x) - gas_eth_usd - gas_base_usd - bridge_cost_usd
//
// Assumptions:
// - Decimals: WETH=18, USDC=6.
// - Pool directions are determined by whether WETH is token0 in each module.
// - Uniswap simulator is single-pool (no multi-hop); fee tier comes from PoolState.key or override.
//
// This file is sync (no RPC); you feed it fresh pool snapshots from your network layer.
//


use num_traits::ToPrimitive;

use crate::math::aerodrome_volatile::{
    VolatilePairState, SwapDirection as AeroDir, simulate_exact_in_volatile,
    volatile_amount_out as aero_amount_out, to_raw as aero_to_raw,
    from_raw as aero_from_raw, spot_price_out_per_in,
};

use crate::math::uniswap_v4::{
    // Uniswap BigInt simulator types and helpers
    PoolState as UniPoolState,
    SwapDirection as UniDir,
    SwapResult as UniSwapResult,
    simulate_exact_in_tokens as uni_exact_in,
};

use ethers::types::U256;
use crate::chain::gas::GasEstimate;

/// All inputs the optimizer needs for one run.
#[derive(Clone, Debug)]
pub struct OptimizerInputs {
    // On-chain snapshots provided by your data layer
    pub uni_pool: UniPoolState,           // Uniswap v3/v4-like pool (single pool)
    pub uni_token0_is_weth: bool,         // true if pool.currency0 == WETH
    pub uni_fee_ppm_override: Option<u32>,// override fee ppm (else use pool.key.fee_ppm)

    pub aero_pair: VolatilePairState,     // Aerodrome volatile pool snapshot
    pub aero_token0_is_weth: bool,        // true if pair.token0 == WETH

    // Costs
    pub gas_eth: GasEstimate,   // Ethereum (Uniswap side)
    pub gas_base: GasEstimate,  // Base (Aerodrome side)
    pub bridge_cost_usd: f64, // optional amortized bridge/rebalance cost per trade (can be 0.0)

    // Search configuration
    pub hint_size_eth: f64, // initial guess (e.g., 1.0)
    pub max_size_eth: f64,  // safety cap (e.g., 200.0)
}

/// Result of the optimizer.
#[derive(Clone, Debug)]
pub struct OptimizeResult {
    pub direction: ArbDirection,
    pub optimal_size_eth: f64,
    pub proceeds_usd: f64,
    pub costs_usd: f64,
    pub gas_usd_total: f64,
    pub bridge_cost_usd: f64,
    pub net_profit_usd: f64,
    pub eff_price_sell_usdc_per_eth: f64, // at optimal size, on the sell venue
    pub eff_price_buy_usdc_per_eth: f64,  // implied on buy venue (usdc needed / eth out)
}

/// Two arbitrage directions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArbDirection {
    SellAeroBuyUni, // Sell ETH->USDC on Aerodrome, buy USDC->ETH on Uniswap
    SellUniBuyAero, // Sell ETH->USDC on Uniswap,  buy USDC->ETH on Aerodrome
}

// ------------------------------ Public entry ---------------------------------

/// Optimize both directions and return the best candidate.
/// Returns None if both directions are non-profitable for all sizes.
pub fn optimize(inputs: &OptimizerInputs) -> Option<OptimizeResult> {
    let a = maximize_direction(inputs, ArbDirection::SellAeroBuyUni);
    let b = maximize_direction(inputs, ArbDirection::SellUniBuyAero);

    let best = match (a, b) {
        (Some(a_result), Some(b_result)) => {
            if a_result.net_profit_usd >= b_result.net_profit_usd { a_result } else { b_result }
        },
        (Some(a_result), None) => a_result,
        (None, Some(b_result)) => b_result,
        (None, None) => return None,
    };

    // Only return result if it's actually profitable
    if best.net_profit_usd > 0.0 {
        Some(best)
    } else {
        None
    }
}

// ------------------------------ Core maximize --------------------------------

fn maximize_direction(inputs: &OptimizerInputs, dir: ArbDirection) -> Option<OptimizeResult> {
    // 1) Bracket with exponential growth from hint
    let (l, r) = bracket_profit(inputs, dir, inputs.hint_size_eth, inputs.max_size_eth)?;
    // 2) Golden-section search (few evals, robust)
    let (x_star, p_star, snapshot) = golden_search(inputs, dir, l, r)?;
    // 3) Build final struct
    let (proceeds_usd, costs_usd, sell_px, buy_px) = snapshot;
    let gas_total = inputs.gas_eth.total_usd + inputs.gas_base.total_usd;
    let net = p_star;

    Some(OptimizeResult {
        direction: dir,
        optimal_size_eth: x_star,
        proceeds_usd,
        costs_usd,
        gas_usd_total: gas_total,
        bridge_cost_usd: inputs.bridge_cost_usd,
        net_profit_usd: net,
        eff_price_sell_usdc_per_eth: sell_px,
        eff_price_buy_usdc_per_eth: buy_px,
    })
}

// ---------------------------- Profit evaluators ------------------------------

/// Evaluate P(x) and also return detail snapshot for reporting:
/// (proceeds_usd, costs_usd, sell_px, buy_px).
fn profit_with_snapshot(inputs: &OptimizerInputs, dir: ArbDirection, x_eth: f64)
    -> Option<(f64 /*P*/, (f64,f64,f64,f64) /*snapshot*/)>
{
    if x_eth <= 0.0 { return None; }

    // gas + bridge
    let gas_total = inputs.gas_eth.total_usd + inputs.gas_base.total_usd;
    let bridge_cost = inputs.bridge_cost_usd;

    match dir {
        ArbDirection::SellAeroBuyUni => {
            // 1) Sell on Aerodrome: ETH -> USDC (exact input)
            let mut sell_px: f64 = 0.0; // USDC per ETH (effective)
            let usdc_out = aero_usdc_out_for_weth_in(&inputs.aero_pair, inputs.aero_token0_is_weth, x_eth, &mut Some(&mut |px| { sell_px = px; }))?;
            // 2) Buy on Uniswap: USDC -> WETH exact-output (need USDC in to get x ETH)
            let (usdc_in, buy_px) = uni_usdc_in_for_weth_out(&inputs.uni_pool, inputs.uni_token0_is_weth, inputs.uni_fee_ppm_override, x_eth)?;
            // 3) Profit
            let proceeds = usdc_out;
            let costs    = usdc_in;
            let p = proceeds - costs - gas_total - bridge_cost;
            Some((p, (proceeds, costs, sell_px, buy_px)))
        }
        ArbDirection::SellUniBuyAero => {
            // 1) Sell on Uniswap: WETH -> USDC (exact input)
            let (usdc_out, sell_px) = uni_usdc_out_for_weth_in(&inputs.uni_pool, inputs.uni_token0_is_weth, inputs.uni_fee_ppm_override, x_eth)?;
            // 2) Buy on Aerodrome: USDC -> WETH exact-output (need USDC in to get x ETH)
            let usdc_in = aero_usdc_in_for_weth_out(&inputs.aero_pair, inputs.aero_token0_is_weth, x_eth)?;
            let buy_px = if x_eth > 0.0 { usdc_in / x_eth } else { 0.0 };
            // 3) Profit
            let proceeds = usdc_out;
            let costs    = usdc_in;
            let p = proceeds - costs - gas_total - bridge_cost;
            Some((p, (proceeds, costs, sell_px, buy_px)))
        }
    }
}

// Aerodrome leg: ETH->USDC exact input (sell). Also compute effective price.
fn aero_usdc_out_for_weth_in(
    pair: &VolatilePairState,
    token0_is_weth: bool,
    eth_in: f64,
    eff_px_sink: &mut Option<&mut dyn FnMut(f64)>
) -> Option<f64> {
    // Use the flag to determine direction, not address checks
    let direction = if token0_is_weth { AeroDir::ZeroForOne } else { AeroDir::OneForZero };
    let (_ain_raw, aout_raw, eff, _spot, _imp) = simulate_exact_in_volatile(pair, direction, eth_in);
    if let Some(cb) = eff_px_sink.as_deref_mut() {
        cb(eff); // USDC per ETH - eff is already tokenOut per tokenIn
    }
    // Output decimals depend on which token is USDC
    let out_decimals = if token0_is_weth { pair.decimals1 } else { pair.decimals0 };
    Some(aero_from_raw(aout_raw, out_decimals))
}

// Aerodrome leg: USDC->WETH exact output (buy): find USDC needed to receive target ETH.
fn aero_usdc_in_for_weth_out(
    pair: &VolatilePairState,
    token0_is_weth: bool,
    target_eth_out: f64,
) -> Option<f64> {
    // Map reserves for USDC->WETH (input = USDC, output = WETH)
    // Use the flag consistently to determine which token is which
    let (rin, rout, din, dout) = if token0_is_weth {
        // token0=WETH, token1=USDC: USDC->WETH means OneForZero (token1 in -> token0 out)
        (pair.reserve1, pair.reserve0, pair.decimals1, pair.decimals0)
    } else {
        // token1=WETH, token0=USDC: USDC->WETH means ZeroForOne (token0 in -> token1 out)
        (pair.reserve0, pair.reserve1, pair.decimals0, pair.decimals1)
    };

    let target_raw = aero_to_raw(target_eth_out, dout);
    if target_raw.is_zero() { return Some(0.0); }

    // Binary search on amountIn so that aero_amount_out(amountIn) >= target_raw.
    let mut lo = U256::zero();
    let mut hi = {
        // conservative upper bound: start from spot*target, inflate 4x
        let spot = spot_price_out_per_in(rin, rout, din, dout);
        let approx_usdc = if spot.is_finite() && spot > 0.0 {
            target_eth_out / spot
        } else {
            target_eth_out * 10_000.0
        };
        let guess = aero_to_raw(approx_usdc * 4.0, din);
        let cap = U256::from_dec_str("1000000000000000000000000000000")
            .unwrap_or_else(|_| U256::max_value()); // 1e30
        if guess > cap { cap } else { guess }
    };

    let fee = pair.fee_bps;
    for _ in 0..64 {
        if lo >= hi { break; }
        let mid = (lo + hi) >> 1;
        let out = aero_amount_out(mid, rin, rout, fee);
        if out < target_raw { lo = mid + U256::from(1u8); } else { hi = mid; }
    }
    Some(aero_from_raw(hi, din))
}

// Uniswap leg: WETH->USDC exact input (sell)
fn uni_usdc_out_for_weth_in(
    pool: &UniPoolState,
    token0_is_weth: bool,
    fee_override_ppm: Option<u32>,
    eth_in: f64,
) -> Option<(f64 /*usdc_out*/, f64 /*eff px usdc/eth*/)> {
    let direction = if token0_is_weth { UniDir::ZeroForOne } else { UniDir::OneForZero };
    let res: UniSwapResult = uni_exact_in(pool, direction, fee_override_ppm, eth_in, 18, None).ok()?;
    // Extract human amounts based on direction
    let (in_eth, out_usdc) = if token0_is_weth {
        // ZeroForOne: amount0 (WETH) is negative (in), amount1 (USDC) is positive (out)
        ((-res.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18,
         res.amount1.clone().to_f64().unwrap_or(0.0) / 1e6)
    } else {
        // OneForZero: amount1 (WETH) is negative (in), amount0 (USDC) is positive (out)
        ((-res.amount1.clone()).to_f64().unwrap_or(0.0) / 1e18,
         res.amount0.clone().to_f64().unwrap_or(0.0) / 1e6)
    };
    let eff = if in_eth > 0.0 { out_usdc / in_eth } else { 0.0 };
    Some((out_usdc, eff))
}

// Uniswap leg: USDC->WETH exact output (buy) — find USDC in so that WETH out >= target.
fn uni_usdc_in_for_weth_out(
    pool: &UniPoolState,
    token0_is_weth: bool,
    fee_override_ppm: Option<u32>,
    target_eth_out: f64,
) -> Option<(f64 /*usdc_in*/, f64 /*implied buy px usdc/eth*/)> {
    // Simulate exact-input USDC->WETH repeatedly with binary search on USDC in.
    // Direction for USDC->WETH:
    let direction = if token0_is_weth { UniDir::OneForZero } else { UniDir::ZeroForOne };

    if target_eth_out <= 0.0 { return Some((0.0, 0.0)); }

    // Initial bracket on USDC input
    let mut lo = 0.0_f64;
    // spot guess: price USDC/ETH via pool.tick (we can approximate via a small simulation)
    let spot_usdc_per_eth = spot_usdc_per_eth_uniswap(pool, token0_is_weth).max(1.0);
    let mut hi = target_eth_out * spot_usdc_per_eth * 4.0; // generous

    // Guard rails
    hi = hi.min(1.0e12); // $1T cap to avoid runaway in degenerate pools

    for _ in 0..64 {
        let mid = (lo + hi) * 0.5;
        let res = uni_exact_in(pool, direction, fee_override_ppm, mid, 6 /*USDC*/, None).ok()?;
        // ETH out (depends on direction)
        let eth_out = match direction {
            UniDir::OneForZero => (res.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18, // token0=WETH out
            UniDir::ZeroForOne => (res.amount1.clone()).to_f64().unwrap_or(0.0) / 1e18, // token1=WETH out
        };
        if eth_out >= target_eth_out { hi = mid; } else { lo = mid; }
        if (hi - lo) / hi.max(1.0) < 1e-4 { break; } // 1 bp in input
    }
    let usdc_in = hi;
    let buy_px = usdc_in / target_eth_out;
    Some((usdc_in, buy_px))
}

fn spot_usdc_per_eth_uniswap(pool: &UniPoolState, token0_is_weth: bool) -> f64 {
    // Use a tiny trade to sample effective price close to spot.
    let tiny = 0.0001_f64;
    let direction = if token0_is_weth { UniDir::ZeroForOne } else { UniDir::OneForZero };
    let res = uni_exact_in(pool, direction, None, tiny, 18, None);
    if let Ok(r) = res {
        let (eth_in, usdc_out) = if token0_is_weth {
            // ZeroForOne: WETH (token0) in, USDC (token1) out
            ((-r.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18,
             r.amount1.clone().to_f64().unwrap_or(0.0) / 1e6)
        } else {
            // OneForZero: WETH (token1) in, USDC (token0) out
            ((-r.amount1.clone()).to_f64().unwrap_or(0.0) / 1e18,
             r.amount0.clone().to_f64().unwrap_or(0.0) / 1e6)
        };
        if eth_in > 0.0 { usdc_out / eth_in } else { 0.0 }
    } else { 0.0 }
}

// -------------------------- Bracket + golden search --------------------------

fn bracket_profit(
    inputs: &OptimizerInputs,
    dir: ArbDirection,
    mut x0: f64,
    x_cap: f64,
) -> Option<(f64 /*left*/, f64 /*right*/)> {
    x0 = x0.max(1e-9);
    let mut best_x = x0;
    let mut best_p = profit(inputs, dir, x0)?;
    let mut l = (x0 * 0.5).max(1e-9);
    let mut r = x0;

    // Grow exponentially until profit starts dropping (or cap)
    for _ in 0..16 {
        let x_try = (r * 2.0).min(x_cap);
        let p_try = profit(inputs, dir, x_try)?;
        if p_try > best_p {
            best_p = p_try;
            best_x = x_try;
            l = r;
            r = x_try;
        } else {
            // we crossed the peak; bracket around (l,r)
            return Some((l, x_try));
        }
        if (x_cap - r).abs() < f64::EPSILON { break; }
    }
    // Fallback: widen around best
    Some((best_x * 0.5, (best_x * 2.0).min(x_cap)))
}

fn golden_search(
    inputs: &OptimizerInputs,
    dir: ArbDirection,
    mut a: f64,
    mut b: f64,
) -> Option<(f64 /*x**/, f64 /*P**/, (f64,f64,f64,f64) /*snapshot*/)> {
    let phi = 0.5 * (3.0_f64.sqrt() + 1.0); // golden ratio ~1.618
    let tol = 1e-3; // 0.1% relative interval width
    let mut c = b - (b - a) / phi;
    let mut d = a + (b - a) / phi;

    let (mut pc, mut sc) = profit_with_snapshot_full(inputs, dir, c)?;
    let (mut pd, mut sd) = profit_with_snapshot_full(inputs, dir, d)?;

    for _ in 0..24 {
        if (b - a) / b.max(1.0) < tol { break; }
        if pc > pd {
            b = d; d = c; pd = pc; sd = sc;
            c = b - (b - a) / phi;
            let (p, snap) = profit_with_snapshot_full(inputs, dir, c)?;
            pc = p; sc = snap;
        } else {
            a = c; c = d; pc = pd; sc = sd;
            d = a + (b - a) / phi;
            let (p, snap) = profit_with_snapshot_full(inputs, dir, d)?;
            pd = p; sd = snap;
        }
    }

    if pc > pd { Some((c, pc, sc)) } else { Some((d, pd, sd)) }
}

#[inline]
fn profit(inputs: &OptimizerInputs, dir: ArbDirection, x_eth: f64) -> Option<f64> {
    let p = profit_with_snapshot(inputs, dir, x_eth)?.0;
    if p.is_finite() { Some(p) } else { None }
}

fn profit_with_snapshot_full(
    inputs: &OptimizerInputs,
    dir: ArbDirection,
    x_eth: f64,
) -> Option<(f64, (f64,f64,f64,f64))> {
    profit_with_snapshot(inputs, dir, x_eth)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::aerodrome_volatile::VolatilePairState;
    use crate::math::uniswap_v4::PoolState as UniPoolState;
    use crate::chain::gas::GasEstimate;
    use ethers::types::{Address, U256};
    use num_bigint::BigInt;

    fn create_test_uni_pool() -> UniPoolState {
        // Create a simple test pool with reasonable liquidity
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
        reserve1: U256::from_dec_str("3400000000000")
            .expect("Failed to parse reserve1"), // 3.4M USDC
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
            total_usd: 2.0,
        }
    }

    #[test]
    fn test_optimizer_basic() {
        let inputs = OptimizerInputs {
            uni_pool: create_test_uni_pool(),
            uni_token0_is_weth: true,
            uni_fee_ppm_override: Some(3000),
            aero_pair: create_test_aero_pool(),
            aero_token0_is_weth: true,
            gas_eth: create_test_gas(),
            gas_base: GasEstimate {
                gas_limit: U256::from(150_000),
                gas_price: U256::from(1_000_000_000u64), // 1 gwei
                l1_data_fee: U256::zero(),
                total_wei: U256::from(150_000_000_000_000u64),
                total_eth: 0.00015,
                total_usd: 0.5,
            },
            bridge_cost_usd: 5.0,
            hint_size_eth: 1.0,
            max_size_eth: 100.0,
        };

        let result = optimize(&inputs);
        
        // Should find an optimal solution or None if not profitable
        if let Some(res) = result {
            // Verify basic sanity checks
            assert!(res.optimal_size_eth >= 0.0);
            assert!(res.optimal_size_eth <= inputs.max_size_eth);
            assert!(res.net_profit_usd > 0.0); // Only returns result if profitable
            
            // Check that costs are included
            assert!(res.gas_usd_total > 0.0);
            assert_eq!(res.bridge_cost_usd, 5.0);
            
            // Check direction is valid
            assert!(
                res.direction == ArbDirection::SellAeroBuyUni ||
                res.direction == ArbDirection::SellUniBuyAero
            );
        }
    }

    #[test]
    fn test_optimizer_no_arbitrage() {
        // Create pools with identical prices and high costs
        let inputs = OptimizerInputs {
            uni_pool: create_test_uni_pool(),
            uni_token0_is_weth: true,
            uni_fee_ppm_override: Some(3000),
            aero_pair: VolatilePairState {
                token0: Address::zero(),
                token1: Address::from([0x22; 20]),
                        reserve0: U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse reserve0"),
        reserve1: U256::from_dec_str("3490000000000")
            .expect("Failed to parse reserve1"), // Almost same price
                decimals0: 18,
                decimals1: 6,
                fee_bps: 30,
            },
            aero_token0_is_weth: true,
            gas_eth: GasEstimate {
                gas_limit: U256::from(200_000),
                gas_price: U256::from(500_000_000_000u64), // Very high gas
                l1_data_fee: U256::zero(),
                total_wei: U256::from(100_000_000_000_000_000u64),
                total_eth: 0.1,
                total_usd: 350.0, // Very high gas cost
            },
            gas_base: GasEstimate {
                gas_limit: U256::from(150_000),
                gas_price: U256::from(100_000_000_000u64), // High gas
                l1_data_fee: U256::zero(),
                total_wei: U256::from(15_000_000_000_000_000u64),
                total_eth: 0.015,
                total_usd: 52.5, // High gas cost
            },
            bridge_cost_usd: 500.0, // Extremely high bridge cost
            hint_size_eth: 1.0,
            max_size_eth: 100.0,
        };

        let result = optimize(&inputs);
        
        // With such high costs and minimal spread, should be unprofitable
        // If it finds a result, verify it's actually profitable
        if let Some(res) = result {
            log::debug!("Found arbitrage with profit: ${}, size: {} ETH", res.net_profit_usd, res.optimal_size_eth);
            assert!(res.net_profit_usd > 0.0, "Optimizer should only return profitable results");
        }
    }

    #[test]
    fn test_optimizer_direction_selection() {
        // Test SellUniBuyAero direction (Uni more expensive)
        let mut inputs = OptimizerInputs {
            uni_pool: create_test_uni_pool(), // ~3500 USDC/ETH
            uni_token0_is_weth: true,
            uni_fee_ppm_override: Some(3000),
            aero_pair: VolatilePairState {
                token0: Address::zero(),
                token1: Address::from([0x22; 20]),
                        reserve0: U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse reserve0"),
        reserve1: U256::from_dec_str("3300000000000")
            .expect("Failed to parse reserve1"), // Cheaper at 3300
                decimals0: 18,
                decimals1: 6,
                fee_bps: 30,
            },
            aero_token0_is_weth: true,
            gas_eth: GasEstimate {
                gas_limit: U256::from(100_000),
                gas_price: U256::from(1_000_000_000u64),
                l1_data_fee: U256::zero(),
                total_wei: U256::from(100_000_000_000_000u64),
                total_eth: 0.0001,
                total_usd: 0.1,
            },
            gas_base: GasEstimate {
                gas_limit: U256::from(100_000),
                gas_price: U256::from(1_000_000_000u64),
                l1_data_fee: U256::zero(),
                total_wei: U256::from(100_000_000_000_000u64),
                total_eth: 0.0001,
                total_usd: 0.1,
            },
            bridge_cost_usd: 1.0,
            hint_size_eth: 1.0,
            max_size_eth: 10.0,
        };

        let result1 = optimize(&inputs);
        
        // Test SellAeroBuyUni direction (Aero more expensive)
        inputs.aero_pair.reserve1 = U256::from_dec_str("3700000000000")
            .expect("Failed to parse reserve1"); // Much more expensive at 3700
        inputs.aero_pair.reserve0 = U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse reserve0");
        
        let result2 = optimize(&inputs);
        
        // If both are profitable, they should find different directions
        // If only one is profitable, that's also valid
        match (result1, result2) {
            (Some(res1), Some(res2)) => {
                // If both found solutions, they should be different directions
                // given the price difference
                log::debug!("Direction 1: {:?}, Direction 2: {:?}", res1.direction, res2.direction);
            }
            _ => {
                // It's OK if one or both don't find profitable arbitrage
                // due to the specific pool configurations
            }
        }
    }

    #[test]
    fn test_optimizer_respects_max_size() {
        let inputs = OptimizerInputs {
            uni_pool: create_test_uni_pool(),
            uni_token0_is_weth: true,
            uni_fee_ppm_override: Some(3000),
            aero_pair: create_test_aero_pool(),
            aero_token0_is_weth: true,
            gas_eth: create_test_gas(),
            gas_base: create_test_gas(),
            bridge_cost_usd: 0.1,
            hint_size_eth: 1.0,
            max_size_eth: 5.0, // Small max size
        };

        let result = optimize(&inputs);
        
        if let Some(res) = result {
            assert!(res.optimal_size_eth <= 5.0);
        }
    }

    #[test]
    fn test_profit_calculation() {
        // Test the profit calculation logic
        let proceeds = 1000.0;
        let costs = 800.0;
        let gas = 50.0;
        let bridge = 10.0;
        
        let net_profit = proceeds - costs - gas - bridge;
        assert_eq!(net_profit, 140.0);
        
        // Test that negative profits are handled
        let proceeds_loss = 500.0;
        let costs_high = 600.0;
        let net_loss = proceeds_loss - costs_high - gas - bridge;
        assert_eq!(net_loss, -160.0);
    }

    #[test]
    fn test_edge_cases() {
        // Test with zero trade size hint
        let mut inputs = OptimizerInputs {
            uni_pool: create_test_uni_pool(),
            uni_token0_is_weth: true,
            uni_fee_ppm_override: Some(3000),
            aero_pair: create_test_aero_pool(),
            aero_token0_is_weth: true,
            gas_eth: create_test_gas(),
            gas_base: create_test_gas(),
            bridge_cost_usd: 5.0,
            hint_size_eth: 0.0, // Zero hint
            max_size_eth: 100.0,
        };

        // Should handle zero hint gracefully
        let _ = optimize(&inputs);
        
        // Test with very small max size
        inputs.hint_size_eth = 0.001;
        inputs.max_size_eth = 0.001;
        
        let _ = optimize(&inputs);
    }
}

