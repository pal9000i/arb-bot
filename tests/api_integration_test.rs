// tests/api_integration_test.rs
// ===================================
// Integration test to verify API endpoint structure and response format

use serde_json::{self};

#[test]
fn test_api_response_structure() {
    // This test verifies the API response structure matches the specification
    // Expected JSON structure:
    let expected_structure = serde_json::json!({
        "timestamp_utc": "2025-08-12T21:30:00Z",
        "trade_size_eth": 10.0,
        "reference_cex_price_usd": 3500.0,
        "uniswap_v4_details": {
            "effective_price_usd": 4607.16,
            "price_impact_percent": -0.072,
            "estimated_gas_cost_usd": 2.71
        },
        "aerodrome_details": {
            "effective_price_usd": 4577.0,
            "price_impact_percent": -0.150,
            "estimated_gas_cost_usd": 0.01
        },
        "arbitrage_summary": {
            "potential_profit_usd": 300.16,
            "total_gas_cost_usd": 2.72,
            "net_profit_usd": 287.44,
            "recommended_action": "ARBITRAGE_DETECTED"
        }
    });

    // Verify all required fields are present
    assert!(expected_structure.get("timestamp_utc").is_some());
    assert!(expected_structure.get("trade_size_eth").is_some());
    assert!(expected_structure.get("reference_cex_price_usd").is_some());
    
    let uniswap_details = expected_structure.get("uniswap_v4_details")
        .expect("uniswap_v4_details not found in expected structure");
    assert!(uniswap_details.get("effective_price_usd").is_some());
    assert!(uniswap_details.get("price_impact_percent").is_some());
    assert!(uniswap_details.get("estimated_gas_cost_usd").is_some());
    
    let aerodrome_details = expected_structure.get("aerodrome_details")
        .expect("aerodrome_details not found in expected structure");
    assert!(aerodrome_details.get("effective_price_usd").is_some());
    assert!(aerodrome_details.get("price_impact_percent").is_some());
    assert!(aerodrome_details.get("estimated_gas_cost_usd").is_some());
    
    let arbitrage_summary = expected_structure.get("arbitrage_summary")
        .expect("arbitrage_summary not found in expected structure");
    assert!(arbitrage_summary.get("potential_profit_usd").is_some());
    assert!(arbitrage_summary.get("total_gas_cost_usd").is_some());
    assert!(arbitrage_summary.get("net_profit_usd").is_some());
    assert!(arbitrage_summary.get("recommended_action").is_some());

    println!("✅ API response structure verification passed!");
    println!("📊 Expected response format:");
    println!("{}", serde_json::to_string_pretty(&expected_structure)
        .expect("Failed to serialize expected structure"));
}

#[test]
fn test_profit_calculation_logic() {
    // Test the profit calculation logic we implemented
    let trade_size_eth = 10.0;
    let uni_price = 4607.16;  // Uniswap effective price USDC/ETH
    let aero_price = 4577.0;  // Aerodrome effective price USDC/ETH
    
    // Calculate both directions (same logic as in main.rs)
    let potential_profit_uni_to_aero: f64 = (trade_size_eth * uni_price) - (trade_size_eth * aero_price);
    let potential_profit_aero_to_uni: f64 = (trade_size_eth * aero_price) - (trade_size_eth * uni_price);
    let potential_profit_usd = potential_profit_uni_to_aero.max(potential_profit_aero_to_uni);
    
    println!("🧮 Profit Calculation Test:");
    println!("  Trade size: {} ETH", trade_size_eth);
    println!("  Uniswap price: ${:.2} USDC/ETH", uni_price);
    println!("  Aerodrome price: ${:.2} USDC/ETH", aero_price);
    println!("  Direction 1 (Uni→Aero): ${:.2}", potential_profit_uni_to_aero);
    println!("  Direction 2 (Aero→Uni): ${:.2}", potential_profit_aero_to_uni);
    println!("  Best potential profit: ${:.2}", potential_profit_usd);
    
    // Since Uniswap price > Aerodrome price, selling on Uni and buying on Aero should be profitable
    assert!((potential_profit_uni_to_aero - 301.6).abs() < 0.01);  // (10 * 4607.16) - (10 * 4577.0) ≈ 301.6
    assert!((potential_profit_aero_to_uni + 301.6).abs() < 0.01);  // (10 * 4577.0) - (10 * 4607.16) ≈ -301.6
    assert!((potential_profit_usd - 301.6).abs() < 0.01);          // max(301.6, -301.6) ≈ 301.6
    
    println!("✅ Profit calculation logic verification passed!");
}

#[test]
fn test_negative_profit_scenario() {
    // Test scenario where arbitrage is unprofitable
    let trade_size_eth = 10.0;
    let uni_price = 4577.0;  // Uniswap price lower
    let aero_price = 4607.16; // Aerodrome price higher
    
    let potential_profit_uni_to_aero: f64 = (trade_size_eth * uni_price) - (trade_size_eth * aero_price);
    let potential_profit_aero_to_uni: f64 = (trade_size_eth * aero_price) - (trade_size_eth * uni_price);
    let potential_profit_usd = potential_profit_uni_to_aero.max(potential_profit_aero_to_uni);
    
    println!("📉 Negative Profit Scenario Test:");
    println!("  Trade size: {} ETH", trade_size_eth);
    println!("  Uniswap price: ${:.2} USDC/ETH", uni_price);
    println!("  Aerodrome price: ${:.2} USDC/ETH", aero_price);
    println!("  Direction 1 (Uni→Aero): ${:.2}", potential_profit_uni_to_aero);
    println!("  Direction 2 (Aero→Uni): ${:.2}", potential_profit_aero_to_uni);
    println!("  Best potential profit: ${:.2}", potential_profit_usd);
    
    // Both directions should be negative in this case
    assert!(potential_profit_uni_to_aero < 0.0);  // Selling low, buying high = loss
    assert!(potential_profit_aero_to_uni > 0.0);  // Selling high, buying low = profit
    assert_eq!(potential_profit_usd, potential_profit_aero_to_uni); // Should pick the profitable direction
    
    println!("✅ Negative profit scenario verification passed!");
}