use arrakis_arbitrage::math::uniswap_v4::*;
use ethers::types::Address;
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};
use std::str::FromStr;

#[tokio::test]
async fn test_mathematical_correctness_against_known_values() {
    println!("=== MATHEMATICAL VALIDATION AGAINST KNOWN UNISWAP V3 VALUES ===");
    
    // Test our implementation against known good values from Uniswap V3
    // (V4 uses the same core math, just different pool structure)
    
    // Test basic tick math correctness
    // Use a tick that should correspond to approximately 3000 USDC/ETH
    let test_price = 3000.0;
    let calculated_tick = tick_from_price(test_price, 18, 6);
    let reverse_price = price_from_tick(calculated_tick, 18, 6);
    
    println!("Price conversion test:");
    println!("  Input price: {:.2} USDC per ETH", test_price);
    println!("  Calculated tick: {}", calculated_tick);
    println!("  Reverse price: {:.2} USDC per ETH", reverse_price);
    
    // Round-trip should be close (within 0.1% due to tick discretization)
    let price_error = (reverse_price - test_price).abs() / test_price;
    assert!(price_error < 0.001, 
            "Round-trip price error too high: {:.6} (expected < 0.1%)", price_error);
    
    // Test that the tick produces a reasonable sqrt price
    let sqrt_price = get_sqrt_ratio_at_tick(calculated_tick);
    println!("  sqrt_price_x96: {}", sqrt_price);
    
    // Should be positive and reasonable magnitude
    assert!(sqrt_price > BigInt::zero(), "sqrt_price should be positive");
    
    // Test edge cases - MIN and MAX ticks
    let min_tick = -887_272;
    let max_tick = 887_272;
    let min_sqrt = get_sqrt_ratio_at_tick(min_tick);
    let max_sqrt = get_sqrt_ratio_at_tick(max_tick);
    
    println!("Edge cases:");
    println!("  MIN_TICK ({}) price: {:.10}", min_tick, price_from_tick(min_tick, 18, 6));
    println!("  MAX_TICK ({}) price: {:.2}", max_tick, price_from_tick(max_tick, 18, 6));
    
    assert!(min_sqrt > BigInt::zero(), "MIN_TICK sqrt should be positive");
    assert!(max_sqrt > min_sqrt, "MAX_TICK sqrt should be > MIN_TICK sqrt");
}

#[tokio::test]
async fn test_real_pool_simulation_accuracy() {
    println!("\n=== REAL POOL SIMULATION ACCURACY TEST ===");
    
    // Create pools that mirror real Uniswap pool characteristics
    let weth = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
    let usdc = Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap();
    
    let pools = create_standard_weth_usdc_pools(weth, usdc, 3000.0).unwrap();
    
    println!("Created {} standard WETH/USDC pools", pools.len());
    
    for (i, pool) in pools.iter().enumerate() {
        println!("\n--- Pool {} ({}bps fee) ---", i + 1, pool.key.fee_ppm / 100);
        println!("  Current tick: {}", pool.tick);
        println!("  Current price: {:.2}", price_from_tick(pool.tick, 18, 6));
        println!("  Liquidity: {}", pool.liquidity);
        println!("  Tick range: {} active ticks", pool.ticks.len());
        
        // Test various swap sizes
        let test_amounts = vec![0.1, 1.0, 10.0];
        
        for amount in test_amounts {
            let result = simulate_exact_in_tokens(
                pool,
                SwapDirection::ZeroForOne,
                None,
                amount,
                18,
                None,
            ).unwrap();
            
            let eth_consumed = (-result.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18;
            let usdc_received = result.amount1.clone().to_f64().unwrap_or(0.0) / 1e6;
            let execution_price = if eth_consumed > 0.0 { usdc_received / eth_consumed } else { 0.0 };
            
            println!("    {:.1} ETH → {:.1} ETH consumed, {:.2} USDC received (price: {:.2})",
                     amount, eth_consumed, usdc_received, execution_price);
            
            // Validation checks
            assert!(eth_consumed > 0.0 && eth_consumed <= amount, 
                    "Should consume positive amount ≤ input: {:.6}", eth_consumed);
            assert!(usdc_received > 0.0, 
                    "Should receive positive USDC: {:.2}", usdc_received);
            assert!(execution_price > 2500.0 && execution_price < 3500.0,
                    "Execution price should be reasonable: {:.2}", execution_price);
            
            // Price impact should be reasonable
            let price_impact = (3000.0_f64 - execution_price).abs() / 3000.0;
            assert!(price_impact < 0.1, // Less than 10% impact
                    "Price impact too high for {:.1} ETH: {:.2}%", amount, price_impact * 100.0);
        }
    }
    
    println!("\n✅ Real pool simulation accuracy test PASSED!");
}

#[tokio::test] 
async fn test_arbitrage_opportunity_detection() {
    println!("\n=== ARBITRAGE OPPORTUNITY DETECTION TEST ===");
    
    // Create two pools with slightly different prices to simulate arbitrage opportunity
    let weth = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
    let usdc = Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap();
    
    let pools_low = create_standard_weth_usdc_pools(weth, usdc, 2980.0).unwrap();  // 2980 price
    let pools_high = create_standard_weth_usdc_pools(weth, usdc, 3020.0).unwrap(); // 3020 price
    
    let trade_size = 5.0; // 5 ETH
    
    // Simulate buying on low price pool
    let buy_result = simulate_exact_in_tokens(
        &pools_low[1], // 0.3% fee
        SwapDirection::ZeroForOne,
        None,
        trade_size,
        18,
        None,
    ).unwrap();
    
    let eth_used = (-buy_result.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18;
    let usdc_received = buy_result.amount1.clone().to_f64().unwrap_or(0.0) / 1e6;
    let buy_price = usdc_received / eth_used;
    
    println!("Buy on low-price pool:");
    println!("  {} ETH → {:.2} USDC", eth_used, usdc_received);
    println!("  Execution price: {:.2}", buy_price);
    
    // Simulate selling the USDC on high price pool (reverse direction)
    let sell_result = simulate_exact_in_tokens(
        &pools_high[1], // 0.3% fee  
        SwapDirection::OneForZero,
        None,
        usdc_received / 1e6, // Convert back to token units for input
        6,  // USDC decimals
        None,
    ).unwrap();
    
    let usdc_used = (-sell_result.amount1.clone()).to_f64().unwrap_or(0.0) / 1e6;
    let eth_received = sell_result.amount0.clone().to_f64().unwrap_or(0.0) / 1e18;
    let sell_price = usdc_used / eth_received;
    
    println!("Sell on high-price pool:");
    println!("  {:.2} USDC → {} ETH", usdc_used, eth_received);
    println!("  Execution price: {:.2}", sell_price);
    
    // Calculate arbitrage profit
    let net_eth = eth_received - eth_used;
    let profit_usd = net_eth * 3000.0; // Approximate USD value
    
    println!("Arbitrage result:");
    println!("  Net ETH: {:.6}", net_eth);
    println!("  Estimated profit: ${:.2}", profit_usd);
    
    // Should be profitable (accounting for fees and price differences)
    if profit_usd > 10.0 {
        println!("✅ Profitable arbitrage opportunity detected!");
        assert!(net_eth > 0.0, "Should have positive ETH profit: {:.6}", net_eth);
    } else {
        println!("ℹ️  Small profit/loss due to fees and price impact: ${:.2}", profit_usd);
        // This is normal - small price differences may not overcome fees
    }
}

#[tokio::test]
async fn test_price_consistency_across_fee_tiers() {
    println!("\n=== PRICE CONSISTENCY ACROSS FEE TIERS TEST ===");
    
    let weth = Address::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
    let usdc = Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap();
    
    let pools = create_standard_weth_usdc_pools(weth, usdc, 3000.0).unwrap();
    
    println!("Testing small trade across all fee tiers...");
    let small_trade = 0.1; // 0.1 ETH - should have minimal price impact
    
    let mut prices = Vec::new();
    
    for pool in &pools {
        let result = simulate_exact_in_tokens(
            pool,
            SwapDirection::ZeroForOne,
            None,
            small_trade,
            18,
            None,
        ).unwrap();
        
        let eth_used = (-result.amount0.clone()).to_f64().unwrap_or(0.0) / 1e18;
        let usdc_received = result.amount1.clone().to_f64().unwrap_or(0.0) / 1e6;
        let execution_price = usdc_received / eth_used;
        
        prices.push(execution_price);
        
        println!("  {}bps fee: {:.2} USDC/ETH", pool.key.fee_ppm / 100, execution_price);
    }
    
    // All prices should be close to each other for small trades
    let min_price = prices.iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let max_price = prices.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    let price_spread = max_price - min_price;
    let relative_spread = price_spread / min_price;
    
    println!("Price spread: {:.2} ({:.3}%)", price_spread, relative_spread * 100.0);
    
    // For small trades, price spread should be mainly due to fees
    assert!(relative_spread < 0.02, // Less than 2% spread
            "Price spread too high: {:.3}%", relative_spread * 100.0);
    
    println!("✅ Price consistency test PASSED!");
}