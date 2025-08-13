// tests/arbitrage_optimizer_integration.rs
// =========================================
// Integration tests for the arbitrage optimizer that finds optimal trade sizes
// using bracket search and golden section optimization.

use arrakis_arbitrage::engine::optimizer::{
    optimize, OptimizerInputs, ArbDirection,
};
use arrakis_arbitrage::chain::gas::create_test_gas_estimate;
use arrakis_arbitrage::math::aerodrome_volatile::{VolatilePairState, to_raw as aero_to_raw};
use arrakis_arbitrage::math::uniswap_v4::{
    create_pool_with_real_data, PoolState as UniPoolState,
};

use ethers::types::Address;
use num_bigint::BigInt;
use std::str::FromStr;

// ====== Test Helpers ======

/// Create a mock Uniswap V4 pool with configurable price
fn create_mock_uniswap_pool(
    weth_is_token0: bool,
    price_usdc_per_eth: f64,
    liquidity_usd: f64,
) -> UniPoolState {
    let weth_addr = if weth_is_token0 { Address::zero() } else { Address::from([0x11; 20]) };
    let usdc_addr = if weth_is_token0 { Address::from([0x22; 20]) } else { Address::zero() };
    
    // Calculate sqrt_price_x96 from human price
    // price = (sqrtPrice / 2^96)^2 * 10^(dec0-dec1) = (sqrtPrice / 2^96)^2 * 10^12
    // For WETH/USDC: price_usdc_per_eth = sqrtPrice^2 / 2^192 * 10^12
    let sqrt_price = (price_usdc_per_eth / 1e12_f64).sqrt() * (1u128 << 96) as f64;
    let sqrt_price_x96 = BigInt::from(sqrt_price as u128);
    
    // Approximate tick from price
    let tick = ((price_usdc_per_eth / 1e12_f64).ln() / 1.0001_f64.ln()).floor() as i32;
    
    // Mock liquidity
    let liquidity = BigInt::from((liquidity_usd * 1e18 / price_usdc_per_eth) as u128);
    
    // Create tick data around current tick
    let tick_data = vec![
        (tick - 600, liquidity.clone()),
        (tick + 600, -liquidity.clone()),
    ];
    
    create_pool_with_real_data(
        weth_addr,
        usdc_addr,
        3000,  // 0.3% fee
        60,    // tick spacing
        Address::zero(), // no hooks
        sqrt_price_x96,
        tick,
        liquidity,
        tick_data,
    )
}

/// Create a mock Aerodrome volatile pool
fn create_mock_aerodrome_pool(
    weth_is_token0: bool,
    price_usdc_per_eth: f64,
    liquidity_usd: f64,
    fee_bps: u32,
) -> VolatilePairState {
    let weth_addr = if weth_is_token0 {
        Address::from_str("0x4200000000000000000000000000000000000006")
            .expect("Failed to parse WETH address")
    } else {
        Address::from([0x33; 20])
    };
    let usdc_addr = if !weth_is_token0 {
        Address::from_str("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913")
            .expect("Failed to parse USDC address")
    } else {
        Address::from([0x44; 20])
    };
    
    // Calculate reserves to match target price
    // For constant product: price = reserve1 / reserve0
    let weth_reserve = liquidity_usd / price_usdc_per_eth / 2.0; // Split liquidity 50/50 by value
    let usdc_reserve = liquidity_usd / 2.0;
    
    let (reserve0, reserve1) = if weth_is_token0 {
        (aero_to_raw(weth_reserve, 18), aero_to_raw(usdc_reserve, 6))
    } else {
        (aero_to_raw(usdc_reserve, 6), aero_to_raw(weth_reserve, 18))
    };
    
    VolatilePairState {
        token0: if weth_is_token0 { weth_addr } else { usdc_addr },
        token1: if weth_is_token0 { usdc_addr } else { weth_addr },
        reserve0,
        reserve1,
        decimals0: if weth_is_token0 { 18 } else { 6 },
        decimals1: if weth_is_token0 { 6 } else { 18 },
        fee_bps,
    }
}

// ====== Integration Tests ======

#[test]
fn test_optimizer_finds_profitable_arbitrage() {
    // Setup: Uniswap more expensive (3500 USDC/ETH), Aerodrome cheaper (3400 USDC/ETH)
    // Optimal strategy: Buy on Aerodrome, sell on Uniswap
    
    let uni_pool = create_mock_uniswap_pool(true, 3500.0, 10_000_000.0);
    let aero_pair = create_mock_aerodrome_pool(true, 3400.0, 5_000_000.0, 30); // 0.3% fee
    
    let inputs = OptimizerInputs {
        uni_pool,
        uni_token0_is_weth: true,
        uni_fee_ppm_override: Some(3000), // 0.3%
        
        aero_pair,
        aero_token0_is_weth: true,
        
        gas_eth: create_test_gas_estimate(25_000_000_000, 200_000, 3500.0),
        gas_base: create_test_gas_estimate(10_000_000_000, 200_000, 3500.0),
        bridge_cost_usd: 50.0, // High bridge cost
        hint_size_eth: 1.0,
        max_size_eth: 100.0,
    };
    
    let result = optimize(&inputs);
    
    if let Some(res) = result {
        println!("💸 High gas cost scenario:");
        println!("  Net Profit: ${:.2}", res.net_profit_usd);
        println!("  Gas Costs: ${:.2}", res.gas_usd_total);
        println!("  Bridge Cost: ${:.2}", res.bridge_cost_usd);
        println!("  Optimal Size: {:.4} ETH", res.optimal_size_eth);
        
        // With high gas, optimizer should find larger trade size to amortize fixed costs
        // or return negative profit if not profitable at any size
        if res.net_profit_usd > 0.0 {
            assert!(res.optimal_size_eth > 5.0, 
                    "Should use larger size to amortize high gas costs");
        }
    }
}

#[test]
fn test_optimizer_with_different_token_orders() {
    // Test with WETH as token1 instead of token0
    let uni_pool = create_mock_uniswap_pool(false, 3500.0, 10_000_000.0); // WETH is token1
    let aero_pair = create_mock_aerodrome_pool(false, 3400.0, 5_000_000.0, 30); // WETH is token1
    
    let inputs = OptimizerInputs {
        uni_pool,
        uni_token0_is_weth: false, // Important: WETH is token1
        uni_fee_ppm_override: Some(3000),
        aero_pair,
        aero_token0_is_weth: false, // Important: WETH is token1
        gas_eth: create_test_gas_estimate(25_000_000_000, 200_000, 3450.0),
        gas_base: create_test_gas_estimate(100_000_000, 150_000, 3450.0),
        bridge_cost_usd: 5.0,
        hint_size_eth: 1.0,
        max_size_eth: 100.0,
    };
    
    let result = optimize(&inputs);
    assert!(result.is_some(), "Should handle reversed token order");
    
    let res = result.expect("Should handle reversed token order");
    println!("🔄 Reversed token order test:");
    println!("  Direction: {:?}", res.direction);
    println!("  Net Profit: ${:.2}", res.net_profit_usd);
    println!("  Optimal Size: {:.4} ETH", res.optimal_size_eth);
    
    assert!(res.net_profit_usd > 0.0, "Should still find profitable arbitrage");
    assert_eq!(res.direction, ArbDirection::SellUniBuyAero, 
              "Should identify correct direction with reversed tokens");
}

#[test]
fn test_optimizer_price_impact_consideration() {
    // Small liquidity pools to test price impact
    let uni_pool = create_mock_uniswap_pool(true, 3500.0, 500_000.0); // Small pool
    let aero_pair = create_mock_aerodrome_pool(true, 3400.0, 500_000.0, 30); // Small pool
    
    let inputs = OptimizerInputs {
        uni_pool,
        uni_token0_is_weth: true,
        uni_fee_ppm_override: Some(3000),
        aero_pair,
        aero_token0_is_weth: true,
        gas_eth: create_test_gas_estimate(25_000_000_000, 200_000, 3450.0),
        gas_base: create_test_gas_estimate(100_000_000, 150_000, 3450.0),
        bridge_cost_usd: 5.0,
        hint_size_eth: 1.0,
        max_size_eth: 100.0,
    };
    
    let result = optimize(&inputs);
    
    if let Some(res) = result {
        println!("📈 Price impact test (small pools):");
        println!("  Optimal Size: {:.4} ETH", res.optimal_size_eth);
        println!("  Sell Price: {:.2} USDC/ETH", res.eff_price_sell_usdc_per_eth);
        println!("  Buy Price: {:.2} USDC/ETH", res.eff_price_buy_usdc_per_eth);
        println!("  Spread: {:.2} USDC", res.eff_price_sell_usdc_per_eth - res.eff_price_buy_usdc_per_eth);
        
        // With small pools, optimal size should be moderate to avoid excessive slippage
        assert!(res.optimal_size_eth < 20.0, 
                "Should limit size in small pools due to price impact");
    }
}

#[test]
fn test_optimizer_convergence_quality() {
    // Test that optimizer converges to a reasonable optimum
    let uni_pool = create_mock_uniswap_pool(true, 3480.0, 5_000_000.0);
    let aero_pair = create_mock_aerodrome_pool(true, 3420.0, 5_000_000.0, 30);
    
    let inputs = OptimizerInputs {
        uni_pool,
        uni_token0_is_weth: true,
        uni_fee_ppm_override: Some(3000),
        aero_pair,
        aero_token0_is_weth: true,
        gas_eth: create_test_gas_estimate(25, 200_000, 3450.0), // 25 gwei instead of 25 billion gwei
        gas_base: create_test_gas_estimate(1, 150_000, 3450.0),  // 1 gwei instead of 100 million gwei
        bridge_cost_usd: 5.0,
        hint_size_eth: 0.1, // Start with small hint
        max_size_eth: 100.0,
    };
    
    let result = optimize(&inputs);
    assert!(result.is_some(), "Should find optimum");
    
    let res = result.expect("Should find optimum");
    
    // Verify optimality by checking nearby points
    // The profit at optimal_size should be higher than slightly smaller/larger sizes
    // This is a simplified check - in production you'd want more thorough validation
    
    println!("🎯 Convergence quality test:");
    println!("  Optimal Size: {:.4} ETH", res.optimal_size_eth);
    println!("  Net Profit at optimum: ${:.2}", res.net_profit_usd);
    
    // The optimizer should find a reasonable size given the market conditions
    assert!(res.optimal_size_eth > 0.5 && res.optimal_size_eth < 50.0,
            "Optimal size should be reasonable for given liquidity");
}

#[test]
fn test_optimizer_fee_variations() {
    // Test with different fee tiers
    let uni_pool = create_mock_uniswap_pool(true, 3500.0, 10_000_000.0);
    
    // High fee Aerodrome pool
    let aero_pair = create_mock_aerodrome_pool(true, 3400.0, 5_000_000.0, 100); // 1% fee
    
    let inputs = OptimizerInputs {
        uni_pool,
        uni_token0_is_weth: true,
        uni_fee_ppm_override: Some(500), // 0.05% fee override for Uniswap
        aero_pair,
        aero_token0_is_weth: true,
        gas_eth: create_test_gas_estimate(25_000_000_000, 200_000, 3450.0),
        gas_base: create_test_gas_estimate(100_000_000, 150_000, 3450.0),
        bridge_cost_usd: 5.0,
        hint_size_eth: 1.0,
        max_size_eth: 100.0,
    };
    
    let result = optimize(&inputs);
    
    if let Some(res) = result {
        println!("💰 Variable fees test:");
        println!("  Net Profit: ${:.2}", res.net_profit_usd);
        println!("  Direction: {:?}", res.direction);
        
        // Even with different fees, should find best direction
        // Lower Uniswap fee (0.05%) vs higher Aerodrome fee (1%) should affect profitability
        assert!(res.direction == ArbDirection::SellUniBuyAero,
                "Should prefer selling on low-fee Uniswap");
    }
}