// tests/across_fees_integration.rs
// Integration tests for the Across protocol bridge fee estimation

use arrakis_arbitrage::chain::across_fees::{
    get_across_relay_fee, get_weth_fee_eth_to_base, get_weth_fee_base_to_eth,
    get_usdc_fee_eth_to_base, get_usdc_fee_base_to_eth,
    CHAIN_ID_ETHEREUM, CHAIN_ID_BASE, get_token_addresses, FeeDetail,
};
use ethers::types::U256;
use std::env;

// Set up environment variables for testing
fn setup_test_env() {
    env::set_var("ETH_WETH_ADDRESS", "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    env::set_var("BASE_WETH_ADDRESS", "0x4200000000000000000000000000000000000006");
    env::set_var("ETH_USDC_ADDRESS", "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606EB48");
    env::set_var("BASE_USDC_ADDRESS", "0x833589fCD6eDb6E08f4c7C32D4f71b54bDA02913");
    env::set_var("ACROSS_TIMEOUT_SECS", "15");
}

#[tokio::test]
async fn test_get_token_addresses() {
    setup_test_env();
    
    let addresses = get_token_addresses().unwrap();
    
    assert_eq!(addresses.weth_ethereum, "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    assert_eq!(addresses.weth_base, "0x4200000000000000000000000000000000000006");
    assert_eq!(addresses.usdc_ethereum, "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606EB48");
    assert_eq!(addresses.usdc_base, "0x833589fCD6eDb6E08f4c7C32D4f71b54bDA02913");
}

#[tokio::test]
async fn test_across_weth_bridge_fee_eth_to_base() {
    setup_test_env();
    
    // Test bridging 1 ETH from Ethereum to Base
    let amount_wei = "1000000000000000000"; // 1 ETH in wei
    
    let result = get_weth_fee_eth_to_base(amount_wei).await;
    
    match result {
        Ok(fees) => {
            println!("🌉 WETH Bridge Fee (ETH → Base):");
            println!("  Amount: {} wei (1 ETH)", amount_wei);
            println!("  Total Fee: {}", fees.total_relay_fee.total);
            if let Some(pct) = &fees.total_relay_fee.pct {
                println!("  Fee Percentage: {}%", pct);
            }
            
            // Test new conversion methods
            let fee_u256 = fees.total_relay_fee.total_as_u256().unwrap();
            assert!(fee_u256 > U256::zero(), "Fee should be greater than 0");
            
            // Convert to USD (assuming ETH price of $3500)
            let fee_usd = fees.total_relay_fee.total_in_usd(18, 3500.0).unwrap();
            println!("  Fee in USD: ${:.2}", fee_usd);
            assert!(fee_usd > 0.0, "USD fee should be positive");
        }
        Err(e) => {
            // Allow test to pass if API is unavailable but log the error
            eprintln!("⚠️  Across API unavailable (this is OK in CI): {}", e);
        }
    }
}

#[tokio::test]
async fn test_across_weth_bridge_fee_base_to_eth() {
    setup_test_env();
    
    // Test bridging 0.5 ETH from Base to Ethereum
    let amount_wei = "500000000000000000"; // 0.5 ETH in wei
    
    let result = get_weth_fee_base_to_eth(amount_wei).await;
    
    match result {
        Ok(fees) => {
            println!("🌉 WETH Bridge Fee (Base → ETH):");
            println!("  Amount: {} wei (0.5 ETH)", amount_wei);
            println!("  Total Fee: {}", fees.total_relay_fee.total);
            if let Some(pct) = &fees.total_relay_fee.pct {
                println!("  Fee Percentage: {}%", pct);
            }
            
            let fee_amount: Result<u128, _> = fees.total_relay_fee.total.parse();
            assert!(fee_amount.is_ok(), "Fee should be a valid number");
            assert!(fee_amount.unwrap() > 0, "Fee should be greater than 0");
        }
        Err(e) => {
            eprintln!("⚠️  Across API unavailable (this is OK in CI): {}", e);
        }
    }
}

#[tokio::test]
async fn test_across_usdc_bridge_fee_eth_to_base() {
    setup_test_env();
    
    // Test bridging 1000 USDC from Ethereum to Base
    let amount_usdc = "1000000000"; // 1000 USDC (6 decimals)
    
    let result = get_usdc_fee_eth_to_base(amount_usdc).await;
    
    match result {
        Ok(fees) => {
            println!("🌉 USDC Bridge Fee (ETH → Base):");
            println!("  Amount: {} (1000 USDC)", amount_usdc);
            println!("  Total Fee: {}", fees.total_relay_fee.total);
            if let Some(pct) = &fees.total_relay_fee.pct {
                println!("  Fee Percentage: {}%", pct);
            }
            
            let fee_u256 = fees.total_relay_fee.total_as_u256().unwrap();
            assert!(fee_u256 > U256::zero(), "Fee should be greater than 0");
            
            let fee_usd = fees.total_relay_fee.total_in_usd(6, 1.0).unwrap(); // USDC conversion
            println!("  Fee in USD: ${:.2}", fee_usd);
        }
        Err(e) => {
            eprintln!("⚠️  Across API unavailable (this is OK in CI): {}", e);
        }
    }
}

#[tokio::test]
async fn test_across_usdc_bridge_fee_base_to_eth() {
    setup_test_env();
    
    // Test bridging 500 USDC from Base to Ethereum
    let amount_usdc = "500000000"; // 500 USDC (6 decimals)
    
    let result = get_usdc_fee_base_to_eth(amount_usdc).await;
    
    match result {
        Ok(fees) => {
            println!("🌉 USDC Bridge Fee (Base → ETH):");
            println!("  Amount: {} (500 USDC)", amount_usdc);
            println!("  Total Fee: {}", fees.total_relay_fee.total);
            if let Some(pct) = &fees.total_relay_fee.pct {
                println!("  Fee Percentage: {}%", pct);
            }
            
            let fee_u256 = fees.total_relay_fee.total_as_u256().unwrap();
            assert!(fee_u256 > U256::zero(), "Fee should be greater than 0");
            
            let fee_usd = fees.total_relay_fee.total_in_usd(6, 1.0).unwrap();
            println!("  Fee in USD: ${:.2}", fee_usd);
        }
        Err(e) => {
            eprintln!("⚠️  Across API unavailable (this is OK in CI): {}", e);
        }
    }
}

#[tokio::test]
async fn test_across_direct_api_call() {
    setup_test_env();
    
    let addresses = get_token_addresses().unwrap();
    
    // Test direct API call with custom parameters
    let result = get_across_relay_fee(
        CHAIN_ID_ETHEREUM,
        CHAIN_ID_BASE,
        &addresses.weth_ethereum,
        &addresses.weth_base,
        "100000000000000000", // 0.1 ETH
    ).await;
    
    match result {
        Ok(fees) => {
            println!("🔗 Direct API Call Test:");
            println!("  Route: Ethereum → Base");
            println!("  Token: WETH");
            println!("  Amount: 0.1 ETH");
            println!("  Total Fee: {}", fees.total_relay_fee.total);
            
            // Validate response structure
            let fee_amount: Result<u128, _> = fees.total_relay_fee.total.parse();
            assert!(fee_amount.is_ok(), "Fee should be a valid number");
        }
        Err(e) => {
            eprintln!("⚠️  Across API unavailable (this is OK in CI): {}", e);
        }
    }
}

#[tokio::test]
async fn test_across_fee_scaling() {
    setup_test_env();
    
    let addresses = get_token_addresses().unwrap();
    
    // Test different amounts to see how fees scale
    let amounts = vec![
        ("0.1 ETH", "100000000000000000"),
        ("1.0 ETH", "1000000000000000000"),
        ("10.0 ETH", "10000000000000000000"),
    ];
    
    for (desc, amount_wei) in amounts {
        let result = get_across_relay_fee(
            CHAIN_ID_ETHEREUM,
            CHAIN_ID_BASE,
            &addresses.weth_ethereum,
            &addresses.weth_base,
            amount_wei,
        ).await;
        
        match result {
            Ok(fees) => {
                println!("📊 Fee Scaling Test - {}:", desc);
                println!("  Total Fee: {}", fees.total_relay_fee.total);
                if let Some(pct) = &fees.total_relay_fee.pct {
                    println!("  Fee Percentage: {}%", pct);
                }
                
                let fee_amount: Result<u128, _> = fees.total_relay_fee.total.parse();
                assert!(fee_amount.is_ok(), "Fee for {} should be valid", desc);
            }
            Err(e) => {
                eprintln!("⚠️  Fee scaling test failed for {} (API may be unavailable): {}", desc, e);
            }
        }
    }
}

#[tokio::test]
async fn test_environment_configuration() {
    // Test custom API URL configuration
    env::set_var("ACROSS_API_URL", "https://custom-across-api.example.com/fees");
    env::set_var("ACROSS_TIMEOUT_SECS", "5");
    setup_test_env();
    
    let addresses = get_token_addresses().unwrap();
    
    // This will likely fail due to custom URL, but tests the configuration path
    let result = get_across_relay_fee(
        CHAIN_ID_ETHEREUM,
        CHAIN_ID_BASE,
        &addresses.weth_ethereum,
        &addresses.weth_base,
        "1000000000000000000",
    ).await;
    
    // Should fail with network error due to fake URL, which confirms config is being used
    assert!(result.is_err(), "Should fail with custom fake URL");
    
    // Reset to default for other tests
    env::remove_var("ACROSS_API_URL");
    env::set_var("ACROSS_TIMEOUT_SECS", "15");
}

#[test]
fn test_fee_detail_u256_conversion() {
    // Test FeeDetail U256 conversion methods
    
    let fee = FeeDetail {
        total: "1500000000000000000".to_string(), // 1.5 ETH in wei
        pct: Some("0.05".to_string()),
    };
    
    // Test total_as_u256
    let amount_u256 = fee.total_as_u256().unwrap();
    assert_eq!(amount_u256, U256::from_dec_str("1500000000000000000").unwrap());
    
    // Test total_in_usd for WETH (18 decimals)
    let usd_amount = fee.total_in_usd(18, 3500.0).unwrap(); // $3500/ETH
    assert!((usd_amount - 5250.0).abs() < 0.01); // 1.5 ETH * $3500 = $5250
    
    println!("💰 Fee Detail Conversion Test:");
    println!("  Raw amount: {}", fee.total);
    println!("  U256 amount: {}", amount_u256);
    println!("  USD amount: ${:.2}", usd_amount);
}

#[test]
fn test_fee_detail_usdc_conversion() {
    // Test USDC conversion (6 decimals)
    
    let fee = FeeDetail {
        total: "5000000".to_string(), // 5 USDC (6 decimals)
        pct: None,
    };
    
    let amount_u256 = fee.total_as_u256().unwrap();
    assert_eq!(amount_u256, U256::from_dec_str("5000000").unwrap());
    
    // Test total_in_usd for USDC (6 decimals, ~$1 per USDC)
    let usd_amount = fee.total_in_usd(6, 1.0).unwrap();
    assert!((usd_amount - 5.0).abs() < 0.01); // 5 USDC * $1 = $5
    
    println!("💰 USDC Fee Conversion Test:");
    println!("  Raw amount: {}", fee.total);
    println!("  USD amount: ${:.2}", usd_amount);
}

#[test]
fn test_fee_detail_invalid_conversion() {
    // Test invalid conversion scenarios
    
    let invalid_fee = FeeDetail {
        total: "not_a_number".to_string(),
        pct: None,
    };
    
    // Should fail gracefully
    let result = invalid_fee.total_as_u256();
    assert!(result.is_err(), "Should fail for invalid number string");
    
    let usd_result = invalid_fee.total_in_usd(18, 3500.0);
    assert!(usd_result.is_err(), "USD conversion should also fail");
}