// src/chain/gas.rs
//
// Fast gas estimation utilities for L1 Ethereum and Base (OP Stack).
// - Minimal RPCs
// - Parallel reads where possible
// - No heavy conversions

use ethers::prelude::*;
use std::sync::Arc;

/// OP Stack / Base GasPriceOracle predeploy (constant across OP chains)
pub const GAS_PRICE_ORACLE: &str = "0x420000000000000000000000000000000000000F";

abigen!(
    GasPriceOracle,
    r#"[
        function getL1Fee(bytes _data) view returns (uint256)
    ]"#
);

/// Result DTO for a single-chain gas estimate
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GasEstimate {
    pub gas_limit: U256,
    pub gas_price: U256,
    pub l1_data_fee: U256,
    pub total_wei: U256,
    pub total_eth: f64,
    pub total_usd: f64,
}

/// Fast convert U256 wei -> f64 ETH (lossy, for reporting)
#[inline]
fn wei_to_eth_f64_fast(v: U256) -> f64 {
    // Gas fees are well within u128 range.
    (v.as_u128() as f64) / 1e18
}

/// Estimate gas cost on **Ethereum L1** using predefined gas limit.
pub async fn estimate_eth_cost_usd(
    provider: Arc<Provider<Http>>,
    gas_units: u64,
    eth_price_usd: f64,
) -> Result<GasEstimate, Box<dyn std::error::Error + Send + Sync>> {
    let gas_limit = U256::from(gas_units);

    // Single lightweight RPC
    let gas_price = provider.get_gas_price().await?;

    let total_wei = gas_price.checked_mul(gas_limit).unwrap_or_default();
    let total_eth = wei_to_eth_f64_fast(total_wei);
    let total_usd = total_eth * eth_price_usd;

    Ok(GasEstimate {
        gas_limit,
        gas_price,
        l1_data_fee: U256::zero(),
        total_wei,
        total_eth,
        total_usd,
    })
}

/// Estimate gas cost on **Base (OP Stack L2)** using predefined gas limit.
pub async fn estimate_base_cost_usd(
    provider: Arc<Provider<Http>>,
    gas_units: u64,
    sample_calldata: &[u8],
    eth_price_usd: f64,
) -> Result<GasEstimate, Box<dyn std::error::Error + Send + Sync>> {
    let gas_limit = U256::from(gas_units);

    // Build oracle once
    let gpo_addr: Address = GAS_PRICE_ORACLE.parse()
        .map_err(|e| format!("Failed to parse gas price oracle address: {}", e))?;
    let gpo = GasPriceOracle::new(gpo_addr, provider.clone());

    // Run both reads IN PARALLEL: gas price + L1 data fee
    let gas_price_fut = async {
        provider
            .get_gas_price()
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
    };
    let l1_fee_fut = async {
        gpo.get_l1_fee(ethers::types::Bytes::from(sample_calldata.to_vec()))
            .call()
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
    };

    let (gas_price, l1_data_fee) = tokio::try_join!(gas_price_fut, l1_fee_fut)?;

    let l2_exec = gas_price.checked_mul(gas_limit).unwrap_or_default();
    let total_wei = l2_exec.checked_add(l1_data_fee).unwrap_or_default();
    let total_eth = wei_to_eth_f64_fast(total_wei);
    let total_usd = total_eth * eth_price_usd;

    Ok(GasEstimate {
        gas_limit,
        gas_price,
        l1_data_fee,
        total_wei,
        total_eth,
        total_usd,
    })
}

/// Simplified gas estimation that returns both ETH and Base estimates
pub async fn estimate_simple_gas_costs(
    eth_provider: Arc<Provider<Http>>,
    base_provider: Arc<Provider<Http>>,
    eth_price_usd: f64,
    gas_uniswap_units: u64,
    gas_aerodrome_units: u64,
) -> Result<(GasEstimate, GasEstimate), Box<dyn std::error::Error + Send + Sync>> {
    // Sample calldata for L1 fee estimation (typical swap calldata size)
    let sample_calldata = b"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

    let (eth_estimate, base_estimate) = tokio::try_join!(
        estimate_eth_cost_usd(eth_provider, gas_uniswap_units, eth_price_usd),
        estimate_base_cost_usd(base_provider, gas_aerodrome_units, sample_calldata, eth_price_usd),
    )?;

    Ok((eth_estimate, base_estimate))
}

/// Helper function to create test gas estimates
/// Available for all test contexts (unit tests and integration tests)
#[cfg(any(test, debug_assertions))]
pub fn create_test_gas_estimate(gas_price_gwei: u64, gas_limit: u64, eth_price_usd: f64) -> GasEstimate {
    let gas_price = U256::from(gas_price_gwei).checked_mul(U256::from(1_000_000_000u64)).unwrap_or_default(); // Convert gwei to wei
    let gas_limit_u256 = U256::from(gas_limit);
    let total_wei = gas_price.checked_mul(gas_limit_u256).unwrap_or_default();
    let total_eth = wei_to_eth_f64_fast(total_wei);
    let total_usd = total_eth * eth_price_usd;

    GasEstimate {
        gas_limit: gas_limit_u256,
        gas_price,
        l1_data_fee: U256::zero(),
        total_wei,
        total_eth,
        total_usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wei_to_eth_f64_fast() {
        // Test standard conversions
        let one_eth = U256::from_dec_str("1000000000000000000")
            .expect("Failed to parse one ETH amount");
        assert!((wei_to_eth_f64_fast(one_eth) - 1.0).abs() < 1e-10);

        let half_eth = U256::from_dec_str("500000000000000000")
            .expect("Failed to parse half ETH amount");
        assert!((wei_to_eth_f64_fast(half_eth) - 0.5).abs() < 1e-10);

        // Test zero
        assert_eq!(wei_to_eth_f64_fast(U256::zero()), 0.0);

        // Test small amounts
        let one_gwei = U256::from_dec_str("1000000000")
            .expect("Failed to parse one gwei amount");
        let expected = 1e-9;
        assert!((wei_to_eth_f64_fast(one_gwei) - expected).abs() < 1e-15);
    }

    #[test]
    fn test_create_test_gas_estimate() {
        let gas_price_gwei = 25; // 25 gwei
        let gas_limit = 200_000;
        let eth_price_usd = 3500.0;

        let estimate = create_test_gas_estimate(gas_price_gwei, gas_limit, eth_price_usd);

        // Verify gas limit
        assert_eq!(estimate.gas_limit, U256::from(gas_limit));

        // Verify gas price (25 gwei = 25 * 10^9 wei)
        let expected_gas_price = U256::from(25_000_000_000u64);
        assert_eq!(estimate.gas_price, expected_gas_price);

        // Verify L1 data fee is zero (for Ethereum)
        assert_eq!(estimate.l1_data_fee, U256::zero());

        // Verify total calculations
        let expected_total_wei = expected_gas_price * U256::from(gas_limit);
        assert_eq!(estimate.total_wei, expected_total_wei);

        // Verify ETH conversion
        let expected_eth = wei_to_eth_f64_fast(expected_total_wei);
        assert!((estimate.total_eth - expected_eth).abs() < 1e-10);

        // Verify USD conversion
        let expected_usd = expected_eth * eth_price_usd;
        assert!((estimate.total_usd - expected_usd).abs() < 1e-6);
    }

    #[test]
    fn test_gas_estimate_debug_clone() {
        let estimate = create_test_gas_estimate(20, 150_000, 3000.0);

        // Test Clone trait
        let cloned = estimate.clone();
        assert_eq!(estimate.gas_limit, cloned.gas_limit);
        assert_eq!(estimate.gas_price, cloned.gas_price);
        assert_eq!(estimate.l1_data_fee, cloned.l1_data_fee);
        assert_eq!(estimate.total_wei, cloned.total_wei);
        assert!((estimate.total_eth - cloned.total_eth).abs() < 1e-10);
        assert!((estimate.total_usd - cloned.total_usd).abs() < 1e-6);

        // Test Debug trait (should not panic)
        let debug_str = format!("{:?}", estimate);
        assert!(debug_str.contains("GasEstimate"));
        assert!(debug_str.len() > 0);
    }

    #[test]
    fn test_gas_estimate_calculations() {
        // Test with specific values that are easy to verify
        let gas_price_gwei = 50; // 50 gwei
        let gas_limit = 100_000; // 100k gas
        let eth_price_usd = 4000.0;

        let estimate = create_test_gas_estimate(gas_price_gwei, gas_limit, eth_price_usd);

        // Manual calculation:
        // gas_price = 50 * 10^9 wei = 50,000,000,000 wei
        // total_wei = 50,000,000,000 * 100,000 = 5,000,000,000,000,000 wei
        // total_eth = 5,000,000,000,000,000 / 10^18 = 0.005 ETH
        // total_usd = 0.005 * 4000 = $20

        assert_eq!(estimate.gas_price, U256::from(50_000_000_000u64));
        let expected_total_wei = U256::from_dec_str("5000000000000000")
            .expect("Failed to parse expected total wei");
        assert_eq!(estimate.total_wei, expected_total_wei);
        assert!((estimate.total_eth - 0.005).abs() < 1e-10);
        assert!((estimate.total_usd - 20.0).abs() < 1e-6);
    }

    #[test]
    fn test_gas_estimate_edge_cases() {
        // Test with zero gas price
        let estimate_zero_price = create_test_gas_estimate(0, 200_000, 3500.0);
        assert_eq!(estimate_zero_price.gas_price, U256::zero());
        assert_eq!(estimate_zero_price.total_wei, U256::zero());
        assert_eq!(estimate_zero_price.total_eth, 0.0);
        assert_eq!(estimate_zero_price.total_usd, 0.0);

        // Test with zero gas limit
        let estimate_zero_limit = create_test_gas_estimate(25, 0, 3500.0);
        assert_eq!(estimate_zero_limit.gas_limit, U256::zero());
        assert_eq!(estimate_zero_limit.total_wei, U256::zero());
        assert_eq!(estimate_zero_limit.total_eth, 0.0);
        assert_eq!(estimate_zero_limit.total_usd, 0.0);

        // Test with zero ETH price
        let estimate_zero_eth_price = create_test_gas_estimate(25, 200_000, 0.0);
        assert!(estimate_zero_eth_price.total_usd == 0.0);
    }

    #[test]
    fn test_gas_estimate_l1_fee_handling() {
        // Test that our helper function creates Ethereum-like estimates (no L1 fee)
        let estimate = create_test_gas_estimate(25, 200_000, 3500.0);
        // The helper function creates Ethereum-like estimates with zero L1 data fee
        assert_eq!(estimate.l1_data_fee, U256::zero());

        // Test that we can manually create a Base-like estimate
        let base_estimate = GasEstimate {
            gas_limit: U256::from(150_000),
            gas_price: U256::from(1_000_000_000u64), // 1 gwei
            l1_data_fee: U256::from_dec_str("50000000000000")
                .expect("Failed to parse L1 data fee"),
            total_wei: U256::from_dec_str("200000000000000")
                .expect("Failed to parse total wei"),
            total_eth: 0.0002,
            total_usd: 0.7,
        };

        assert!(base_estimate.l1_data_fee > U256::zero());
        assert!(base_estimate.total_wei > base_estimate.gas_price * base_estimate.gas_limit);
    }

    #[test]
    fn test_gas_price_oracle_constant() {
        // Verify the constant is correctly formatted
        assert_eq!(GAS_PRICE_ORACLE, "0x420000000000000000000000000000000000000F");

        // Should be a valid Ethereum address
        let parsed_address: Result<Address, _> = GAS_PRICE_ORACLE.parse();
        assert!(parsed_address.is_ok());

        let address = parsed_address.expect("Failed to parse gas price oracle address");
        assert_ne!(address, Address::zero());
    }

    #[test]
    fn test_gas_calculations_precision() {
        // Test calculations that might lose precision
        let gas_price_gwei = 123; // Odd number
        let gas_limit = 234_567; // Odd number
        let eth_price_usd = 3456.78; // Fractional price

        let estimate = create_test_gas_estimate(gas_price_gwei, gas_limit, eth_price_usd);

        // Verify internal consistency
        let manual_total_wei = estimate.gas_price * estimate.gas_limit;
        assert_eq!(estimate.total_wei, manual_total_wei);

        let manual_eth = wei_to_eth_f64_fast(estimate.total_wei);
        assert!((estimate.total_eth - manual_eth).abs() < 1e-12);

        let manual_usd = estimate.total_eth * eth_price_usd;
        assert!((estimate.total_usd - manual_usd).abs() < 1e-9);
    }

    #[test]
    fn test_realistic_gas_scenarios() {
        // Test scenarios based on real network conditions
        
        // Low gas scenario (off-peak hours)
        let low_gas = create_test_gas_estimate(15, 200_000, 3500.0);
        assert!(low_gas.total_usd < 20.0);

        // Medium gas scenario (normal hours)
        let med_gas = create_test_gas_estimate(50, 200_000, 3500.0);
        assert!(med_gas.total_usd > 20.0 && med_gas.total_usd < 50.0);

        // High gas scenario (network congestion)
        let high_gas = create_test_gas_estimate(200, 200_000, 3500.0);
        assert!(high_gas.total_usd > 100.0);

        // Complex DeFi transaction (higher gas limit)
        let complex_tx = create_test_gas_estimate(75, 500_000, 3500.0);
        assert!(complex_tx.total_usd > med_gas.total_usd);
        assert_eq!(complex_tx.gas_limit, U256::from(500_000));
    }

    #[test]
    fn test_overflow_protection() {
        // Test that our functions handle potential overflows gracefully
        use std::u64::MAX;

        // Very high gas price and limit that might cause overflow if not handled properly
        let estimate = create_test_gas_estimate(MAX / 1_000_000_000, MAX / 1000, 1.0);

        // Should not panic and should produce reasonable results
        assert!(estimate.gas_price > U256::zero());
        assert!(estimate.gas_limit > U256::zero());
        assert!(estimate.total_wei >= estimate.gas_price); // At minimum should be gas_price * 1
        assert!(estimate.total_eth >= 0.0);
        assert!(estimate.total_usd >= 0.0);
    }

    #[test]
    fn test_wei_to_eth_conversion_edge_cases() {
        // Test large U256 value (but not MAX to avoid overflow)
        let large_u256 = U256::from_dec_str("100000000000000000000000000000")
            .expect("Failed to parse large U256 value"); // 100B ETH
        let eth_value = wei_to_eth_f64_fast(large_u256);
        assert!(eth_value.is_finite());
        assert!(eth_value > 0.0);

        // Test various powers of 10
        let one_wei = U256::from(1);
        assert!((wei_to_eth_f64_fast(one_wei) - 1e-18).abs() < 1e-25);

        let one_ether = U256::from_dec_str("1000000000000000000")
            .expect("Failed to parse one ether");
        assert!((wei_to_eth_f64_fast(one_ether) - 1.0).abs() < 1e-10);
    }
}
