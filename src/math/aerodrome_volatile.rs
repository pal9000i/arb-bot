// Aerodrome (Solidly/Velodrome) — Volatile Pool Math Module
// ----------------------------------------------------------
// Focus: exact constant-product (x*y=k) swap math with fee, using integer arithmetic.
// This simulates the pool's getAmountOut for volatile pools, off-chain, without RPC calls.
// It is optimized for WETH/USDC, but works for any 2 tokens given reserves/decimals/fee.
//
// Cargo.toml deps (examples):
//   ethers = { version = "2", default-features = false, features = ["core"] }
//   num-traits = "0.2"
//   serde = { version = "1", features = ["derive"], optional = true }
//
// Notes:
// - All internal math is done in raw token units (integers). No floating point until reporting.
// - Fee is expressed in basis points (bps). γ = (10_000 - fee_bps) / 10_000.
// - We never assume token order. Direction is explicit and reserves are mapped accordingly.
// - For price/impact reporting we normalize by the respective token decimals.
//
// If you later want to validate parity with chain, do one staticcall to
// pair.getAmountOut(amountIn, tokenIn) right before execution (good hygiene).

use std::cmp::min;
use ethers::types::{Address, U256};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// ------------------------------- Data types ----------------------------------

/// Swap direction: ZeroForOne means token0 -> token1 input; OneForZero = token1 -> token0 input.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwapDirection { ZeroForOne, OneForZero }

/// Minimal volatile-pool state snapshot (what you need to simulate).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct VolatilePairState {
    pub token0: Address,
    pub token1: Address,
    pub reserve0: U256,        // raw units
    pub reserve1: U256,        // raw units
    pub decimals0: u8,         // e.g., WETH=18
    pub decimals1: u8,         // e.g., USDC=6
    pub fee_bps: u32,          // e.g., 5 for 0.05%
}

impl VolatilePairState {
    #[inline]
    #[allow(dead_code)]
    pub fn is_zero_liquidity(&self) -> bool {
        self.reserve0.is_zero() || self.reserve1.is_zero()
    }
}

// ------------------------------- Core math -----------------------------------

/// Exact constant-product with fee (volatile pool).
/// amountIn, reserveIn, reserveOut are raw token units. Returns raw amountOut.
/// Formula: out = ( (in * γ) * R_out ) / ( R_in + (in * γ) )
#[inline]
pub fn volatile_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    fee_bps: u32,
) -> U256 {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() {
        return U256::zero();
    }
    let fee_bps = min(fee_bps, 9_999); // avoid γ=0 edge
    let num = U256::from(10_000u32 - fee_bps);
    let den = U256::from(10_000u32);

    let amount_in_after_fee = amount_in * num / den;
    // out = (amount_in' * R_out) / (R_in + amount_in')
    (amount_in_after_fee * reserve_out) / (reserve_in + amount_in_after_fee)
}

/// Map direction to (reserve_in, reserve_out) and decimals for reporting.
#[inline]
pub fn map_direction<'a>(
    pair: &'a VolatilePairState,
    direction: SwapDirection,
) -> (U256, U256, u8, u8) {
    match direction {
        SwapDirection::ZeroForOne => (pair.reserve0, pair.reserve1, pair.decimals0, pair.decimals1),
        SwapDirection::OneForZero => (pair.reserve1, pair.reserve0, pair.decimals1, pair.decimals0),
    }
}

// --------------------------- Price & conversions -----------------------------

/// Spot price (tokenOut per tokenIn), normalized by decimals.
#[inline]
pub fn spot_price_out_per_in(
    reserve_in: U256,
    reserve_out: U256,
    dec_in: u8,
    dec_out: u8,
) -> f64 {
    if reserve_in.is_zero() { return 0.0; }
    // price = (R_out / 10^d_out) / (R_in / 10^d_in)
    //       = (R_out * 10^d_in) / (R_in * 10^d_out)
    let pow_in  = pow10(dec_in);
    let pow_out = pow10(dec_out);
    let num = mul_div_to_f64(reserve_out, pow_in);
    let den = mul_div_to_f64(reserve_in, pow_out);
    if den == 0.0 { 0.0 } else { num / den }
}

/// Convert human -> raw integer units (rounded).
#[inline]
pub fn to_raw(amount_human: f64, decimals: u8) -> U256 {
    if amount_human <= 0.0 { return U256::zero(); }
    let scale = 10f64.powi(decimals as i32);
    let v = (amount_human * scale).round();
    if v <= 0.0 { U256::zero() } else { U256::from(v as u128) }
}

/// Convert raw integer units -> human f64.
#[inline]
pub fn from_raw(amount: U256, decimals: u8) -> f64 {
    let s = 10f64.powi(decimals as i32);
    (amount.as_u128() as f64) / s
}

#[inline]
fn pow10(dec: u8) -> U256 {
    // safe for dec<=77 with U256, but ERC20 decimals are typically <= 18.
    U256::from(10u8).pow(U256::from(dec))
}

#[inline]
fn mul_div_to_f64(a: U256, b: U256) -> f64 {
    // Convert smallish U256 products to f64 for reporting only.
    // For ERC20 reserves this is safe for WETH/USDC scales; if you expect giant pools,
    // switch reporting to decimal/bigdecimal to avoid precision loss.
    let _ah = (a >> 128).low_u128();
    let _al = a.low_u128();
    let _bh = (b >> 128).low_u128();
    let _bl = b.low_u128();
    // naive: (a*b) ~ (ah<<128 + al) * (bh<<128 + bl) — we only need f64, so simplify:
    let approx = (a.as_u128() as f64) * (b.as_u128() as f64);
    approx
}

// ----------------------------- Public simulator ------------------------------

/// Simulate exact-input swap on a volatile pool.
/// Returns (amount_in_raw, amount_out_raw, effective_price_out_per_in, spot_price, price_impact_pct).
pub fn simulate_exact_in_volatile(
    pair: &VolatilePairState,
    direction: SwapDirection,
    amount_in_human: f64,
) -> (U256, U256, f64, f64, f64) {
    let (reserve_in, reserve_out, dec_in, dec_out) = map_direction(pair, direction);
    let amount_in_raw = to_raw(amount_in_human, dec_in);
    let amount_out_raw = volatile_amount_out(amount_in_raw, reserve_in, reserve_out, pair.fee_bps);

    // reporting
    let in_h  = from_raw(amount_in_raw, dec_in);
    let out_h = from_raw(amount_out_raw, dec_out);
    let eff   = if in_h <= 0.0 { 0.0 } else { out_h / in_h };
    let spot  = spot_price_out_per_in(reserve_in, reserve_out, dec_in, dec_out);
    let impact_pct = if spot > 0.0 { (eff / spot - 1.0) * 100.0 } else { 0.0 };

    (amount_in_raw, amount_out_raw, eff, spot, impact_pct)
}

/// Convenience: compute execution price only (tokenOut per tokenIn, human units).
#[inline]
#[allow(dead_code)]
pub fn execution_price_out_per_in(
    pair: &VolatilePairState,
    direction: SwapDirection,
    amount_in_human: f64,
) -> f64 {
    let (_, _, eff, _, _) = simulate_exact_in_volatile(pair, direction, amount_in_human);
    eff
}

// ------------------------------ Sanity helpers -------------------------------

/// Update reserves after a hypothetical swap (pure math), useful for iterative sims.
/// NOTE: This models *volatile* math only. Not accounting for tax tokens, etc.
#[allow(dead_code)]
pub fn apply_swap_to_reserves(
    pair: &VolatilePairState,
    direction: SwapDirection,
    amount_in_raw: U256,
) -> (U256, U256) {
    let (rin, rout, _, _) = map_direction(pair, direction);
    let out = volatile_amount_out(amount_in_raw, rin, rout, pair.fee_bps);
    let fee_num = U256::from(10_000u32 - min(pair.fee_bps, 9_999));
    let fee_den = U256::from(10_000u32);
    let in_after_fee = amount_in_raw * fee_num / fee_den;

    match direction {
        SwapDirection::ZeroForOne => {
            // token0 in, token1 out
            let new_r0 = rin + in_after_fee; // reserve_in increases by net input
            let new_r1 = rout - out;         // reserve_out decreases by output
            (new_r0, new_r1)
        }
        SwapDirection::OneForZero => {
            let new_r1 = rin + in_after_fee;
            let new_r0 = rout - out;
            (new_r0, new_r1)
        }
    }
}

// ---------------------------------- Tests ------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    fn addr(x: u8) -> Address { Address::from([x; 20]) }

    fn mock_weth_usdc_pool(_price_usdc_per_weth: f64, weth: f64, usdc: f64, fee_bps: u32) -> VolatilePairState {
        // token0=WETH(18), token1=USDC(6) layout (common but not assumed by math)
        // Choose reserves roughly consistent with target price for sanity.
        let decimals0 = 18u8;
        let decimals1 = 6u8;

        let r0 = to_raw(weth, decimals0);
        let r1 = to_raw(usdc, decimals1);

        // sanity: price from reserves (approx)
        let spot = spot_price_out_per_in(r0, r1, decimals0, decimals1);
        assert!(spot > 0.0, "spot must be positive");

        VolatilePairState {
            token0: addr(0x11),
            token1: addr(0x22),
            reserve0: r0,
            reserve1: r1,
            decimals0,
            decimals1,
            fee_bps,
        }
    }

    #[test]
    fn volatile_amount_out_monotonicity() {
        let pair = mock_weth_usdc_pool(3000.0, 5_000.0, 15_000_000.0, 5); // 5 bps
        let a1 = to_raw(0.1, pair.decimals0);
        let a2 = to_raw(0.5, pair.decimals0);
        let a3 = to_raw(1.0, pair.decimals0);

        let (r_in, r_out, _, _) = map_direction(&pair, SwapDirection::ZeroForOne);
        let o1 = volatile_amount_out(a1, r_in, r_out, pair.fee_bps);
        let o2 = volatile_amount_out(a2, r_in, r_out, pair.fee_bps);
        let o3 = volatile_amount_out(a3, r_in, r_out, pair.fee_bps);

        assert!(o1 < o2 && o2 < o3, "output should increase with input");
    }

    #[test]
    fn execution_price_reasonable() {
        let pair = mock_weth_usdc_pool(3000.0, 5_000.0, 15_000_000.0, 5);
        let px_small = execution_price_out_per_in(&pair, SwapDirection::ZeroForOne, 0.1);
        let spot     = spot_price_out_per_in(pair.reserve0, pair.reserve1, pair.decimals0, pair.decimals1);
        assert!(px_small > 0.0 && spot > 0.0);
        // at small size, effective should be close to spot but slightly worse
        assert!(px_small < spot * 1.001);
    }

    #[test]
    fn price_impact_sign_and_magnitude() {
        let pair = mock_weth_usdc_pool(3000.0, 10_000.0, 30_000_000.0, 5);
        let (_, _, eff_small, spot, imp_small) = simulate_exact_in_volatile(&pair, SwapDirection::ZeroForOne, 0.1);
        let (_, _, eff_big,   _,   imp_big  ) = simulate_exact_in_volatile(&pair, SwapDirection::ZeroForOne, 50.0);

        assert!(eff_small < spot && imp_small < 0.0, "buying out of pool worsens price");
        assert!(eff_big   < eff_small, "bigger trade => worse execution");
        assert!(imp_big < imp_small, "impact becomes more negative with size");
    }

    #[test]
    fn reserves_update_consistency() {
        let mut pair = mock_weth_usdc_pool(3000.0, 5_000.0, 15_000_000.0, 5);
        let amount_in = to_raw(10.0, pair.decimals0);
        let (out_in_raw, out_raw, ..) = simulate_exact_in_volatile(&pair, SwapDirection::ZeroForOne, 10.0);
        assert_eq!(out_in_raw, amount_in);

        let (new_r0, new_r1) = apply_swap_to_reserves(&pair, SwapDirection::ZeroForOne, amount_in);
        // reflect update with correct mapping
        pair.reserve0 = new_r0;
        pair.reserve1 = new_r1;

        // Next small trade should execute slightly worse after reserves move
        let px_before = execution_price_out_per_in(&mock_weth_usdc_pool(3000.0, 5_000.0, 15_000_000.0, 5),
                                                  SwapDirection::ZeroForOne, 1.0);
        let px_after  = execution_price_out_per_in(&pair, SwapDirection::ZeroForOne, 1.0);
        assert!(px_after < px_before);
        // out must be positive
        assert!(!out_raw.is_zero());
    }
}