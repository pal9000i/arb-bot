use ethers::types::U256;
use reqwest::Client;
use serde::Deserialize;
use std::{env, time::Duration};

/// Ethereum & Base chain IDs
pub const CHAIN_ID_ETHEREUM: u64 = 1;
pub const CHAIN_ID_BASE: u64 = 8453;

#[derive(Debug)]
pub struct TokenAddresses {
    pub weth_ethereum: String,
    pub weth_base: String,
    pub usdc_ethereum: String,
    pub usdc_base: String,
}

#[derive(Debug, Deserialize)]
pub struct FeeDetail {
    pub total: String, // total fee in smallest units (wei or 6 decimals for USDC)
    #[serde(default)]
    #[allow(dead_code)] // Field from API response, may be used in future
    pub pct: Option<String>, // % fee (optional in API)
}

impl FeeDetail {
    /// Convert the `total` string into a U256
    pub fn total_as_u256(&self) -> Result<U256, Box<dyn std::error::Error + Send + Sync>> {
        Ok(U256::from_dec_str(&self.total)?)
    }

    /// Convert to USD given token decimals & live token USD price
    pub fn total_in_usd(
        &self,
        token_decimals: u32,
        token_price_usd: f64,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        let raw_amount = self.total_as_u256()?;
        let divisor = 10u128.pow(token_decimals);
        let amount_in_token = raw_amount.as_u128() as f64 / divisor as f64;
        Ok(amount_in_token * token_price_usd)
    }
}

#[derive(Debug, Deserialize)]
pub struct SuggestedFees {
    #[serde(rename = "totalRelayFee")]
    pub total_relay_fee: FeeDetail,
}

/// Load token addresses from environment variables
pub fn get_token_addresses() -> Result<TokenAddresses, Box<dyn std::error::Error + Send + Sync>> {
    Ok(TokenAddresses {
        weth_ethereum: env::var("ETH_WETH_ADDRESS")?,
        weth_base: env::var("BASE_WETH_ADDRESS")?,
        usdc_ethereum: env::var("ETH_USDC_ADDRESS")?,
        usdc_base: env::var("BASE_USDC_ADDRESS")?,
    })
}

/// Across API URL from env or default
fn get_across_api_url() -> String {
    env::var("ACROSS_API_URL")
        .unwrap_or_else(|_| "https://app.across.to/api/suggested-fees".to_string())
}

/// Fetch Across relay fee for given origin/dest & token addresses
pub async fn get_across_relay_fee(
    origin_chain: u64,
    dest_chain: u64,
    token_address_origin: &str,
    token_address_dest: &str,
    amount_smallest_unit: &str,
) -> Result<SuggestedFees, Box<dyn std::error::Error + Send + Sync>> {
    let timeout_secs = env::var("ACROSS_TIMEOUT_SECS")
        .unwrap_or_else(|_| "10".to_string())
        .parse()
        .unwrap_or(10);

    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let api_url = get_across_api_url();
    let url = format!(
        "{api}?inputToken={input}&outputToken={output}&originChainId={origin}&destinationChainId={dest}&amount={amount}",
        api = api_url,
        input = token_address_origin,
        output = token_address_dest,
        origin = origin_chain,
        dest = dest_chain,
        amount = amount_smallest_unit
    );

    let resp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<SuggestedFees>()
        .await?;

    Ok(resp)
}

/// WETH Ethereum → Base
pub async fn get_weth_fee_eth_to_base(amount_wei: &str) -> Result<SuggestedFees, Box<dyn std::error::Error + Send + Sync>> {
    let a = get_token_addresses()?;
    get_across_relay_fee(CHAIN_ID_ETHEREUM, CHAIN_ID_BASE, &a.weth_ethereum, &a.weth_base, amount_wei).await
}

/// WETH Base → Ethereum
pub async fn get_weth_fee_base_to_eth(amount_wei: &str) -> Result<SuggestedFees, Box<dyn std::error::Error + Send + Sync>> {
    let a = get_token_addresses()?;
    get_across_relay_fee(CHAIN_ID_BASE, CHAIN_ID_ETHEREUM, &a.weth_base, &a.weth_ethereum, amount_wei).await
}

/// USDC Ethereum → Base
pub async fn get_usdc_fee_eth_to_base(amount_6_dec: &str) -> Result<SuggestedFees, Box<dyn std::error::Error + Send + Sync>> {
    let a = get_token_addresses()?;
    get_across_relay_fee(CHAIN_ID_ETHEREUM, CHAIN_ID_BASE, &a.usdc_ethereum, &a.usdc_base, amount_6_dec).await
}

/// USDC Base → Ethereum
pub async fn get_usdc_fee_base_to_eth(amount_6_dec: &str) -> Result<SuggestedFees, Box<dyn std::error::Error + Send + Sync>> {
    let a = get_token_addresses()?;
    get_across_relay_fee(CHAIN_ID_BASE, CHAIN_ID_ETHEREUM, &a.usdc_base, &a.usdc_ethereum, amount_6_dec).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_detail_u256_conversion() {
        let fee = FeeDetail {
            total: "1000000000000000000".to_string(), // 1 ETH in wei
            pct: Some("0.05".to_string()),
        };

        let amount_u256 = fee.total_as_u256()
            .expect("Failed to convert fee to U256");
        assert_eq!(amount_u256, U256::from_dec_str("1000000000000000000")
            .expect("Failed to parse expected U256 value"));
    }

    #[test]
    fn test_fee_detail_usd_conversion() {
        // Test WETH conversion (18 decimals)
        let weth_fee = FeeDetail {
            total: "500000000000000000".to_string(), // 0.5 ETH in wei
            pct: None,
        };

        let usd_amount = weth_fee.total_in_usd(18, 3500.0)
            .expect("Failed to convert WETH fee to USD"); // $3500/ETH
        assert!((usd_amount - 1750.0).abs() < 0.01); // 0.5 ETH * $3500 = $1750

        // Test USDC conversion (6 decimals)
        let usdc_fee = FeeDetail {
            total: "5000000".to_string(), // 5 USDC
            pct: None,
        };

        let usdc_usd = usdc_fee.total_in_usd(6, 1.0)
            .expect("Failed to convert USDC fee to USD"); // $1/USDC
        assert!((usdc_usd - 5.0).abs() < 0.01); // 5 USDC * $1 = $5
    }

    #[test]
    fn test_chain_id_constants() {
        assert_eq!(CHAIN_ID_ETHEREUM, 1);
        assert_eq!(CHAIN_ID_BASE, 8453);
    }

    #[test]
    fn test_get_across_api_url() {
        // Test default URL when env var is not set
        std::env::remove_var("ACROSS_API_URL");
        let default_url = get_across_api_url();
        assert_eq!(default_url, "https://app.across.to/api/suggested-fees");

        // Test custom URL from env var
        std::env::set_var("ACROSS_API_URL", "https://custom-api.example.com/fees");
        let custom_url = get_across_api_url();
        assert_eq!(custom_url, "https://custom-api.example.com/fees");

        // Clean up
        std::env::remove_var("ACROSS_API_URL");
    }

    #[test]
    fn test_token_addresses_debug() {
        let addresses = TokenAddresses {
            weth_ethereum: "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".to_string(),
            weth_base: "0x4200000000000000000000000000000000000006".to_string(),
            usdc_ethereum: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
            usdc_base: "0x833589fCD6eDb6E08f4c7C32D4f71b54bDA02913".to_string(),
        };

        // Should be able to debug print
        let debug_str = format!("{:?}", addresses);
        assert!(debug_str.contains("TokenAddresses"));
        assert!(debug_str.contains("weth_ethereum"));
    }
}
