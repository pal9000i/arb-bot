// tests/arbitrage_optimizer_live_data.rs
// ======================================
// Live-data integration test for your optimizer using Uniswap V4 (Ethereum mainnet)
// and Aerodrome volatile pool (Base mainnet).

use arrakis_arbitrage::engine::optimizer::{
    optimize, OptimizerInputs,
};
use arrakis_arbitrage::chain::gas::{GasEstimate, create_test_gas_estimate};
use arrakis_arbitrage::math::aerodrome_volatile::VolatilePairState;
use arrakis_arbitrage::math::uniswap_v4::create_pool_with_real_data;
use arrakis_arbitrage::math::uniswap_v4::PoolState as UniPoolState;

use ethers::prelude::*;
use ethers::abi::{self, Token};
use ethers::types::{Address, U256};
use ethers::utils::keccak256;
use num_bigint::BigInt;
use std::str::FromStr;
use std::sync::Arc;

// --------- Constants (token addresses) ---------

#[allow(dead_code)] // May be used in future test expansions
const ETHEREUM_WETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";
const ETHEREUM_USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";

const BASE_WETH: &str = "0x4200000000000000000000000000000000000006";
const BASE_USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";

// Aerodrome factory (Base mainnet)
const AERODROME_FACTORY: &str = "0x420DD381b31aEf6683db6B902084cB0FFECe40Da";

// --------- ABIs ---------

abigen!(
    // Minimal read-only view for Uniswap V4 pools (poolId → slot0/liquidity).
    // Provide the deployed address via UNIV4_STATE_VIEW_ADDRESS env var.
    StateView,
    r#"[
      {
        "type": "function",
        "name": "getSlot0",
        "stateMutability": "view",
        "inputs": [{"name": "poolId", "type": "bytes32"}],
        "outputs": [
          {"name": "sqrtPriceX96", "type": "uint160"},
          {"name": "tick", "type": "int24"}
        ]
      },
      {
        "type": "function",
        "name": "getLiquidity",
        "stateMutability": "view",
        "inputs": [{"name": "poolId", "type": "bytes32"}],
        "outputs": [{"name": "liquidity", "type": "uint128"}]
      }
    ]"#
);

abigen!(
    AerodromeFactory,
    r#"[
      {"type":"function","name":"getPool","stateMutability":"view",
       "inputs":[
         {"name":"tokenA","type":"address"},
         {"name":"tokenB","type":"address"},
         {"name":"stable","type":"bool"}],
       "outputs":[{"name":"pool","type":"address"}]
      }
    ]"#
);

abigen!(
    AerodromePool,
    r#"[
      {"type":"function","name":"getReserves","stateMutability":"view","inputs":[],"outputs":[
        {"name":"reserve0","type":"uint256"},
        {"name":"reserve1","type":"uint256"},
        {"name":"blockTimestampLast","type":"uint256"}
      ]},
      {"type":"function","name":"token0","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]},
      {"type":"function","name":"token1","stateMutability":"view","inputs":[],"outputs":[{"name":"","type":"address"}]}
    ]"#
);

abigen!(
    AerodromePoolFactory,
    r#"[
      {"type":"function","name":"getFee","stateMutability":"view","inputs":[
        {"name":"pool","type":"address"},
        {"name":"stable","type":"bool"}
      ],"outputs":[{"name":"","type":"uint256"}]}
    ]"#
);

// --------- Helpers: Uniswap V4 PoolKey → poolId ---------

/// Encode PoolKey as Solidity does and keccak256 hash to get poolId.
/// PoolKey = (currency0: address, currency1: address, fee: uint24, tickSpacing: int24, hooks: address)
fn v4_pool_id(currency0: Address, currency1: Address, fee_ppm: u32, tick_spacing: i32, hooks: Address) -> [u8; 32] {
    // Solidity encodes int24 as uint256 in ABI, so we use U256 for both
    let tokens = vec![
        Token::Tuple(vec![
            Token::Address(currency0),
            Token::Address(currency1),
            Token::Uint(U256::from(fee_ppm)),      // uint24
            Token::Int(U256::from(tick_spacing as i64)),  // int24 as signed
            Token::Address(hooks),
        ])
    ];
    keccak256(abi::encode(&tokens))
}

/// For Uniswap V4, native ETH is `address(0)`. We build the Currency ordering from ETH (0x0) and USDC.
fn v4_currency_order_for_eth_usdc(usdc: Address) -> (Address, Address, bool) {
    // currencyETH = address(0)
    let eth = Address::zero();
    // order: currency0 < currency1
    if eth < usdc {
        (eth, usdc, true) // token0 is ETH
    } else {
        (usdc, eth, false) // token1 is ETH
    }
}

// --------- Live fetchers ---------

async fn fetch_uniswap_v4_pool_state(
    provider: Arc<Provider<Http>>,
    state_view_addr: Address,
    usdc_addr: Address,
    fee_ppm: u32,      // e.g., 3000
    tick_spacing: i32, // e.g., 60
) -> Result<(UniPoolState, /*token0_is_eth*/ bool), Box<dyn std::error::Error + Send + Sync>> {
    let state_view = StateView::new(state_view_addr, provider);

    // Currency ordering with native ETH (address(0))
    let (currency0, currency1, token0_is_eth) = v4_currency_order_for_eth_usdc(usdc_addr);
    let hooks = Address::zero(); // common case
    let pool_id = v4_pool_id(currency0, currency1, fee_ppm, tick_spacing, hooks);

    let (sqrt_price_x96, tick) = state_view.get_slot_0(pool_id).call().await?;
    let liquidity = state_view.get_liquidity(pool_id).call().await?;

    println!("📊 Uniswap V4 live:");
    println!("  currency0: {currency0}  (isETH0: {token0_is_eth})");
    println!("  currency1: {currency1}");
    println!("  fee_ppm: {fee_ppm}, tick_spacing: {tick_spacing}, hooks: {hooks}");
    println!("  sqrtPriceX96: {sqrt_price_x96}  tick: {tick}  liquidity: {liquidity}");

    // Convert to your simulator's PoolState
    // (We'll synthesize a tiny 2-tick band around current tick with all liquidity.)
    let sqrt_bi = BigInt::from(sqrt_price_x96.as_u128());
    let liq_bi  = BigInt::from(liquidity as u128);
    let lower = tick - 600;
    let upper = tick + 600;
    let tick_data = vec![(lower, liq_bi.clone()), (upper, -liq_bi.clone())];

    let pool = create_pool_with_real_data(
        currency0,
        currency1,
        fee_ppm,
        tick_spacing,
        hooks,
        sqrt_bi,
        tick,
        liq_bi,
        tick_data,
    );

    Ok((pool, token0_is_eth))
}

async fn fetch_aerodrome_pair(
    provider: Arc<Provider<Http>>,
    weth: Address,
    usdc: Address,
) -> Result<(VolatilePairState, /*token0_is_weth*/ bool), Box<dyn std::error::Error + Send + Sync>> {
    let factory = AerodromeFactory::new(Address::from_str(AERODROME_FACTORY)?, provider.clone());
    let pool_addr = factory.get_pool(weth, usdc, false).call().await?;
    if pool_addr == Address::zero() {
        return Err("Aerodrome volatile pool not found".into());
    }

    let pool = AerodromePool::new(pool_addr, provider.clone());
    let token0 = pool.token_0().call().await?;
    let token1 = pool.token_1().call().await?;
    let (r0, r1, _ts) = pool.get_reserves().call().await?;

    // Get fee from factory using the correct method
    let factory = AerodromePoolFactory::new(Address::from_str(AERODROME_FACTORY)?, provider.clone());
    
    // Debug: Check what the factory getFee call returns for both stable and volatile
    println!("🔍 DEBUG: Testing both stable and volatile fee calls...");
    
    match factory.get_fee(pool_addr, false).call().await { // false = volatile pool
        Ok(fee_raw) => {
            println!("🔍 DEBUG: factory.getFee(volatile) returned: {}", fee_raw);
            println!("🔍 DEBUG: volatile fee as percentage: {:.6}%", fee_raw.as_u32() as f64 / 10000.0);
        }
        Err(e) => println!("🔍 DEBUG: volatile getFee() FAILED: {}", e),
    }
    
    match factory.get_fee(pool_addr, true).call().await { // true = stable pool
        Ok(fee_raw) => {
            println!("🔍 DEBUG: factory.getFee(stable) returned: {}", fee_raw);
            println!("🔍 DEBUG: stable fee as percentage: {:.6}%", fee_raw.as_u32() as f64 / 10000.0);
        }
        Err(e) => println!("🔍 DEBUG: stable getFee() FAILED: {}", e),
    }
    
    // Also check if fee is stored differently (maybe /1000000 instead of /10000)
    let fee_raw = factory.get_fee(pool_addr, false).call().await.unwrap_or(U256::from(30));
    println!("🔍 DEBUG: Alternative interpretations:");
    println!("🔍 DEBUG: fee/1000000 = {:.6}%", fee_raw.as_u32() as f64 / 1000000.0);
    println!("🔍 DEBUG: fee/100000 = {:.6}%", fee_raw.as_u32() as f64 / 100000.0);
    
    let fee_bps = factory.get_fee(pool_addr, false).call().await.unwrap_or_else(|_| U256::from(30)).as_u32();

    let token0_is_weth = token0 == weth;

    println!("📊 Aerodrome live:");
    println!("  pair: {pool_addr}");
    println!("  token0: {token0} (isWETH0: {token0_is_weth})");
    println!("  token1: {token1}");
    println!("  reserves: r0={r0}  r1={r1}");
    println!("  fee_bps: {fee_bps}");

    let pair = VolatilePairState {
        token0,
        token1,
        reserve0: r0,
        reserve1: r1,
        decimals0: if token0_is_weth { 18 } else { 6 },
        decimals1: if token0_is_weth { 6 } else { 18 },
        fee_bps,
    };
    Ok((pair, token0_is_weth))
}

async fn fetch_live_gas(
    eth: Arc<Provider<Http>>,
    base: Arc<Provider<Http>>,
    total_usd: f64, // pass a quote (or wire Coinbase here if you like)
) -> Result<(GasEstimate, GasEstimate), Box<dyn std::error::Error + Send + Sync>> {
    let eth_gas_price = eth.get_gas_price().await?;
    let base_gas_price = base.get_gas_price().await?;

    let eth_gas = create_test_gas_estimate(eth_gas_price.as_u128() as u64, 200_000u64, total_usd);
    let base_gas = create_test_gas_estimate(base_gas_price.as_u128() as u64, 150_000u64, total_usd);

    println!("⛽ Gas (live): eth={} gwei (~${:.2}), base={} gwei (~${:.2})",
        eth_gas_price.as_u128() / 1_000_000_000, eth_gas.total_usd,
        base_gas_price.as_u128() / 1_000_000_000, base_gas.total_usd
    );

    Ok((eth_gas, base_gas))
}

// --------- The tests ---------

#[tokio::test]
async fn test_arbitrage_optimizer_live() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    // Skip test if RPC URLs not provided
    let eth_rpc = match std::env::var("ETHEREUM_RPC_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("⚠️ Skipping live arbitrage optimizer test - ETHEREUM_RPC_URL not set");
            return Ok(());
        }
    };
    let base_rpc = match std::env::var("BASE_RPC_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("⚠️ Skipping live arbitrage optimizer test - BASE_RPC_URL not set");
            return Ok(());
        }
    };
    let v4_state_view_addr = match std::env::var("UNISWAP_V4_STATE_VIEW") {
        Ok(addr) => addr,
        Err(_) => {
            println!("⚠️ Skipping live arbitrage optimizer test - UNISWAP_V4_STATE_VIEW not set");
            return Ok(());
        }
    }; // deployer's StateView address

    let eth = Arc::new(Provider::<Http>::try_from(eth_rpc)?);
    let base = Arc::new(Provider::<Http>::try_from(base_rpc)?);

    // quick connectivity sanity
    let (eth_bn, base_bn) = tokio::join!(eth.get_block_number(), base.get_block_number());
    println!("✅ connected: eth#{}, base#{}", eth_bn?, base_bn?);

    let usdc_eth = Address::from_str(ETHEREUM_USDC)?;
    let (uni_pool, uni_token0_is_eth) =
        fetch_uniswap_v4_pool_state(eth.clone(), Address::from_str(&v4_state_view_addr)?, usdc_eth, 3000, 60).await?;

    // On V4, token is native ETH (address(0)). Your Uniswap simulator tracks “WETH”.
    // For price/amount math below, we treat “ETH” like WETH with 18 decimals.
    let base_weth = Address::from_str(BASE_WETH)?;
    let base_usdc = Address::from_str(BASE_USDC)?;
    let (aero_pair, aero_token0_is_weth) = fetch_aerodrome_pair(base.clone(), base_weth, base_usdc).await?;

    // ETH/USD ref — plug a CEX quote here; we keep a constant for the test harness.
    let total_usd = 3500.0;
    let (gas_eth, gas_base) = fetch_live_gas(eth.clone(), base.clone(), total_usd).await?;

    // Build optimizer inputs
    let inputs = OptimizerInputs {
        // Uniswap v4 side
        uni_pool,
        // IMPORTANT: The simulator flag means “is token0 the 18-dec ETH side?”
        // For v4 ETH/USDC, currency0=ETH(address(0)) → true if our pool currency0 is ETH.
        uni_token0_is_weth: uni_token0_is_eth,
        uni_fee_ppm_override: Some(3000), // set fee tier you queried

        // Aerodrome side (Base)
        aero_pair,
        aero_token0_is_weth,

        // Costs
        gas_eth,
        gas_base,
        bridge_cost_usd: 10.0,

        // Search
        hint_size_eth: 1.0,
        max_size_eth: 50.0,
    };

    println!("🧮 running optimizer with live pool snapshots...");
    let t0 = std::time::Instant::now();
    let maybe = optimize(&inputs);
    let dt = t0.elapsed();
    println!("⏱️  optimize() took {:?}", dt);

    match maybe {
        Some(op) => {
            println!("🎯 RESULT");
            println!("  direction: {:?}", op.direction);
            println!("  optimal size: {:.4} ETH", op.optimal_size_eth);
            println!("  proceeds: ${:.2}", op.proceeds_usd);
            println!("  costs:    ${:.2}", op.costs_usd);
            println!("  gas:      ${:.2}", op.gas_usd_total);
            println!("  bridge:   ${:.2}", op.bridge_cost_usd);
            println!("  net:      ${:.2}", op.net_profit_usd);
            println!("  sell px (USDC/ETH): {:.2}", op.eff_price_sell_usdc_per_eth);
            println!("  buy  px (USDC/ETH): {:.2}", op.eff_price_buy_usdc_per_eth);

            // Basic sanity assertions (don’t require profit to be >0 in live test)
            assert!(op.optimal_size_eth > 0.0);
            assert!(op.optimal_size_eth <= 50.0);
        }
        None => {
            println!("❌ no opportunity (live spreads + gas aren’t favorable)");
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_live_fetch_only() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    // Skip test if RPC URLs not provided
    let eth_rpc = match std::env::var("ETHEREUM_RPC_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("⚠️ Skipping live fetch test - ETHEREUM_RPC_URL not set");
            return Ok(());
        }
    };
    let base_rpc = match std::env::var("BASE_RPC_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("⚠️ Skipping live fetch test - BASE_RPC_URL not set");
            return Ok(());
        }
    };
    let v4_state_view_addr = match std::env::var("UNISWAP_V4_STATE_VIEW") {
        Ok(addr) => addr,
        Err(_) => {
            println!("⚠️ Skipping live fetch test - UNISWAP_V4_STATE_VIEW not set");
            return Ok(());
        }
    };

    let eth = Arc::new(Provider::<Http>::try_from(eth_rpc)?);
    let base = Arc::new(Provider::<Http>::try_from(base_rpc)?);

    // Uniswap V4 live
    let usdc_eth = Address::from_str(ETHEREUM_USDC)?;
    let (pool, token0_is_eth) =
        fetch_uniswap_v4_pool_state(eth.clone(), Address::from_str(&v4_state_view_addr)?, usdc_eth, 3000, 60).await?;
    println!("✅ V4 slot0/liquidity OK; token0_is_eth={}", token0_is_eth);
    println!("   sqrtPriceX96={}, tick={}, L={}", pool.sqrt_price_x96, pool.tick, pool.liquidity);

    // Aerodrome live
    let (pair, token0_is_weth) =
        fetch_aerodrome_pair(base.clone(), Address::from_str(BASE_WETH)?, Address::from_str(BASE_USDC)?).await?;
    println!("✅ Aerodrome reserves OK; token0_is_weth={}", token0_is_weth);
    println!("   r0={}, r1={}, fee_bps={}", pair.reserve0, pair.reserve1, pair.fee_bps);

    // Gas
    let (_ge, _gb) = fetch_live_gas(eth.clone(), base.clone(), 3500.0).await?;
    println!("✅ gas OK");

    Ok(())
}
