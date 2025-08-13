use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CoinbaseResponse {
    data: CoinbaseData,
}

#[derive(Debug, Deserialize)]
struct CoinbaseData {
    rates: std::collections::HashMap<String, String>,
}


pub struct CexClient {
    client: Client,
    api_url: String,
}

impl CexClient {
    pub fn new(api_url: String) -> Self {
        Self {
            client: Client::new(),
            api_url,
        }
    }


    pub async fn get_coinbase_price(&self) -> Result<f64> {
        let response: CoinbaseResponse = self.client
            .get(&self.api_url)
            .send()
            .await
            .context("Failed to fetch from Coinbase API")?
            .json()
            .await
            .context("Failed to parse Coinbase response")?;

        let usd_rate = response.data.rates.get("USD")
            .context("USD rate not found in Coinbase response")?;

        let price: f64 = usd_rate.parse()
            .context("Failed to parse USD rate as float")?;

        Ok(price)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cex_client_creation() {
        let api_url = "https://api.coinbase.com/v2/exchange-rates?currency=ETH".to_string();
        let client = CexClient::new(api_url.clone());

        // Verify the client is created with correct API URL
        assert_eq!(client.api_url, api_url);
    }

    #[test]
    fn test_coinbase_response_deserialization() {
        // Test deserialization of a valid Coinbase API response
        let json_response = r#"{
            "data": {
                "rates": {
                    "USD": "3456.78",
                    "EUR": "3123.45",
                    "GBP": "2789.12"
                }
            }
        }"#;

        let response: CoinbaseResponse = serde_json::from_str(json_response)
            .expect("Failed to deserialize Coinbase response");

        // Verify structure
        assert!(response.data.rates.contains_key("USD"));
        assert_eq!(response.data.rates.get("USD").expect("USD rate not found"), "3456.78");
    }

    #[test]
    fn test_price_parsing_logic() {
        // Test the price parsing logic used in get_coinbase_price
        let test_cases = vec![
            ("3456.78", 3456.78),
            ("0.123456", 0.123456),
            ("10000", 10000.0),
            ("0", 0.0),
        ];

        for (input, expected) in test_cases {
            let parsed: f64 = input.parse()
                .expect(&format!("Failed to parse input: {}", input));
            assert!((parsed - expected).abs() < 1e-10, "Failed for input: {}", input);
        }
    }
}