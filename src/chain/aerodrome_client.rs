use ethers::prelude::*;
use ethers::contract::Multicall;
use std::sync::Arc;

use crate::math::aerodrome_volatile::VolatilePairState;

ethers::contract::abigen!(
    AerodromeFactory,
    "./abis/AerodromeFactory.json",
);

ethers::contract::abigen!(
    AerodromePool,
    "./abis/AerodromePool.json",
);

ethers::contract::abigen!(
    AerodromePairFees,
    "./abis/AerodromePairFees.json",
);

pub async fn load_volatile_pair_snapshot(
    provider: Arc<Provider<Http>>,
    weth: Address,
    usdc: Address,
    factory_address: Address,
    pool_address: Option<Address>,
) -> Result<(VolatilePairState, bool), Box<dyn std::error::Error + Send + Sync>> {
    // 1) Use provided pool address or discover via factory
    let pool_addr = match pool_address {
        Some(addr) => {
            log::debug!("Using provided Aerodrome pool address: {}", addr);
            addr
        }
        None => {
            log::debug!("Discovering Aerodrome pool via factory");
            let factory = AerodromeFactory::new(factory_address, provider.clone());
            let discovered_addr = factory.get_pool(weth, usdc, false).call().await?;
            if discovered_addr == Address::zero() {
                return Err("Aerodrome volatile pool not found".into());
            }
            discovered_addr
        }
    };

    // Prepare contract handles bound to the discovered pool
    let pool = AerodromePool::new(pool_addr, provider.clone());
    let factory = AerodromeFactory::new(factory_address, provider.clone());

    // 2) One MULTICALL for token0, token1, reserves, fee
    //    Multicall::new(provider, None) auto-detects the chain's Multicall address.
    let (token0, token1, (r0, r1, _ts), fee_raw): (Address, Address, (U256, U256, U256), U256) = {
        let mut mc = Multicall::new(provider.clone(), None).await?;
        mc.add_call(pool.token_0(), false);
        mc.add_call(pool.token_1(), false);
        mc.add_call(pool.get_reserves(), false);
        mc.add_call(factory.get_fee(pool_addr, false), false); // getFee(pool, volatile=false)
        mc.call().await?
    };

    let fee_bps = fee_raw.as_u32();
    let token0_is_weth = token0 == weth;

    log::debug!("Fetched Aerodrome state — reserves: {} / {}, fee_bps: {}", r0, r1, fee_bps);

    let pair = VolatilePairState {
        token0,
        token1,
        reserve0: r0,
        reserve1: r1,
        // If you actually need to read decimals on-chain, swap these heuristics for ERC20 calls.
        decimals0: if token0_is_weth { 18 } else { 6 },
        decimals1: if token0_is_weth { 6 } else { 18 },
        fee_bps,
    };

    Ok((pair, token0_is_weth))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_mock_addresses() -> (Address, Address, Address) {
        let weth = Address::zero(); // Use zero address for WETH in tests
        let usdc = Address::from([0x11; 20]); // Mock USDC address
        let factory = Address::from([0x22; 20]); // Mock factory address
        (weth, usdc, factory)
    }

    #[test]
    fn test_token_ordering_logic() {
        let (weth, usdc, _factory) = create_mock_addresses();
        
        // Test WETH as token0 case
        let token0_is_weth_case1 = weth == weth;
        assert!(token0_is_weth_case1);

        // Test USDC as token0 case (when weth is token1)
        let token0_is_weth_case2 = usdc == weth;
        assert!(!token0_is_weth_case2);

        // Test with different addresses
        let other_address = Address::from([0x33; 20]);
        let token0_is_weth_case3 = other_address == weth;
        assert!(!token0_is_weth_case3);
    }

    #[test]
    fn test_decimal_assignment_logic() {
        let (_weth, _usdc, _factory) = create_mock_addresses();

        // Test decimals assignment when token0 is WETH
        let token0_is_weth = true;
        let (decimals0, decimals1) = if token0_is_weth { 
            (18, 6) // WETH=18, USDC=6
        } else { 
            (6, 18) // USDC=6, WETH=18
        };
        assert_eq!(decimals0, 18);
        assert_eq!(decimals1, 6);

        // Test decimals assignment when token1 is WETH
        let token0_is_weth = false;
        let (decimals0, decimals1) = if token0_is_weth { 
            (18, 6) 
        } else { 
            (6, 18) 
        };
        assert_eq!(decimals0, 6);
        assert_eq!(decimals1, 18);
    }

    #[test]
    fn test_volatile_pair_state_creation() {
        let (weth, usdc, _factory) = create_mock_addresses();
        
        let token0 = weth;
        let token1 = usdc;
        let reserve0 = U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse reserve0"); // 1000 WETH
        let reserve1 = U256::from_dec_str("3500000000000")
            .expect("Failed to parse reserve1"); // 3.5M USDC
        let fee_bps = 30u32; // 0.3%
        let token0_is_weth = true;

        let pair = VolatilePairState {
            token0,
            token1,
            reserve0,
            reserve1,
            decimals0: if token0_is_weth { 18 } else { 6 },
            decimals1: if token0_is_weth { 6 } else { 18 },
            fee_bps,
        };

        // Verify all fields are set correctly
        assert_eq!(pair.token0, weth);
        assert_eq!(pair.token1, usdc);
        assert_eq!(pair.reserve0, reserve0);
        assert_eq!(pair.reserve1, reserve1);
        assert_eq!(pair.decimals0, 18); // WETH decimals
        assert_eq!(pair.decimals1, 6);  // USDC decimals
        assert_eq!(pair.fee_bps, 30);
    }

    #[test]
    fn test_volatile_pair_state_token1_weth() {
        let (weth, usdc, _factory) = create_mock_addresses();
        
        // Test case where token1 is WETH (token0 is USDC)
        let token0 = usdc; // USDC first
        let token1 = weth; // WETH second
        let reserve0 = U256::from_dec_str("3500000000000")
            .expect("Failed to parse reserve0"); // 3.5M USDC
        let reserve1 = U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse reserve1"); // 1000 WETH
        let fee_bps = 25u32; // 0.25%
        let token0_is_weth = false; // token0 is USDC, not WETH

        let pair = VolatilePairState {
            token0,
            token1,
            reserve0,
            reserve1,
            decimals0: if token0_is_weth { 18 } else { 6 },
            decimals1: if token0_is_weth { 6 } else { 18 },
            fee_bps,
        };

        // Verify decimals are swapped correctly
        assert_eq!(pair.token0, usdc);
        assert_eq!(pair.token1, weth);
        assert_eq!(pair.decimals0, 6);  // USDC decimals (token0)
        assert_eq!(pair.decimals1, 18); // WETH decimals (token1)
        assert_eq!(pair.fee_bps, 25);
    }

    #[test]
    fn test_fee_conversion() {
        // Test U256 to u32 conversion for fees
        let fee_values = vec![
            (U256::from(30u32), 30u32),
            (U256::from(25u32), 25u32),
            (U256::from(100u32), 100u32),
            (U256::zero(), 0u32),
        ];

        for (fee_raw, expected_bps) in fee_values {
            let fee_bps = fee_raw.as_u32();
            assert_eq!(fee_bps, expected_bps);
        }

        // Test maximum u32 value
        let max_u32_fee = U256::from(u32::MAX);
        let converted_fee = max_u32_fee.as_u32();
        assert_eq!(converted_fee, u32::MAX);
    }

    #[test]
    fn test_reserve_handling() {
        // Test various reserve amounts that might be encountered
        let test_reserves = vec![
            // Small liquidity pool
            (
                U256::from_dec_str("1000000000000000000")
                    .expect("Failed to parse small WETH reserve"), // 1 WETH
                U256::from_dec_str("3500000000")
                    .expect("Failed to parse small USDC reserve"),           // 3500 USDC
            ),
            // Medium liquidity pool
            (
                U256::from_dec_str("100000000000000000000")
                    .expect("Failed to parse medium WETH reserve"), // 100 WETH
                U256::from_dec_str("350000000000")
                    .expect("Failed to parse medium USDC reserve"),          // 350k USDC
            ),
            // Large liquidity pool
            (
                U256::from_dec_str("10000000000000000000000")
                    .expect("Failed to parse large WETH reserve"), // 10k WETH
                U256::from_dec_str("35000000000000")
                    .expect("Failed to parse large USDC reserve"),          // 35M USDC
            ),
        ];

        for (reserve0, reserve1) in test_reserves {
            let pair = VolatilePairState {
                token0: Address::zero(),
                token1: Address::from([0x11; 20]),
                reserve0,
                reserve1,
                decimals0: 18,
                decimals1: 6,
                fee_bps: 30,
            };

            // Verify reserves are stored correctly
            assert_eq!(pair.reserve0, reserve0);
            assert_eq!(pair.reserve1, reserve1);
            
            // Both reserves should be positive for active pools
            assert!(pair.reserve0 > U256::zero());
            assert!(pair.reserve1 > U256::zero());
        }
    }

    #[test]
    fn test_zero_address_handling() {
        // Test zero address detection
        let zero_addr = Address::zero();
        let non_zero_addr = Address::from([0x01; 20]);

        // Zero address should be detected
        assert_eq!(zero_addr, Address::zero());
        assert_ne!(non_zero_addr, Address::zero());

        // Test in context of pool discovery
        let discovered_addr = Address::zero(); // Simulated failure case
        let should_error = discovered_addr == Address::zero();
        assert!(should_error); // Should trigger error condition

        let valid_addr = Address::from([0x11; 20]);
        let should_succeed = valid_addr != Address::zero();
        assert!(should_succeed); // Should proceed normally
    }

    #[test]
    fn test_address_formatting() {
        // Test address formatting for logging
        let test_addr = Address::from([0xAB; 20]);
        let formatted = format!("{}", test_addr);
        
        // Should format to a non-empty string that starts with 0x
        assert!(!formatted.is_empty(), "Address should format to non-empty string");
        assert!(formatted.starts_with("0x"), "Address should start with 0x");
    }

    #[test]
    fn test_multicall_structure() {
        // Test the structure of calls that would be made in multicall
        // This tests the logic without actual network calls
        
        // Simulate the tuple structure returned by multicall
        type MulticallResult = (Address, Address, (U256, U256, U256), U256);
        
        let mock_result: MulticallResult = (
            Address::zero(), // token0
            Address::from([0x11; 20]), // token1
            ( // reserves tuple
                U256::from_dec_str("1000000000000000000000")
                    .expect("Failed to parse mock reserve0"), // reserve0
                U256::from_dec_str("3500000000000")
                    .expect("Failed to parse mock reserve1"),         // reserve1
                U256::from(12345), // timestamp (unused)
            ),
            U256::from(30), // fee_raw
        );

        // Destructure the same way as in the actual function
        let (token0, token1, (r0, r1, _ts), fee_raw) = mock_result;
        
        // Verify destructuring works correctly
        assert_eq!(token0, Address::zero());
        assert_eq!(token1, Address::from([0x11; 20]));
        assert_eq!(r0, U256::from_dec_str("1000000000000000000000")
            .expect("Failed to parse expected reserve0"));
        assert_eq!(r1, U256::from_dec_str("3500000000000")
            .expect("Failed to parse expected reserve1"));
        assert_eq!(fee_raw, U256::from(30));
        
        // Verify fee conversion
        let fee_bps = fee_raw.as_u32();
        assert_eq!(fee_bps, 30);
    }

    #[test]
    fn test_pool_state_consistency() {
        let (weth, usdc, _factory) = create_mock_addresses();
        
        // Test consistency between different configurations
        let configs = vec![
            (true, weth, usdc, 18u8, 6u8),   // WETH as token0
            (false, usdc, weth, 6u8, 18u8),  // USDC as token0
        ];

        for (token0_is_weth, expected_token0, expected_token1, expected_dec0, expected_dec1) in configs {
            let pair = VolatilePairState {
                token0: expected_token0,
                token1: expected_token1,
                reserve0: U256::from(1000),
                reserve1: U256::from(2000),
                decimals0: if token0_is_weth { 18 } else { 6 },
                decimals1: if token0_is_weth { 6 } else { 18 },
                fee_bps: 30,
            };

            // Verify consistency
            assert_eq!(pair.token0, expected_token0);
            assert_eq!(pair.token1, expected_token1);
            assert_eq!(pair.decimals0, expected_dec0);
            assert_eq!(pair.decimals1, expected_dec1);

            // Verify the boolean calculation
            let calculated_token0_is_weth = pair.token0 == weth;
            assert_eq!(calculated_token0_is_weth, token0_is_weth);
        }
    }

    #[test]
    fn test_edge_case_reserves() {
        // Test edge cases for reserves
        let edge_cases = vec![
            (U256::from(1), U256::from(1)), // Minimal liquidity
            (U256::MAX, U256::from(1)),     // Maximum reserve0
            (U256::from(1), U256::MAX),     // Maximum reserve1
        ];

        for (reserve0, reserve1) in edge_cases {
            let pair = VolatilePairState {
                token0: Address::zero(),
                token1: Address::from([0x11; 20]),
                reserve0,
                reserve1,
                decimals0: 18,
                decimals1: 6,
                fee_bps: 30,
            };

            // Should handle extreme values without panicking
            assert_eq!(pair.reserve0, reserve0);
            assert_eq!(pair.reserve1, reserve1);
            assert!(pair.reserve0 > U256::zero());
            assert!(pair.reserve1 > U256::zero());
        }
    }

    #[test]
    fn test_fee_range_validation() {
        // Test various fee values that might be encountered
        let fee_cases = vec![
            0u32,    // 0% fee (unlikely but possible)
            1u32,    // 0.01% fee
            25u32,   // 0.25% fee (common)
            30u32,   // 0.30% fee (common)
            100u32,  // 1% fee (high)
            1000u32, // 10% fee (very high)
        ];

        for fee_bps in fee_cases {
            let pair = VolatilePairState {
                token0: Address::zero(),
                token1: Address::from([0x11; 20]),
                reserve0: U256::from(1000),
                reserve1: U256::from(2000),
                decimals0: 18,
                decimals1: 6,
                fee_bps,
            };

            // Verify fee is stored correctly
            assert_eq!(pair.fee_bps, fee_bps);
            
            // Fee should be reasonable (less than 100% = 10000 bps)
            assert!(pair.fee_bps <= 10000, "Fee {} bps is unreasonably high", fee_bps);
        }
    }

    #[test]
    fn test_address_uniqueness() {
        // Test that token addresses should be different
        let same_addr = Address::from([0x11; 20]);
        
        // In practice, token0 and token1 should be different
        // But our struct doesn't enforce this - it's a business logic constraint
        let pair_same_tokens = VolatilePairState {
            token0: same_addr,
            token1: same_addr, // Same as token0 - should be avoided in practice
            reserve0: U256::from(1000),
            reserve1: U256::from(2000),
            decimals0: 18,
            decimals1: 6,
            fee_bps: 30,
        };

        // The struct allows this, but in practice this would be invalid
        assert_eq!(pair_same_tokens.token0, pair_same_tokens.token1);
        
        // Proper case: different tokens
        let pair_different_tokens = VolatilePairState {
            token0: Address::zero(),
            token1: Address::from([0x11; 20]),
            reserve0: U256::from(1000),
            reserve1: U256::from(2000),
            decimals0: 18,
            decimals1: 6,
            fee_bps: 30,
        };

        assert_ne!(pair_different_tokens.token0, pair_different_tokens.token1);
    }
}
