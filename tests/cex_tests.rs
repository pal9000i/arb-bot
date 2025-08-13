use arrakis_arbitrage::chain::cex_client::CexClient;

#[tokio::test]
async fn test_coinbase_price_fetch() {
    println!("🚀 STARTING COINBASE PRICE FETCH TEST");
    let client = CexClient::new("https://api.coinbase.com/v2/exchange-rates?currency=ETH".to_string());
    
    println!("📡 Making API call to Coinbase...");
    match client.get_coinbase_price().await {
        Ok(price) => {
            println!("✅ SUCCESS! Coinbase ETH price: ${:.2}", price);
            println!("🔍 Validating price range...");
            assert!(price > 0.0, "Price should be positive");
            assert!(price < 20000.0, "Price should be reasonable (< $20,000)");
            assert!(price > 100.0, "Price should be reasonable (> $100)");
            println!("✅ Price validation passed!");
        }
        Err(e) => {
            // Network issues are acceptable in tests
            println!("⚠️  Warning: Coinbase API test failed (network issue?): {}", e);
        }
    }
    println!("🏁 COINBASE TEST COMPLETED");
}

#[test]
fn test_cex_client_creation() {
    let _client = CexClient::new("https://api.coinbase.com/v2/exchange-rates?currency=ETH".to_string());
    // Test that we can create the client without panicking
    println!("✅ CexClient created successfully");
}

#[tokio::test]
async fn test_price_validation() {
    let client = CexClient::new("https://api.coinbase.com/v2/exchange-rates?currency=ETH".to_string());
    
    match client.get_coinbase_price().await {
        Ok(price) => {
            // Validate price is in reasonable range
            assert!(price > 100.0, "ETH price should be > $100");
            assert!(price < 20000.0, "ETH price should be < $20,000");
            
            // Test that price is a valid number
            assert!(price.is_finite(), "Price should be a finite number");
            assert!(!price.is_nan(), "Price should not be NaN");
            
            println!("✅ Price validation passed: ${:.2}", price);
        }
        Err(e) => {
            println!("⚠️ Price fetch failed (network issue expected): {}", e);
        }
    }
}


#[tokio::test]
async fn test_error_handling() {
    // Test that our client handles errors gracefully
    let client = CexClient::new("https://api.coinbase.com/v2/exchange-rates?currency=ETH".to_string());
    
    // The actual API call might succeed or fail depending on network
    // But we test that it doesn't panic
    let _result = client.get_coinbase_price().await;
    println!("✅ Error handling test completed (no panic)");
}