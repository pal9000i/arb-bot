// Single-file BigInt Uniswap v3/v4 math + exact-input swap simulator + best-pool selection
// ----------------------------------------------------------------------------------------
// Cargo.toml dependencies:
//   num-bigint = "0.4"
//   num-integer = "0.1"
//   num-traits = "0.2"
//
// Notes:
// - This uses BigInt end-to-end to avoid overflow/rounding bugs and to mirror the reference
//   implementations cleanly. Rounding semantics match Uniswap (two-step ceil for token0).
// - You provide the pool(s). This file does not use Quoter; it reads pool state you pass in.
// - Amount signs in results: negative = spent, positive = received.
//
// If you want byte-for-byte parity with EVM fixed-width later, you can switch intermediates
// to U256/U512 after you’re satisfied with correctness here.

use std::cmp::min;
use std::collections::BTreeMap;

use ethers::types::Address;
use num_bigint::BigInt;
use num_traits::{One, Zero, ToPrimitive, Signed};

const MIN_TICK: i32 = -887_272;
const MAX_TICK: i32 =  887_272;
const FEE_DENOMINATOR_PPM: i64 = 1_000_000; // ppm
const Q96_U128: u128 = 1u128 << 96;

// --------------------------------- Helpers ---------------------------------

#[inline]
fn bi(v: i64) -> BigInt { BigInt::from(v) }

#[inline]
fn bu128(v: u128) -> BigInt { BigInt::from(v) }

#[inline]
fn ceil_div(a: &BigInt, b: &BigInt) -> BigInt {
    // assumes a>=0, b>0
    if a.is_zero() { return BigInt::zero(); }
    (a + (b - BigInt::one()) ) / b
}

#[inline]
fn max_bi(a: &BigInt, b: &BigInt) -> BigInt { if a > b { a.clone() } else { b.clone() } }

#[inline]
fn min_bi(a: &BigInt, b: &BigInt) -> BigInt { if a < b { a.clone() } else { b.clone() } }

// -------------------------------- Tick Math --------------------------------

/// Exact TickMath.getSqrtRatioAtTick (Q64.96 integer), ported with canonical constants.
pub fn get_sqrt_ratio_at_tick(tick: i32) -> BigInt {
    assert!((MIN_TICK..=MAX_TICK).contains(&tick), "tick out of range");
    let abs_tick = tick.unsigned_abs();

    // ratio is Q128.128
    let mut ratio = if abs_tick & 0x1 != 0 {
        BigInt::parse_bytes(b"fffcb933bd6fad37aa2d162d1a594001", 16)
            .expect("Failed to parse BigInt constant")
    } else {
        BigInt::one() << 128
    };

    macro_rules! ms {
        ($hex:literal, $cond:expr) => {
            if $cond {
                ratio = ( &ratio * BigInt::parse_bytes($hex.as_bytes(), 16)
                    .expect(&format!("Failed to parse BigInt constant: {}", $hex)) ) >> 128;
            }
        };
    }

    ms!("fff97272373d413259a46990580e213a", (abs_tick & 0x2)     != 0);
    ms!("fff2e50f5f656932ef12357cf3c7fdcc", (abs_tick & 0x4)     != 0);
    ms!("ffe5caca7e10e4e61c3624eaa0941cd0", (abs_tick & 0x8)     != 0);
    ms!("ffcb9843d60f6159c9db58835c926644", (abs_tick & 0x10)    != 0);
    ms!("ff973b41fa98c081472e6896dfb254c0", (abs_tick & 0x20)    != 0);
    ms!("ff2ea16466c96a3843ec78b326b52861", (abs_tick & 0x40)    != 0);
    ms!("fe5dee046a99a2a811c461f1969c3053", (abs_tick & 0x80)    != 0);
    ms!("fcbe86c7900a88aedcffc83b479aa3a4", (abs_tick & 0x100)   != 0);
    ms!("f987a7253ac413176f2b074cf7815e54", (abs_tick & 0x200)   != 0);
    ms!("f3392b0822b70005940c7a398e4b70f3", (abs_tick & 0x400)   != 0);
    ms!("e7159475a2c29b7443b29c7fa6e889d9", (abs_tick & 0x800)   != 0);
    ms!("d097f3bdfd2022b8845ad8f792aa5825", (abs_tick & 0x1000)  != 0);
    ms!("a9f746462d870fdf8a65dc1f90e061e5", (abs_tick & 0x2000)  != 0);
    ms!("70d869a156d2a1b890bb3df62baf32f7", (abs_tick & 0x4000)  != 0);
    ms!("31be135f97d08fd981231505542fcfa6", (abs_tick & 0x8000)  != 0);
    ms!("09aa508b5b7a84e1c677de54f3e99bc9", (abs_tick & 0x10000) != 0);
    ms!("05d6af8dedb81196699c329225ee604",  (abs_tick & 0x20000) != 0);
    ms!("01dcdc6f2d7c3395a2ed4f8b7feaf38",  (abs_tick & 0x40000) != 0);
    ms!("48a170391f7dc42444e8fa2",          (abs_tick & 0x80000) != 0);

    if tick > 0 {
        let max = (BigInt::one() << 256) - 1;
        ratio = max / ratio;
    }
    // round-up shift by 32 (Q128.128 -> Q64.96)
    ( &ratio + ( (BigInt::one() << 32) - 1 ) ) >> 32
}

/// Binary search inverse of get_sqrt_ratio_at_tick (exact on-grid).
pub fn get_tick_at_sqrt_ratio(sqrt_price_x96: &BigInt) -> i32 {
    let mut lo = MIN_TICK;
    let mut hi = MAX_TICK;
    while lo < hi {
        let mid = lo + ((hi - lo + 1) / 2);
        if get_sqrt_ratio_at_tick(mid) <= *sqrt_price_x96 { lo = mid; } else { hi = mid - 1; }
    }
    lo
}

// --------------------------- SqrtPriceMath deltas ---------------------------

/// Uniswap-exact rounding:
/// amount0 =
///   if round_up:
///     ceil( ceil( (L << 96) * (sb - sa) / sb ) / sa )
///   else:
///     floor( floor( (L << 96) * (sb - sa) / sb ) / sa )
pub fn amount0_delta(
    sqrt_ratio_a_x96: &BigInt,
    sqrt_ratio_b_x96: &BigInt,
    liquidity: &BigInt,
    round_up: bool,
) -> BigInt {
    if liquidity.is_zero() { return BigInt::zero(); }
    let (sa, sb) = if sqrt_ratio_a_x96 < sqrt_ratio_b_x96 {
        (sqrt_ratio_a_x96.clone(), sqrt_ratio_b_x96.clone())
    } else {
        (sqrt_ratio_b_x96.clone(), sqrt_ratio_a_x96.clone())
    };
    if sa.is_zero() || sa == sb { return BigInt::zero(); }

    let numerator1 = liquidity << 96;
    let numerator2 = &sb - &sa;

    if round_up {
        // ceil( ceil(n1*n2 / sb) / sa )
        let t = ceil_div(&( &numerator1 * &numerator2 ), &sb);
        ceil_div(&t, &sa)
    } else {
        // floor( floor(n1*n2 / sb) / sa )
        ( (&numerator1 * &numerator2) / &sb ) / &sa
    }
}

/// Uniswap-exact rounding:
/// amount1 =
///   if round_up: ceil( L * (sb - sa) / Q96 )
///   else:       floor( L * (sb - sa) / Q96 )
pub fn amount1_delta(
    sqrt_ratio_a_x96: &BigInt,
    sqrt_ratio_b_x96: &BigInt,
    liquidity: &BigInt,
    round_up: bool,
) -> BigInt {
    if liquidity.is_zero() { return BigInt::zero(); }
    let (sa, sb) = if sqrt_ratio_a_x96 < sqrt_ratio_b_x96 {
        (sqrt_ratio_a_x96.clone(), sqrt_ratio_b_x96.clone())
    } else {
        (sqrt_ratio_b_x96.clone(), sqrt_ratio_a_x96.clone())
    };
    if sa == sb { return BigInt::zero(); }

    let num = liquidity * (sb - sa);
    let den = bu128(Q96_U128);
    if round_up {
        ceil_div(&num, &den)
    } else {
        num / den
    }
}

// Signed variants per Uniswap conventions (rounding depends on sign).

// ----------------------------- Next price helpers -----------------------------

fn default_limit(direction: SwapDirection) -> BigInt {
    match direction {
        SwapDirection::ZeroForOne => get_sqrt_ratio_at_tick(MIN_TICK + 1),
        SwapDirection::OneForZero => get_sqrt_ratio_at_tick(MAX_TICK - 1),
    }
}

#[inline]
fn next_sqrt_from_input_zero_for_one(
    liquidity: &BigInt,
    sqrt_p_x96: &BigInt,
    amount_in_net: &BigInt,
) -> BigInt {
    // Uniswap-exact: getNextSqrtPriceFromAmount0RoundingUp
    // sqrtQ = ceil( ( (L<<96) * sqrtP ) / ( (L<<96) + amountIn * sqrtP ) )
    if amount_in_net.is_zero() || liquidity.is_zero() { return sqrt_p_x96.clone(); }

    let numerator1   = liquidity << 96;                 // L<<96
    let numerator    = &numerator1 * sqrt_p_x96;        // (L<<96) * sqrtP
    let denominator  = &numerator1 + amount_in_net * sqrt_p_x96; // (L<<96) + amountIn*sqrtP

    ceil_div(&numerator, &denominator)
}

#[inline]
fn next_sqrt_from_input_one_for_zero(liquidity: &BigInt, sqrt_p_x96: &BigInt, amount_in_net: &BigInt) -> BigInt {
    // sqrtQ = P + floor( amountIn * Q96 / L )
    if amount_in_net.is_zero() || liquidity.is_zero() { return sqrt_p_x96.clone(); }
    let inc = (amount_in_net * bu128(Q96_U128)) / liquidity;
    sqrt_p_x96 + inc
}

// ------------------------------- Data types ----------------------------------

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct PoolKey {
    pub currency0: Address,
    pub currency1: Address,
    pub fee_ppm: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}
impl PoolKey {
    #[allow(dead_code)]
    pub fn pool_id(&self) -> String {
        format!("{:x?}-{:x?}-{}-{}-{:x?}", self.currency0, self.currency1, self.fee_ppm, self.tick_spacing, self.hooks)
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct TickInfo {
    pub tick: i32,
    pub liquidity_net: BigInt, // signed
}

#[derive(Clone, Debug)]
pub struct PoolState {
    pub key: PoolKey,
    pub sqrt_price_x96: BigInt,
    pub tick: i32,
    pub liquidity: BigInt, // non-negative
    pub ticks: BTreeMap<i32, TickInfo>, // initialized ticks
}

#[derive(Copy, Clone, Debug)]
pub enum SwapDirection { ZeroForOne, OneForZero }

#[derive(Clone, Debug)]
pub struct SwapParams {
    pub direction: SwapDirection,
    pub amount_specified: BigInt,     // exact input (>0)
    pub sqrt_price_limit_x96: BigInt, // bound
    pub fee_ppm: u32,
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct SwapResult {
    pub amount0: BigInt,        // negative if spent
    pub amount1: BigInt,        // positive if received
    pub sqrt_price_x96: BigInt,
    pub tick: i32,
    pub liquidity: BigInt,
    pub crossed_ticks: usize,
}

// Hook fee (optional)
pub trait HookFee {
    fn adjust_fee_ppm(&self, _pool: &PoolState, _params: &SwapParams, _remaining_in: &BigInt) -> u32 { 0 }
}
pub struct NoHook;
impl HookFee for NoHook {}

// ------------------------------- Swap math step -------------------------------

fn compute_swap_step(
    sqrt_price_x96: &BigInt,
    sqrt_price_target_x96: &BigInt,
    liquidity: &BigInt,
    amount_remaining: &BigInt,
    fee_ppm: u32,
    zero_for_one: bool,
) -> (BigInt /*sqrtQ*/, BigInt /*amountIn*/, BigInt /*amountOut*/, BigInt /*fee*/ )
{
    let fee_ppm_bi = bi(fee_ppm as i64);
    let denom = bi(FEE_DENOMINATOR_PPM);
    let fee_complement = &denom - &fee_ppm_bi;

    // amount remaining net of fee (if we partially move)
    let amount_remaining_less_fee = (amount_remaining * &fee_complement) / &denom;

    if zero_for_one {
        // input needed (net) to reach target
        let amount_in_to_target = amount0_delta(sqrt_price_target_x96, sqrt_price_x96, liquidity, true);
        // gross with fee
        let gross_to_target = ceil_div(&( &amount_in_to_target * &denom ), &fee_complement);

        if &gross_to_target <= amount_remaining {
            // reach target
            let amount_out = amount1_delta(sqrt_price_target_x96, sqrt_price_x96, liquidity, false);
            let fee_amt = &gross_to_target - &amount_in_to_target;
            (sqrt_price_target_x96.clone(), amount_in_to_target, amount_out, fee_amt)
        } else {
            // partial move
            let sqrt_q = next_sqrt_from_input_zero_for_one(liquidity, sqrt_price_x96, &amount_remaining_less_fee);
            let amount_in_used  = amount0_delta(&sqrt_q, sqrt_price_x96, liquidity, true);
            let amount_out_recv = amount1_delta(&sqrt_q, sqrt_price_x96, liquidity, false);
            let gross_used = ceil_div(&( &amount_in_used * &denom ), &fee_complement);
            let fee_amt = &gross_used - &amount_in_used;
            (sqrt_q, amount_in_used, amount_out_recv, fee_amt)
        }
    } else {
        // one for zero
        let amount_in_to_target = amount1_delta(sqrt_price_x96, sqrt_price_target_x96, liquidity, true);
        let gross_to_target = ceil_div(&( &amount_in_to_target * &denom ), &fee_complement);

        if &gross_to_target <= amount_remaining {
            let amount_out = amount0_delta(sqrt_price_x96, sqrt_price_target_x96, liquidity, false);
            let fee_amt = &gross_to_target - &amount_in_to_target;
            (sqrt_price_target_x96.clone(), amount_in_to_target, amount_out, fee_amt)
        } else {
            let sqrt_q = next_sqrt_from_input_one_for_zero(liquidity, sqrt_price_x96, &amount_remaining_less_fee);
            let amount_in_used  = amount1_delta(sqrt_price_x96, &sqrt_q, liquidity, true);
            let amount_out_recv = amount0_delta(sqrt_price_x96, &sqrt_q, liquidity, false);
            let gross_used = ceil_div(&( &amount_in_used * &denom ), &fee_complement);
            let fee_amt = &gross_used - &amount_in_used;
            (sqrt_q, amount_in_used, amount_out_recv, fee_amt)
        }
    }
}

// -------------------------------- Simulator ---------------------------------

fn next_initialized_tick(
    ticks: &BTreeMap<i32, TickInfo>,
    current_tick: i32,
    direction: SwapDirection,
) -> (i32, bool) {
    match direction {
        SwapDirection::ZeroForOne => {
            if let Some((&t, _)) = ticks.range(..=current_tick).next_back() { (t, true) } else { (MIN_TICK, false) }
        }
        SwapDirection::OneForZero => {
            if let Some((&t, _)) = ticks.range(current_tick + 1 ..).next()     { (t, true) } else { (MAX_TICK, false) }
        }
    }
}

pub fn simulate_swap(pool: &PoolState, params: &SwapParams, hook: &dyn HookFee) -> Result<SwapResult, String> {
    if params.amount_specified <= BigInt::zero() {
        return Err("amount_specified must be positive (exact input)".into());
    }

    let mut amount_remaining = params.amount_specified.clone();
    let mut sqrt_price       = pool.sqrt_price_x96.clone();
    let mut liquidity        = pool.liquidity.clone();
    let mut current_tick     = pool.tick;

    let mut amount0_total = BigInt::zero();
    let mut amount1_total = BigInt::zero();
    let mut ticks_crossed = 0usize;

    // price limit sanity
    match params.direction {
        SwapDirection::ZeroForOne => {
            if params.sqrt_price_limit_x96 >= sqrt_price {
                return Err("price limit must be < current sqrt for ZeroForOne".into());
            }
        }
        SwapDirection::OneForZero => {
            if params.sqrt_price_limit_x96 <= sqrt_price {
                return Err("price limit must be > current sqrt for OneForZero".into());
            }
        }
    }

    while amount_remaining > BigInt::zero() && liquidity > BigInt::zero() {
        let hook_adj = hook.adjust_fee_ppm(pool, params, &amount_remaining);
        let eff_fee_ppm = min(
            (FEE_DENOMINATOR_PPM - 1) as u32,
            params.fee_ppm.saturating_add(hook_adj)
        );

        let (next_tick, has_next) = next_initialized_tick(&pool.ticks, current_tick, params.direction);
        let sqrt_next = if has_next { get_sqrt_ratio_at_tick(next_tick) } else {
            default_limit(params.direction)
        };

        let sqrt_target_bound = match params.direction {
            SwapDirection::ZeroForOne => max_bi(&params.sqrt_price_limit_x96, &sqrt_next),
            SwapDirection::OneForZero  => min_bi(&params.sqrt_price_limit_x96, &sqrt_next),
        };

        let zero_for_one = matches!(params.direction, SwapDirection::ZeroForOne);
        let (sqrt_q, used_in, got_out, fee_amt) =
            compute_swap_step(&sqrt_price, &sqrt_target_bound, &liquidity, &amount_remaining, eff_fee_ppm, zero_for_one);

        if zero_for_one {
            amount0_total -= &used_in + &fee_amt; // spent token0 (gross)
            amount1_total += &got_out;            // received token1
        } else {
            amount1_total -= &used_in + &fee_amt; // spent token1 (gross)
            amount0_total += &got_out;            // received token0
        }
        amount_remaining -= &used_in + &fee_amt;
        sqrt_price = sqrt_q;

        let crossed = has_next && sqrt_price == sqrt_next;
        if crossed {
            ticks_crossed += 1;
            if let Some(ti) = pool.ticks.get(&next_tick) {
                match params.direction {
                    SwapDirection::ZeroForOne => {
                        // moving left: liquidity -= liquidityNet
                        if ti.liquidity_net.is_negative() { liquidity += -ti.liquidity_net.clone() }
                        else                               { liquidity -= ti.liquidity_net.clone() }
                    }
                    SwapDirection::OneForZero => {
                        // moving right: liquidity += liquidityNet
                        liquidity += ti.liquidity_net.clone();
                    }
                }
            }
            current_tick = match params.direction {
                SwapDirection::ZeroForOne => next_tick - 1,
                SwapDirection::OneForZero => next_tick,
            };
        } else {
            current_tick = get_tick_at_sqrt_ratio(&sqrt_price);
            break;
        }
    }

    Ok(SwapResult {
        amount0: amount0_total,
        amount1: amount1_total,
        sqrt_price_x96: sqrt_price,
        tick: current_tick,
        liquidity,
        crossed_ticks: ticks_crossed,
    })
}

// ----------------------- Convenience + Best-pool picker ----------------------

pub fn simulate_exact_in_tokens(
    pool: &PoolState,
    direction: SwapDirection,
    fee_ppm_override: Option<u32>,
    amount_in_tokens: f64,
    input_decimals: u8,
    price_limit: Option<BigInt>,
) -> Result<SwapResult, String> {
    let scale = 10f64.powi(input_decimals as i32);
    let amount_units = (amount_in_tokens * scale).round();
    if amount_units < 0.0 { return Err("amount_in_tokens must be >= 0".into()); }
    let amount_bi = BigInt::from(amount_units as i128);

    let limit = price_limit.unwrap_or_else(|| default_limit(direction));

    let fee = fee_ppm_override.unwrap_or(pool.key.fee_ppm);
    let params = SwapParams {
        direction,
        amount_specified: amount_bi,
        sqrt_price_limit_x96: limit,
        fee_ppm: fee,
    };
    simulate_swap(pool, &params, &NoHook)
}

#[allow(dead_code)]
pub fn execution_price_out_per_in(
    res: &SwapResult,
    direction: SwapDirection,
    in_decimals: u8,
    out_decimals: u8,
) -> f64 {
    let si = 10f64.powi(in_decimals as i32);
    let so = 10f64.powi(out_decimals as i32);
    match direction {
        SwapDirection::ZeroForOne => {
            let input  = (-res.amount0.clone()).to_f64().unwrap_or(0.0) / si;
            let output = ( res.amount1.clone()).to_f64().unwrap_or(0.0) / so;
            if input <= 0.0 { 0.0 } else { output / input }
        }
        SwapDirection::OneForZero => {
            let input  = (-res.amount1.clone()).to_f64().unwrap_or(0.0) / si;
            let output = ( res.amount0.clone()).to_f64().unwrap_or(0.0) / so;
            if input <= 0.0 { 0.0 } else { output / input }
        }
    }
}

#[allow(dead_code)]
pub fn best_pool_for_exact_in(
    pools: &[PoolState],
    direction: SwapDirection,
    amount_in_tokens: f64,
    in_decimals: u8,
    out_decimals: u8,
    fee_ppm_override: Option<u32>,
) -> Option<(PoolState, SwapResult, f64)> {
    let mut best: Option<(PoolState, SwapResult, f64)> = None;
    for p in pools {
        let fee = fee_ppm_override.unwrap_or(p.key.fee_ppm);
        let sim = simulate_exact_in_tokens(p, direction, Some(fee), amount_in_tokens, in_decimals, None).ok()?;
        let px  = execution_price_out_per_in(&sim, direction, in_decimals, out_decimals);
        match &best {
            Some((_, _, best_px)) if px <= *best_px => {}
            _ => best = Some((p.clone(), sim, px)),
        }
    }
    best
}

// --- helper: f64 -> token units ---
#[allow(dead_code)]
fn units(amount: f64, decimals: u32) -> BigInt {
    let scale = 10f64.powi(decimals as i32);
    BigInt::from((amount * scale).round() as i128)
}

// --- compute L from reserves & range ---
#[allow(dead_code)]
fn liquidity_from_reserves(
    sa: &BigInt, sp: &BigInt, sb: &BigInt,
    amount0: &BigInt, amount1: &BigInt, // raw token units (wei / 6-dec units)
) -> BigInt {
    assert!(sa < sp && sp < sb, "expect sa < sp < sb");

    // L0 = amount0 * (sb * sp) / ( (sb - sp) << 96 )
    let sb_sp = sb * sp;
    let sb_minus_sp = sb - sp;
    let q96 = BigInt::one() << 96;
    let denom0: BigInt = &sb_minus_sp * &q96; // (sb - sp) * 2^96
    let l0 = if denom0.is_zero() { BigInt::zero() } else { (amount0 * sb_sp) / denom0 };

    // L1 = amount1 * Q96 / (sp - sa)
    let sp_minus_sa = sp - sa;
    let l1 = if sp_minus_sa.is_zero() { BigInt::zero() } else { (amount1 * &q96) / sp_minus_sa };

    // Use the limiting side so both tokens are respected
    if l0 < l1 { l0 } else { l1 }
}

// Helper functions for integration compatibility
#[allow(dead_code)]
pub fn create_standard_weth_usdc_pools(
    weth_address: Address,
    usdc_address: Address,
    initial_price: f64,
) -> Result<Vec<PoolState>, String> {
    let pools = vec![
        mock_pool_with_addresses(weth_address, usdc_address, initial_price,  500, 10),   // 0.05%
        mock_pool_with_addresses(weth_address, usdc_address, initial_price, 3000, 60),  // 0.30%
        mock_pool_with_addresses(weth_address, usdc_address, initial_price, 10000, 200), // 1.00%
    ];
    Ok(pools)
}

// Create pool with real on-chain data
pub fn create_pool_with_real_data(
    currency0: Address,
    currency1: Address,
    fee_ppm: u32,
    tick_spacing: i32,
    hooks: Address,
    sqrt_price_x96: BigInt,
    current_tick: i32,
    liquidity: BigInt,
    tick_data: Vec<(i32, BigInt)>, // Vec of (tick, liquidityNet)
) -> PoolState {
    let mut ticks = BTreeMap::new();
    
    // Add all provided tick data
    for (tick, liquidity_net) in tick_data {
        ticks.insert(tick, TickInfo {
            tick,
            liquidity_net,
        });
    }
    
    PoolState {
        key: PoolKey {
            currency0,
            currency1,
            fee_ppm,
            tick_spacing,
            hooks,
        },
        sqrt_price_x96,
        tick: current_tick,
        liquidity,
        ticks,
    }
}

// --- new pool builder that sets L correctly ---
#[allow(dead_code)]
fn mock_pool_with_addresses(
    currency0: Address,
    currency1: Address,
    price_token1_per_token0: f64, // USDC per WETH
    fee_ppm: u32,
    tick_spacing: i32,
) -> PoolState {
    // 1) center at current price & pick a sensible width per fee tier
    let center_tick = tick_from_price(price_token1_per_token0, 18, 6);
    let width = match fee_ppm {
        500   => 3000,  // tighter band for low fee
        3000  => 6000,
        10000 => 10000,
        _     => 6000,
    };
    let lower_tick = ((center_tick - width) / tick_spacing) * tick_spacing;
    let upper_tick = ((center_tick + width) / tick_spacing) * tick_spacing;

    // 2) choose intended reserves for this pool (you can tune these)
    //    token0=WETH(18), token1=USDC(6)
    let (amount0_token0, amount1_token1) = match fee_ppm {
        500   => (units(200.0, 18), units(600_000.0, 6)),   // 200 WETH, 600k USDC
        3000  => (units(100.0, 18), units(300_000.0, 6)),   // 100 WETH, 300k USDC
        10000 => (units( 50.0, 18), units(150_000.0, 6)),   //  50 WETH, 150k USDC
        _     => (units(100.0, 18), units(300_000.0, 6)),
    };

    // 3) compute sqrt prices (Q64.96)
    let sa = get_sqrt_ratio_at_tick(lower_tick);
    let sp = get_sqrt_ratio_at_tick(center_tick);
    let sb = get_sqrt_ratio_at_tick(upper_tick);

    // 4) compute concentrated liquidity for this price range
    let concentrated_liquidity = liquidity_from_reserves(&sa, &sp, &sb, &amount0_token0, &amount1_token1);

    // 5) materialize ticks and pool state
    let mut ticks = BTreeMap::new();
    ticks.insert(lower_tick, TickInfo { tick: lower_tick, liquidity_net: concentrated_liquidity.clone() });
    ticks.insert(upper_tick, TickInfo { tick: upper_tick, liquidity_net: -concentrated_liquidity.clone() });

    PoolState {
        key: PoolKey { currency0, currency1, fee_ppm, tick_spacing, hooks: Address::zero() },
        sqrt_price_x96: sp,
        tick: center_tick,
        liquidity: concentrated_liquidity,
        ticks,
    }
}

// Backward compatibility functions
#[allow(dead_code)]
pub fn tick_from_price(price_1_for_0: f64, dec0: u8, dec1: u8) -> i32 {
    // price_1_for_0 is the human price: token1 per 1 token0
    let price_raw = price_1_for_0 * 10f64.powi(dec1 as i32 - dec0 as i32); // scale to raw units
    let t = (price_raw.ln() / 1.0001f64.ln()).floor() as i32;
    t.clamp(MIN_TICK, MAX_TICK)
}

#[allow(dead_code)]
pub fn price_from_tick(tick: i32, dec0: u8, dec1: u8) -> f64 {
    // returns the human price: token1 per 1 token0
    let s = get_sqrt_ratio_at_tick(tick).to_f64().unwrap_or(0.0) / ((1u128 << 96) as f64);
    let price_raw = s * s;
    price_raw * 10f64.powi(dec0 as i32 - dec1 as i32)
}

// ------------------------------- Minimal tests -------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(x: u8) -> Address { Address::from([x;20]) }

    fn mock_pool(price_token1_per_token0: f64, fee_ppm: u32, tick_spacing: i32) -> PoolState {
        mock_pool_with_addresses(addr(1), addr(2), price_token1_per_token0, fee_ppm, tick_spacing)
    }

    #[test]
    fn debug_scaling_issue() {
        // Test with minimal amounts to understand scaling
        let liquidity = BigInt::from(1000000000000000000000u128); // 1000 in 18-decimal units
        let sqrt_price_a = BigInt::from(4339357908326790765990283501801u128); // ~3000 price
        let sqrt_price_b = BigInt::from(4315791062650166323685528455962u128); // slightly lower
        
        log::debug!("=== DEBUGGING AMOUNT DELTA SCALING ===");
        log::debug!("Liquidity: {} (represents {} in human terms)", liquidity, liquidity.clone().to_f64().unwrap_or(0.0) / 1e18);
        log::debug!("Price A (Q64.96): {} = sqrt ratio {:.6}", sqrt_price_a, sqrt_price_a.clone().to_f64().unwrap_or(0.0) / (1u128 << 96) as f64);
        log::debug!("Price B (Q64.96): {} = sqrt ratio {:.6}", sqrt_price_b, sqrt_price_b.clone().to_f64().unwrap_or(0.0) / (1u128 << 96) as f64);
        
        let amount0 = amount0_delta(&sqrt_price_a, &sqrt_price_b, &liquidity, true);
        let amount1 = amount1_delta(&sqrt_price_a, &sqrt_price_b, &liquidity, false);
        
        log::debug!("Amount0 delta: {} wei = {:.6} WETH", amount0, amount0.clone().to_f64().unwrap_or(0.0) / 1e18);
        log::debug!("Amount1 delta: {} wei = {:.6} USDC", amount1, amount1.clone().to_f64().unwrap_or(0.0) / 1e6);
        
        // What should the ratio be? If we swap amount0 WETH at ~3000 price:
        let expected_usdc = amount0.clone().to_f64().unwrap_or(0.0) / 1e18 * 3000.0;
        let actual_usdc = amount1.clone().to_f64().unwrap_or(0.0) / 1e6;
        log::debug!("Expected USDC at 3000 price: {:.2}", expected_usdc);  
        log::debug!("Actual USDC from delta: {:.2}", actual_usdc);
        log::debug!("Ratio (actual/expected): {:.2e}", actual_usdc / expected_usdc);
        
        // Test individual components
        log::debug!("\n=== COMPONENT ANALYSIS ===");
        let sa = &sqrt_price_a;
        let sb = &sqrt_price_b;
        let l = &liquidity;
        let q96 = bu128(1u128 << 96);
        
        // amount1_delta formula: L * (sb - sa) / Q96
        let numerator = l * (sb - sa);
        let result = &numerator / &q96;
        log::debug!("Manual amount1_delta calculation:");
        log::debug!("  L * (sb - sa) = {}", numerator);
        log::debug!("  Divided by Q96 = {}", result);
        log::debug!("  In USDC tokens = {:.6}", result.to_f64().unwrap_or(0.0) / 1e6);
    }

    #[test]
    fn basic_swap_zero_for_one() {
        let p = mock_pool(3000.0, 3000, 60);
        
        log::debug!("Pool state:");
        log::debug!("  sqrt_price_x96: {}", p.sqrt_price_x96);
        log::debug!("  tick: {}", p.tick);
        log::debug!("  liquidity: {}", p.liquidity);
        
        let requested_amount = BigInt::from(1e18 as i128);
        log::debug!("  requested amount: {} wei = 1.0 ETH", requested_amount);
        
        let res = simulate_exact_in_tokens(&p, SwapDirection::ZeroForOne, None, 1.0, 18, None).unwrap();
        let px  = execution_price_out_per_in(&res, SwapDirection::ZeroForOne, 18, 6);
        
        log::debug!("Swap result:");
        log::debug!("  Raw amounts - amount0: {}, amount1: {}", res.amount0, res.amount1);
        log::debug!("  Input ETH: {:.6}, Output USDC: {:.2}", 
                 (-res.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18,
                 res.amount1.clone().to_f64().unwrap_or(0.0) / 1e6);
        log::debug!("  Final sqrt_price: {}", res.sqrt_price_x96);
        log::debug!("  Ticks crossed: {}", res.crossed_ticks);
        
        assert!(res.amount0 < BigInt::zero() && res.amount1 > BigInt::zero());
        assert!(px > 2800.0 && px < 3200.0, "px {}", px);
    }

    #[test]
    fn best_pool_pick() {
        let p1 = mock_pool(3000.0,  500, 10);  
        let p2 = mock_pool(3000.0, 3000, 60);  
        let p3 = mock_pool(3000.0,10000, 200);
        let pools = vec![p1,p2,p3];
        let (pool, res, px) = best_pool_for_exact_in(&pools, SwapDirection::ZeroForOne, 1.0, 18, 6, None).unwrap();
        assert!(res.amount0 < BigInt::zero() && res.amount1 > BigInt::zero());
        assert!(px > 2800.0 && px < 3200.0, "px {}", px);
        // lower fee tier usually wins at small size
        assert_eq!(pool.key.fee_ppm, 500);
    }
}