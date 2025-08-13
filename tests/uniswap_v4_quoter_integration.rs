// tests/uniswap_v4_quoter_simulator_accurate.rs

use arrakis_arbitrage::math::uniswap_v4::{
  create_pool_with_real_data, simulate_exact_in_tokens, SwapDirection, PoolState,
};
use ethers::prelude::*;
use ethers::types::U256;
use ethers::abi::Token;
use num_bigint::BigInt;
use num_traits::{Zero, ToPrimitive};
use std::str::FromStr;
use std::sync::Arc;

// ======================= ABIs =======================

// QuoterV4 (mainnet)
abigen!(
  QuoterV4,
  "./abis/UniswapV4Quoter.json"
);

// StateView (read-only V4 pool state)
abigen!(
  StateView,
  "./abis/UniswapV4StateView.json"
);

// ======================= Constants =======================

// Mainnet quoter (public)
const QUOTER_V4: &str = "0x52F0E24D1c21C8A0cB1e5a5dD6198556BD9E1203";
// USDC mainnet
const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
// We’ll test the 0.05% tier; adjust if your deployment/liquidity differs.
const FEE_PPM: u32 = 500;   // 0.05%
const TICK_SPACING: i32 = 10;
const HOOKS: &str = "0x0000000000000000000000000000000000000000";

// How wide (in bitmap words) to scan for real initialized ticks around current tick
const BITMAP_WORD_SCAN: i16 = 64; // 64 words each side (≈ 64 * 256 * tickSpacing ticks)

// Error tolerance (relative)
const MAX_PCT_ERR: f64 = 1.0; // 1%

// ======================= Utilities =======================

fn compute_pool_id(
  currency0: Address,
  currency1: Address,
  fee_ppm: u32,
  tick_spacing: i32,
  hooks: Address,
) -> [u8; 32] {
  use ethers::utils::keccak256;

  // v4 poolId = keccak256(abi.encode(currency0, currency1, fee, tickSpacing, hooks))
  let encoded = ethers::abi::encode(&[
      Token::Address(currency0),
      Token::Address(currency1),
      Token::Uint(U256::from(fee_ppm)),            // uint24
      Token::Int(U256::from(tick_spacing as u32)),        // int24 (signed!) ✅
      Token::Address(hooks),
  ]);
  keccak256(encoded)
}

fn order_currencies(eth: Address, usdc: Address) -> (Address, Address, bool) {
  // currency0 < currency1 by address ordering
  if eth < usdc {
      (eth, usdc, true) // token0 is ETH
  } else {
      (usdc, eth, false) // token0 is USDC
  }
}

fn u256_to_bigint(v: U256) -> BigInt {
  let mut buf = [0u8; 32];
  v.to_big_endian(&mut buf);
  BigInt::from_bytes_be(num_bigint::Sign::Plus, &buf)
}

// For printing human-readable numbers
trait U256F64 {
  fn to_f64_lossy(&self) -> f64;
}
impl U256F64 for U256 {
  fn to_f64_lossy(&self) -> f64 {
      self.to_string().parse::<f64>().unwrap_or(0.0)
  }
}

// ======================= Tick Reconstruction =======================

async fn fetch_real_ticks_around(
  state_view: &StateView<Provider<Http>>,
  pool_id: [u8; 32],
  current_tick: i32,
  tick_spacing: i32,
) -> Result<Vec<(i32, BigInt)>, Box<dyn std::error::Error + Send + Sync>> {
  let mut ticks: Vec<(i32, BigInt)> = Vec::new();

  // Word index (each word covers 256 initialized ticks)
  let current_word = (current_tick / tick_spacing) >> 8; // divide by 256

  for word_offset in -BITMAP_WORD_SCAN..=BITMAP_WORD_SCAN {
      let word_pos = (current_word as i16).saturating_add(word_offset);
      // Pull bitmap word
      let word = match state_view.get_tick_bitmap(pool_id, word_pos).call().await {
          Ok(w) => w,
          Err(_) => continue,
      };
      if word.is_zero() {
          continue;
      }
      // Scan bits
      for bit in 0..256u32 {
          if word.bit(bit as usize) {
              // This bit corresponds to an initialized tick index
              let tick_index = (word_pos as i32) * 256 + (bit as i32);
              let tick = tick_index * tick_spacing;

              // Pull tick info to get liquidityNet (signed)
              if let Ok((liq_gross, liq_net, _fg0, _fg1)) =
                  state_view.get_tick_info(pool_id, tick).call().await
              {
                  if !liq_gross.is_zero() {
                      // Convert liq_net (int128) to BigInt (keep sign)
                      let net = {
                          // ethers::types::I256 can convert from i128 safely, but here ABI gave int128 into U256/I256?
                          // We get it already as I256 via abigen; convert by to_string.
                          let s = liq_net.to_string(); // preserve sign
                          BigInt::parse_bytes(s.as_bytes(), 10).unwrap_or(BigInt::zero())
                      };
                      if !net.is_zero() {
                          ticks.push((tick, net));
                      }
                  }
              }
          }
      }
  }

  // If none found, return an error (we want an accurate comparison)
  if ticks.is_empty() {
      return Err("No initialized ticks discovered in bitmap scan".into());
  }

  // Sort by tick ascending (good practice for your simulator)
  ticks.sort_by_key(|(t, _)| *t);
  Ok(ticks)
}

// ======================= Main test =======================

#[tokio::test]
async fn test_uniswap_v4_quoter_vs_simulator_eth_usdc_both_directions() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  dotenv::dotenv().ok();

  // --- Environment guards
  let rpc_url = match std::env::var("ETHEREUM_RPC_URL") {
      Ok(v) => v,
      Err(_) => {
          eprintln!("⚠️  ETHEREUM_RPC_URL not set; skipping test.");
          return Ok(());
      }
  };
  let state_view_addr = match std::env::var("UNISWAP_V4_STATE_VIEW") {
      Ok(v) => v,
      Err(_) => {
          eprintln!("⚠️  UNISWAP_V4_STATE_VIEW not set; skipping test.");
          return Ok(());
      }
  };

  // --- Setup clients
  let provider = Arc::new(Provider::<Http>::try_from(rpc_url)?);
  let state_view = StateView::new(Address::from_str(&state_view_addr)?, provider.clone());
  let quoter = QuoterV4::new(Address::from_str(QUOTER_V4)?, provider.clone());

  // --- Key construction (ETH is address(0) in v4 pool keys)
  let eth = Address::zero();
  let usdc = Address::from_str(USDC)?;
  let (currency0, currency1, token0_is_eth) = order_currencies(eth, usdc);
  let fee_ppm = FEE_PPM;
  let tick_spacing = TICK_SPACING;
  let hooks = Address::from_str(HOOKS)?;

  // --- poolId
  let pool_id = compute_pool_id(currency0, currency1, fee_ppm, tick_spacing, hooks);
  println!("🧮 poolId = 0x{}", hex::encode(pool_id));

  // --- slot0 + liquidity
  let (sqrt_price_x96_u160, current_tick, protocol_fee, lp_fee) = state_view.get_slot_0(pool_id).call().await?;
  let liquidity_u128 = state_view.get_liquidity(pool_id).call().await?;
  if liquidity_u128.is_zero() {
      eprintln!("⚠️  Pool has zero liquidity for the given key; skipping test.");
      return Ok(());
  }

  // Convert to BigInt
  let sqrt_price_x96 = u256_to_bigint(U256::from(sqrt_price_x96_u160));
  let liquidity_bi = u256_to_bigint(U256::from(liquidity_u128));

  println!("📊 slot0:");
  println!("  tick         = {}", current_tick);
  println!("  sqrtPriceX96 = {}", sqrt_price_x96);
  println!("  protocolFee  = {}", protocol_fee);
  println!("  lpFee        = {}", lp_fee);
  println!("  liquidity    = {}", liquidity_bi);

  // --- Pull real initialized ticks via bitmap
  let ticks = match fetch_real_ticks_around(&state_view, pool_id, current_tick, tick_spacing).await {
      Ok(v) => v,
      Err(e) => {
          eprintln!("⚠️  Unable to reconstruct real ticks: {e}. Skipping for accuracy.");
          return Ok(());
      }
  };
  println!("✅ Discovered {} initialized ticks", ticks.len());

  // --- Build local pool
  let pool: PoolState = create_pool_with_real_data(
      currency0,
      currency1,
      fee_ppm,
      tick_spacing,
      hooks,
      sqrt_price_x96.clone(),
      current_tick,
      liquidity_bi.clone(),
      ticks, // full real tick table
  );

  // ================= DIR 1: ETH -> USDC (zeroForOne if token0 is ETH) =================
  {
      let zero_for_one = token0_is_eth; // currency0==ETH -> currency1==USDC
      let direction = if zero_for_one { SwapDirection::ZeroForOne } else { SwapDirection::OneForZero };

      // Quoter for exact-in: 1 ETH
      let amount_in_wei = U256::exp10(18); // 1e18
      let params = (
          (currency0, currency1, fee_ppm, tick_spacing, hooks),
          zero_for_one,
          amount_in_wei.as_u128(), // uint128
          Bytes::new(),
      );

      let (amount_out_quoter_u256, _gas_est) = quoter.quote_exact_input_single(params).call().await?;
      // USDC out if ETH->USDC
      let out_quoter = if zero_for_one {
          amount_out_quoter_u256.to_f64_lossy() / 1e6
      } else {
          amount_out_quoter_u256.to_f64_lossy() / 1e18
      };

      // Local simulator
      let sim = simulate_exact_in_tokens(&pool, direction, Some(fee_ppm), 1.0, 18, None)
          .expect("simulate_exact_in_tokens failed");
      let out_sim = if zero_for_one {
          sim.amount1.to_f64().unwrap_or(0.0) / 1e6
      } else {
          sim.amount0.to_f64().unwrap_or(0.0) / 1e18
      };

      let denom = out_quoter.max(1e-12);
      let pct_err = ((out_sim - out_quoter).abs() / denom) * 100.0;

      println!("\n=== ETH -> USDC ===");
      println!("Quoter:    {:.6} USDC", out_quoter);
      println!("Simulator: {:.6} USDC", out_sim);
      println!("Error:     {:.6} USDC | {:.3}%", (out_sim - out_quoter).abs(), pct_err);

      assert!(
          pct_err <= MAX_PCT_ERR,
          "ETH->USDC simulator error too high: {:.3}% (max {:.3}%)",
          pct_err, MAX_PCT_ERR
      );
  }

  // ================= DIR 2: USDC -> ETH (oneForZero if token0 is ETH) =================
  {
      let zero_for_one = token0_is_eth; // ETH->USDC flag from above
      let direction = if zero_for_one { SwapDirection::OneForZero } else { SwapDirection::ZeroForOne };

      // Quoter for exact-in: 10,000 USDC (a decent size)
      let usdc_in = 10_000.0_f64;
      let amount_in_usdc = U256::from((usdc_in * 1e6) as u128); // uint128 fits easily here
      let params = (
          (currency0, currency1, fee_ppm, tick_spacing, hooks),
          false, // USDC -> ETH means currency1->currency0 if token0 is ETH; i.e., zeroForOne = false when token0_is_eth
          amount_in_usdc.as_u128(),
          Bytes::new(),
      );

      let (amount_out_quoter_u256, _gas_est) = quoter.quote_exact_input_single(params).call().await?;
      // ETH out when USDC->ETH
      let out_quoter_eth = amount_out_quoter_u256.to_f64_lossy() / 1e18;

      // Simulator: input token has 6 decimals (USDC)
      let sim = simulate_exact_in_tokens(&pool, direction, Some(fee_ppm), usdc_in, 6, None)
          .expect("simulate_exact_in_tokens failed");
      let out_sim_eth = if zero_for_one {
          // oneForZero -> amount0 (ETH) is positive out
          sim.amount0.to_f64().unwrap_or(0.0) / 1e18
      } else {
          // zeroForOne -> amount1 (ETH) is positive out
          sim.amount1.to_f64().unwrap_or(0.0) / 1e18
      };

      let denom = out_quoter_eth.max(1e-18);
      let pct_err = ((out_sim_eth - out_quoter_eth).abs() / denom) * 100.0;

      println!("\n=== USDC -> ETH ===");
      println!("Quoter:    {:.8} ETH", out_quoter_eth);
      println!("Simulator: {:.8} ETH", out_sim_eth);
      println!("Error:     {:.8} ETH | {:.3}%", (out_sim_eth - out_quoter_eth).abs(), pct_err);

      assert!(
          pct_err <= MAX_PCT_ERR,
          "USDC->ETH simulator error too high: {:.3}% (max {:.3}%)",
          pct_err, MAX_PCT_ERR
      );
  }

  println!("\n✅ Uniswap v4 quoter vs simulator test PASSED (within {:.2}% error)", MAX_PCT_ERR);
  Ok(())
}
