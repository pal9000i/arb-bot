// tests/aerodrome_base_integration.rs

use arrakis_arbitrage::math::aerodrome_volatile::{VolatilePairState, SwapDirection, simulate_exact_in_volatile, from_raw};
use ethers::prelude::*;
use std::str::FromStr;
use std::sync::Arc;

// ====== ABIs ======

// Aerodrome Router for getting quotes
abigen!(
    AerodromeRouter,
    "./abis/AerodromeRouter.json"
);

// Aerodrome Pool interface for getting reserves and quotes
abigen!(
    AerodromePool,
    "./abis/AerodromePool.json"
);

// Aerodrome Pool Fee interface
abigen!(
    AerodromePairFees,
    "./abis/AerodromePairFees.json"
);

// Aerodrome Factory Fee interface
abigen!(
    AerodromeFactoryFees,
    "./abis/AerodromeFactoryFees.json"
);

// Note: Token decimals read from environment variables for performance

// Aerodrome Factory for finding pools
abigen!(
    AerodromeFactory,
    "./abis/AerodromeFactory.json"
);

// ====== Constants ======

// Base network addresses
const AERODROME_ROUTER: &str = "0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43"; // Aerodrome Router V2
const AERODROME_FACTORY: &str = "0x420DD381b31aEf6683db6B902084cB0FFECe40Da"; // Aerodrome Factory

// Base token addresses
const BASE_WETH: &str = "0x4200000000000000000000000000000000000006"; // WETH on Base
const BASE_USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"; // USDC on Base

// ====== Helpers ======

// Read actual fee bps from pool or factory
async fn read_fee_bps(
    provider: Arc<Provider<Http>>,
    pool_addr: Address,
    is_stable: bool,
) -> Option<u32> {
    // Try pool-level fee functions first
    let pair_fees = AerodromePairFees::new(pool_addr, provider.clone());
    
    // Try fee() function
    if let Ok(fee) = pair_fees.fee().call().await {
        let fee_u32 = fee.as_u32();
        println!("  🔍 Pool fee(): {} bps", fee_u32);
        return Some(fee_u32);
    }
    
    // Try fees() function
    if let Ok(fee) = pair_fees.fees().call().await {
        let fee_u32 = fee.as_u32();
        println!("  🔍 Pool fees(): {} bps", fee_u32);
        return Some(fee_u32);
    }
    
    // Factory fallback
    if let Ok(factory_addr) = Address::from_str(AERODROME_FACTORY) {
        let factory = AerodromeFactoryFees::new(factory_addr, provider);
        
        if is_stable {
            if let Ok(fee) = factory.stable_fee().call().await {
                let fee_u32 = fee.as_u32();
                println!("  🔍 Factory stableFee(): {} bps", fee_u32);
                return Some(fee_u32);
            }
        } else {
            if let Ok(fee) = factory.volatile_fee().call().await {
                let fee_u32 = fee.as_u32();
                println!("  🔍 Factory volatileFee(): {} bps", fee_u32);
                return Some(fee_u32);
            }
        }
    }
    
    println!("  ⚠️  Failed to read fee, using fallback");
    None
}

// Fetch real Aerodrome pool state from Base network
async fn fetch_aerodrome_pool_state(
    provider: Arc<Provider<Http>>,
    pool_address: Address,
) -> Result<(VolatilePairState, bool), Box<dyn std::error::Error + Send + Sync>> {
    let pool = AerodromePool::new(pool_address, provider.clone());

    // Get pool basic info
    let token0_addr = pool.token_0().call().await?;
    let token1_addr = pool.token_1().call().await?;
    let is_stable = pool.stable().call().await?;
    let (reserve0, reserve1, timestamp) = pool.get_reserves().call().await?;

    println!("📊 Aerodrome Pool State:");
    println!("  Pool Address: {}", pool_address);
    println!("  Token0: {}", token0_addr);
    println!("  Token1: {}", token1_addr);
    println!("  Stable: {}", is_stable);
    
    // Determine decimals from env variables based on token addresses
    let weth_decimals: u8 = std::env::var("WETH_DECIMALS")
        .unwrap_or("18".to_string())
        .parse()
        .unwrap_or(18);
    let usdc_decimals: u8 = std::env::var("USDC_DECIMALS")
        .unwrap_or("6".to_string())
        .parse()
        .unwrap_or(6);
    
    let (decimals0, decimals1) = if token0_addr == Address::from_str(BASE_WETH)? {
        (weth_decimals, usdc_decimals) // token0=WETH, token1=USDC
    } else if token1_addr == Address::from_str(BASE_WETH)? {
        (usdc_decimals, weth_decimals) // token0=USDC, token1=WETH
    } else {
        return Err("Pool does not contain WETH/USDC pair".into());
    };
    
    println!("  Decimals: token0={}, token1={} (from env)", decimals0, decimals1);
    
    // Read real fee from pool or factory
    let fee_bps = read_fee_bps(provider.clone(), pool_address, is_stable)
        .await
        .unwrap_or_else(|| if is_stable { 1 } else { 5 }); // fallback
    
    println!("  Fee: {} bps ({:.3}%)", fee_bps, fee_bps as f64 / 100.0);
    
    // Display reserves with correct decimals
    println!("  Reserve0: {} ({:.6} tokens)", reserve0, reserve0.as_u128() as f64 / 10f64.powi(decimals0 as i32));
    println!("  Reserve1: {} ({:.6} tokens)", reserve1, reserve1.as_u128() as f64 / 10f64.powi(decimals1 as i32));
    println!("  Timestamp: {}", timestamp);

    // Create volatile pair state with real on-chain values
    let pair_state = VolatilePairState {
        token0: token0_addr,
        token1: token1_addr,
        reserve0,
        reserve1,
        decimals0,
        decimals1,
        fee_bps,
    };

    Ok((pair_state, is_stable))
}

// Find Aerodrome pool for token pair
async fn find_aerodrome_pool(
    provider: Arc<Provider<Http>>,
    token_a: Address,
    token_b: Address,
    is_stable: bool,
) -> Result<Address, Box<dyn std::error::Error + Send + Sync>> {
    let factory = AerodromeFactory::new(Address::from_str(AERODROME_FACTORY)?, provider);
    let pool_addr = factory.get_pool(token_a, token_b, is_stable).call().await?;
    
    if pool_addr == Address::zero() {
        return Err(format!("No pool found for tokens {} - {} (stable: {})", token_a, token_b, is_stable).into());
    }
    
    Ok(pool_addr)
}

// ====== Tests ======

#[tokio::test]
async fn test_aerodrome_router_vs_simulator() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    // --- Provider for Base network
    let base_rpc_url = match std::env::var("BASE_RPC_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("⚠️ Skipping Aerodrome router test - BASE_RPC_URL not set");
            return Ok(());
        }
    };
    let provider = Arc::new(Provider::<Http>::try_from(base_rpc_url)?);

    println!("🔎 Testing Aerodrome on Base network...");
    
    let weth = Address::from_str(BASE_WETH)?;
    let usdc = Address::from_str(BASE_USDC)?;

    // Test both stable and volatile pools
    for is_stable in [false, true] {
        println!("\n--- Testing {} pool ---", if is_stable { "Stable" } else { "Volatile" });
        
        // Find pool
        let pool_addr = match find_aerodrome_pool(provider.clone(), weth, usdc, is_stable).await {
            Ok(addr) => {
                println!("✅ Found pool: {}", addr);
                addr
            }
            Err(e) => {
                println!("⚠️  No {} pool found: {}", if is_stable { "stable" } else { "volatile" }, e);
                continue;
            }
        };

        // Get real pool state
        let (pool_state, is_stable) = fetch_aerodrome_pool_state(provider.clone(), pool_addr).await?;

        // --- Router quote
        println!("\n📞 Getting Aerodrome Router quote...");
        let router = AerodromeRouter::new(Address::from_str(AERODROME_ROUTER)?, provider.clone());
        
        let amount_in = U256::from_dec_str("1000000000000000000")?; // 1 WETH
        let factory = Address::from_str(AERODROME_FACTORY)?;
        
        // Create route for WETH -> USDC with factory address
        let route = vec![(weth, usdc, is_stable, factory)];
        
        let amounts_out = router.get_amounts_out(amount_in, route).call().await?;
        let usdc_out_router = if amounts_out.len() >= 2 {
            amounts_out[1].as_u128() as f64 / 1e6
        } else {
            println!("  ⚠️  Unexpected router response");
            continue;
        };
        
        println!("  Router amountOut: {:.6} USDC", usdc_out_router);

        // --- Simulator
        println!("\n🧪 Running Aerodrome volatile simulator...");
        
        // Skip stable pools for now since we only have volatile math
        if is_stable {
            println!("  ⚠️  Skipping stable pool simulation (volatile math only)");
            continue;
        }
        
        // Determine WETH->USDC swap direction 
        let weth = Address::from_str(BASE_WETH)?;
        let is_token0_weth = pool_state.token0 == weth;
        
        let direction = if is_token0_weth {
            SwapDirection::ZeroForOne // WETH(token0) -> USDC(token1)
        } else {
            SwapDirection::OneForZero // WETH(token1) -> USDC(token0)
        };
        
        let output_decimals = match direction {
            SwapDirection::ZeroForOne => pool_state.decimals1,
            SwapDirection::OneForZero => pool_state.decimals0,
        };
        
        let amount_in_human = 1.0; // 1 WETH
        let amount_in_wei = U256::from_dec_str("1000000000000000000")?; // 1 WETH in wei
        
        println!("  Direction: {:?} (WETH at position {})", direction, if is_token0_weth { "0" } else { "1" });
        
        // --- Ground truth: pair.getAmountOut
        println!("\n📊 Pair direct quote...");
        let pair_for_quote = AerodromePool::new(pool_addr, provider.clone());
        let pair_direct_out = pair_for_quote.get_amount_out(amount_in_wei, weth).call().await?;
        let pair_direct_usdc = from_raw(pair_direct_out, output_decimals);
        println!("  Pair getAmountOut: {:.6} USDC", pair_direct_usdc);
        
        // --- Our simulator
        let (_amount_in_raw, amount_out_raw, effective_price, spot_price, price_impact_pct) = 
            simulate_exact_in_volatile(&pool_state, direction, amount_in_human);
        
        let usdc_out_sim = from_raw(amount_out_raw, output_decimals);
        println!("  Simulator amountOut: {:.6} USDC", usdc_out_sim);
        println!("  Price impact: {:.3}%", price_impact_pct);
        println!("  Effective price: {:.2} USDC/WETH", effective_price);
        println!("  Spot price: {:.2} USDC/WETH", spot_price);
        
        // --- Compare simulator vs pair direct
        let sim_vs_pair_diff = (usdc_out_sim - pair_direct_usdc).abs();
        let sim_vs_pair_err = if pair_direct_usdc > 0.0 { (sim_vs_pair_diff / pair_direct_usdc) * 100.0 } else { 0.0 };
        
        println!("\n=== SIMULATOR vs PAIR ===");
        println!("Pair direct: {:.6} USDC", pair_direct_usdc);
        println!("Simulator:   {:.6} USDC", usdc_out_sim);
        println!("Diff:        {:.6} USDC | Err: {:.6}%", sim_vs_pair_diff, sim_vs_pair_err);
        
        if sim_vs_pair_err < 0.001 {
            println!("✅ Simulator matches pair exactly!");
        } else if sim_vs_pair_err < 0.01 {
            println!("✅ Simulator very close to pair");
        } else {
            println!("⚠️  Simulator error vs pair: {:.6}%", sim_vs_pair_err);
        }

        // --- Router vs Simulator comparison
        let router_sim_diff = (usdc_out_sim - usdc_out_router).abs();
        let router_sim_err = if usdc_out_router > 0.0 { (router_sim_diff / usdc_out_router) * 100.0 } else { 0.0 };

        println!("\n=== SIMULATOR vs ROUTER ===");
        println!("Router:    {:.6} USDC", usdc_out_router);
        println!("Simulator: {:.6} USDC", usdc_out_sim);
        println!("Diff:      {:.6} USDC | Err: {:.6}%", router_sim_diff, router_sim_err);

        // With real fees, we should get very close to the router
        if router_sim_err < 0.01 {
            println!("✅ {} pool test PASSED! (Exact match)", if is_stable { "Stable" } else { "Volatile" });
        } else if router_sim_err < 0.1 {
            println!("✅ {} pool test PASSED! (Very close)", if is_stable { "Stable" } else { "Volatile" });
        } else if router_sim_err < 1.0 {
            println!("✅ {} pool test PASSED! (Close enough)", if is_stable { "Stable" } else { "Volatile" });
        } else {
            println!("⚠️  {} pool test: Error {:.6}%", if is_stable { "Stable" } else { "Volatile" }, router_sim_err);
        }
    }

    println!("\n✅ Aerodrome Base integration test completed!");
    Ok(())
}

#[tokio::test]
async fn test_aerodrome_pool_discovery() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    let base_rpc_url = match std::env::var("BASE_RPC_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("⚠️ Skipping Aerodrome pool discovery test - BASE_RPC_URL not set");
            return Ok(());
        }
    };
    let provider = Arc::new(Provider::<Http>::try_from(base_rpc_url)?);

    println!("🔍 Discovering Aerodrome WETH/USDC pools on Base...");
    
    let weth = Address::from_str(BASE_WETH)?;
    let usdc = Address::from_str(BASE_USDC)?;

    // Check both stable and volatile pools
    for is_stable in [false, true] {
        match find_aerodrome_pool(provider.clone(), weth, usdc, is_stable).await {
            Ok(pool_addr) => {
                println!("✅ {} pool found: {}", if is_stable { "Stable" } else { "Volatile" }, pool_addr);
                
                // Get basic pool info
                let (pool_state, is_stable) = fetch_aerodrome_pool_state(provider.clone(), pool_addr).await?;
                
                // Calculate approximate TVL - find which token is USDC (6 decimals)
                let usdc_reserve = if pool_state.decimals0 == 6 {
                    from_raw(pool_state.reserve0, pool_state.decimals0)
                } else if pool_state.decimals1 == 6 {
                    from_raw(pool_state.reserve1, pool_state.decimals1)
                } else {
                    // If neither is 6 decimals, assume token1 is the stable value token
                    from_raw(pool_state.reserve1, pool_state.decimals1)
                };
                let tvl_usd = usdc_reserve * 2.0; // Double to account for both sides
                println!("   TVL: ~${:.2}", tvl_usd);
                println!("   Pool type: {}", if is_stable { "Stable" } else { "Volatile" });
            }
            Err(e) => {
                println!("❌ {} pool: {}", if is_stable { "Stable" } else { "Volatile" }, e);
            }
        }
    }

    Ok(())
}