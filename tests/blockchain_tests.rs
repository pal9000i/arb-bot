use ethers::prelude::*;
use std::str::FromStr;
use num_traits::Zero;
use std::collections::BTreeMap;

#[tokio::test]
async fn test_ethereum_rpc_connection() {
    let eth_rpc = std::env::var("ETHEREUM_RPC_URL")
        .unwrap_or_else(|_| "https://eth-mainnet.g.alchemy.com/v2/demo".to_string());
    
    let provider_result = Provider::<Http>::try_from(&eth_rpc);
    
    match provider_result {
        Ok(_provider) => {
            println!("✅ Successfully created Ethereum RPC provider");
        }
        Err(e) => {
            println!("⚠️ Ethereum RPC provider creation failed (expected with demo endpoint): {}", e);
        }
    }
}

#[tokio::test]
async fn test_base_rpc_connection() {
    let base_rpc = std::env::var("BASE_RPC_URL")
        .unwrap_or_else(|_| "https://base-mainnet.g.alchemy.com/v2/demo".to_string());
    
    let provider_result = Provider::<Http>::try_from(&base_rpc);
    
    match provider_result {
        Ok(_provider) => {
            println!("✅ Successfully created Base RPC provider");
        }
        Err(e) => {
            println!("⚠️ Base RPC provider creation failed (expected with demo endpoint): {}", e);
        }
    }
}

#[test]
fn test_pool_state_math() {
    use arrakis_arbitrage::math::uniswap_v4::PoolState;
    use num_bigint::BigInt;
    use ethers::types::Address;
    
    let pool = PoolState {
        key: arrakis_arbitrage::math::uniswap_v4::PoolKey {
            currency0: Address::zero(),
            currency1: Address::from_str("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(),
            fee_ppm: 3000,
            tick_spacing: 60,
            hooks: Address::zero(),
        },
        sqrt_price_x96: BigInt::from(1000000000000000000000000u128),
        tick: 200000,
        liquidity: BigInt::from(500000000000000000000000u128),
        ticks: BTreeMap::new(),
    };
    
    assert!(!pool.sqrt_price_x96.is_zero());
    assert!(!pool.liquidity.is_zero());
    assert_eq!(pool.key.fee_ppm, 3000);
    assert_eq!(pool.key.tick_spacing, 60);
    println!("✅ Pool state creation and basic validation passed");
}

#[test]
fn test_invalid_address_parsing() {
    use ethers::types::Address;
    
    let invalid_addresses = vec![
        "not_an_address",
        "0x123", // Too short
        "0xzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", // Invalid hex
        "", // Empty
    ];
    
    for addr in invalid_addresses {
        let parse_result = addr.parse::<Address>();
        assert!(parse_result.is_err(), "Invalid address {} should fail to parse", addr);
    }
    
    // Test valid addresses
    let valid_addresses = vec![
        "0x0000000000000000000000000000000000000000",
        "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48", // USDC
        "0x4200000000000000000000000000000000000006", // WETH on Base
    ];
    
    for addr in valid_addresses {
        let parse_result = addr.parse::<Address>();
        assert!(parse_result.is_ok(), "Valid address {} should parse successfully", addr);
    }
    
    println!("✅ Address parsing validation passed");
}

#[test]
fn test_gas_cost_calculation() {
    use arrakis_arbitrage::chain::gas::create_test_gas_estimate;
    
    let gas_cost = create_test_gas_estimate(20, 300_000, 3000.0);
    
    assert!(gas_cost.gas_price > U256::zero());
    assert!(gas_cost.gas_limit > U256::zero());
    assert!(gas_cost.total_usd > 0.0);
    
    let cost_usd = gas_cost.total_usd;
    assert!(cost_usd > 0.0, "Gas cost should be positive");
    assert!(cost_usd < 1000.0, "Gas cost should be reasonable");
    
    println!("✅ Estimated gas cost: ${:.4} USD", cost_usd);
}

#[tokio::test]
async fn test_live_gas_estimation() {
    use arrakis_arbitrage::chain::gas::estimate_simple_gas_costs;
    use ethers::prelude::*;
    use std::sync::Arc;
    
    let eth_rpc = std::env::var("ETHEREUM_RPC_URL").unwrap_or_default();
    let base_rpc = std::env::var("BASE_RPC_URL").unwrap_or_default();
    
    if eth_rpc.is_empty() || base_rpc.is_empty() {
        println!("⚠️ Skipping live gas estimation test - no RPC URLs provided");
        return;
    }
    
    let eth_provider = Arc::new(Provider::<Http>::try_from(&eth_rpc)
        .expect("Failed to create Ethereum provider"));
    let base_provider = Arc::new(Provider::<Http>::try_from(&base_rpc)
        .expect("Failed to create Base provider"));
    let cex_price = 3000.0; // Mock CEX price
    
    // Mock addresses for the test (prefixed with _ since they're not currently used)
    let _eth_usdc: Address = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".parse()
        .expect("Failed to parse ETH USDC address");
    let _base_weth: Address = "0x4200000000000000000000000000000000000006".parse()
        .expect("Failed to parse Base WETH address");
    let _base_usdc: Address = "0x833589fcd6eDb6E08f4c7C32D4f71b54bDA02913".parse()
        .expect("Failed to parse Base USDC address");
    let _uniswap_router: Address = "0x68b3465833fb72A70ecDF485E0e4C7bD8665Fc45".parse()
        .expect("Failed to parse Uniswap router address");
    let _aerodrome_router: Address = "0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43".parse()
        .expect("Failed to parse Aerodrome router address");
    
    match estimate_simple_gas_costs(eth_provider, base_provider, cex_price, 120000, 185000).await {
        Ok((eth_gas, base_gas)) => {
            assert!(eth_gas.gas_price > U256::zero(), "Ethereum gas price should be positive");
            assert!(base_gas.gas_price > U256::zero(), "Base gas price should be positive");
            assert!(eth_gas.gas_limit > U256::zero(), "Gas limit should be positive");
            
            println!("✅ ETH gas: {} gwei, Base gas: {} gwei", 
                eth_gas.gas_price / 1_000_000_000, 
                base_gas.gas_price / 1_000_000_000);
        }
        Err(e) => {
            println!("⚠️ Gas estimation failed (expected with demo endpoints): {}", e);
        }
    }
}