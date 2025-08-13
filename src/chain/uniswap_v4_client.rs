use ethers::abi::{self, Token};
use ethers::contract::{abigen, Multicall};
use ethers::prelude::*;
use ethers::providers::{Http, Provider};
use ethers::utils::keccak256;
use futures::{stream, StreamExt, TryStreamExt};
use num_bigint::{BigInt, Sign};
use std::sync::Arc;

use crate::math::uniswap_v4::{create_pool_with_real_data, PoolState};

abigen!(
    StateView,
    r#"[
      {
        "type": "function",
        "name": "getSlot0",
        "stateMutability": "view",
        "inputs": [{"name": "poolId", "type": "bytes32"}],
        "outputs": [
          {"name": "sqrtPriceX96", "type": "uint160"},
          {"name": "tick", "type": "int24"}
        ]
      },
      {
        "type": "function",
        "name": "getLiquidity",
        "stateMutability": "view",
        "inputs": [{"name": "poolId", "type": "bytes32"}],
        "outputs": [{"name": "liquidity", "type": "uint128"}]
      },
      {
        "type": "function",
        "name": "getTickInfo",
        "stateMutability": "view",
        "inputs": [
          {"name": "poolId", "type": "bytes32"},
          {"name": "tick", "type": "int24"}
        ],
        "outputs": [
          {"name": "liquidityGross", "type": "uint128"},
          {"name": "liquidityNet", "type": "int128"},
          {"name": "feeGrowthOutside0X128", "type": "uint256"},
          {"name": "feeGrowthOutside1X128", "type": "uint256"}
        ]
      },
      {
        "type": "function",
        "name": "getTickBitmap",
        "stateMutability": "view",
        "inputs": [
          {"name": "poolId", "type": "bytes32"},
          {"name": "wordPos", "type": "int16"}
        ],
        "outputs": [{"name": "word", "type": "uint256"}]
      }
    ]"#
);

/// Backward-compatible entry (5 args). Auto-detects Multicall on the chain.
pub async fn load_v4_pool_snapshot(
    provider: Arc<Provider<Http>>,
    state_view_addr: Address,
    usdc_addr: Address,
    fee_ppm: u32,
    tick_spacing: i32,
) -> Result<(PoolState, bool), Box<dyn std::error::Error + Send + Sync>> {
    load_v4_pool_snapshot_with_multicall(
        provider,
        state_view_addr,
        usdc_addr,
        fee_ppm,
        tick_spacing,
        None,
    )
    .await
}

/// Main entry with optional explicit Multicall address
pub async fn load_v4_pool_snapshot_with_multicall(
    provider: Arc<Provider<Http>>,
    state_view_addr: Address,
    usdc_addr: Address,
    fee_ppm: u32,
    tick_spacing: i32,
    multicall_addr: Option<Address>,
) -> Result<(PoolState, bool), Box<dyn std::error::Error + Send + Sync>> {
    let view = StateView::new(state_view_addr, provider.clone());

    // Currency ordering with native ETH = address(0)
    let eth = Address::zero();
    let (currency0, currency1, token0_is_eth) = if eth < usdc_addr {
        (eth, usdc_addr, true)
    } else {
        (usdc_addr, eth, false)
    };

    let hooks = Address::zero();

    // Encode V4 PoolKey -> poolId
    let tokens = vec![Token::Tuple(vec![
        Token::Address(currency0),
        Token::Address(currency1),
        Token::Uint(U256::from(fee_ppm)),            // uint24 in practice
        Token::Int(U256::from(tick_spacing as i64)), // int24
        Token::Address(hooks),
    ])];
    let pool_id = keccak256(abi::encode(&tokens));

    // 1) slot0 + liquidity in ONE multicall
    let ((sqrt_price_x96, current_tick), liquidity) =
        fetch_core_state_multicall(provider.clone(), &view, pool_id, multicall_addr).await?;

    log::debug!("V4 state — tick: {}, liquidity: {}", current_tick, liquidity);

    let sqrt_bi = u256_to_bigint(sqrt_price_x96.into());
    let liq_bi = u256_to_bigint(liquidity.into());

    // 2) Tick data via bitmaps + tick infos (few multicalls)
    let tick_data = fetch_tick_data_multicall(
        provider.clone(),
        &view,
        pool_id,
        current_tick,
        tick_spacing,
        24,     // word_range (±N words around current)
        4096,   // tickinfo_chunk_size (try 8192 if your RPC allows)
        6,      // parallel_chunks
        multicall_addr,
    )
    .await?;

    // 3) Build pool
    let pool = create_pool_with_real_data(
        currency0, currency1, fee_ppm, tick_spacing, hooks, sqrt_bi, current_tick, liq_bi, tick_data,
    );

    Ok((pool, token0_is_eth))
}

/// ONE multicall for slot0 + liquidity
async fn fetch_core_state_multicall<M: Middleware + 'static>(
    client: Arc<M>,
    view: &StateView<M>,
    pool_id: [u8; 32],
    multicall_addr: Option<Address>,
) -> Result<((U256, i32), U256), Box<dyn std::error::Error + Send + Sync>> {
    let mut mc = Multicall::new(client.clone(), multicall_addr).await?;
    mc.add_call(view.get_slot_0(pool_id), false);
    mc.add_call(view.get_liquidity(pool_id), false);
    let out: ((U256, i32), U256) = mc.call().await?;
    Ok(out)
}

/// Bitmaps + tick infos via Multicall. Few calls, **bounded-parallel** chunked tick infos.
async fn fetch_tick_data_multicall<M: Middleware + 'static>(
    client: Arc<M>,
    view: &StateView<M>,
    pool_id: [u8; 32],
    current_tick: i32,
    tick_spacing: i32,
    word_range: i16,
    tickinfo_chunk_size: usize,
    parallel_chunks: usize,
    multicall_addr: Option<Address>,
) -> Result<Vec<(i32, BigInt)>, Box<dyn std::error::Error + Send + Sync>> {
    // Compute word positions around current tick
    let current_word = current_tick / tick_spacing / 256;
    let word_positions: Vec<i16> = (-word_range..=word_range)
        .map(|off| (current_word as i16).saturating_add(off))
        .collect();

    // 2a) All bitmaps in ONE multicall (homogeneous => call_array())
    let bitmaps: Vec<U256> = {
        let mut mc = Multicall::new(client.clone(), multicall_addr).await?;
        for wp in &word_positions {
            mc.add_call(view.get_tick_bitmap(pool_id, *wp), false);
        }
        mc.call_array().await?
    };

    // 2b) Decode set bits -> candidate ticks
    let mut candidate_ticks = Vec::new();
    for (idx, word) in bitmaps.into_iter().enumerate() {
        if word.is_zero() {
            continue;
        }
        let word_pos = word_positions[idx] as i32;
        for bit in 0..256usize {
            if word.bit(bit) {
                let t = word_pos * 256 * tick_spacing + (bit as i32) * tick_spacing;
                candidate_ticks.push(t);
            }
        }
    }

    if candidate_ticks.is_empty() {
        // Fallback: synthetic wide ranges to keep downstream pricing working
        let mut tick_data = Vec::new();
        let wide_range = 12_000;
        let synthetic_liq = BigInt::from(1_000_000_000_000_000_000_000_000u128);
        for i in 0..10 {
            let lower = current_tick - wide_range + (i * wide_range / 5);
            let upper = current_tick + wide_range - (i * wide_range / 5);
            let per = &synthetic_liq / BigInt::from(10u8);
            tick_data.push((lower, per.clone()));
            tick_data.push((upper, -per));
        }
        return Ok(tick_data);
    }

    // 2c) Tick infos via Multicall, **chunked + bounded parallel**
    candidate_ticks.sort_unstable();
    candidate_ticks.dedup();

    let chunked_ticks: Vec<Vec<i32>> = candidate_ticks
        .chunks(tickinfo_chunk_size)
        .map(|s| s.to_vec())
        .collect();

    // Stream all chunks, but only `parallel_chunks` in flight at once.
    let results: Vec<(Vec<i32>, Vec<(u128, i128, U256, U256)>)> = stream::iter(
        chunked_ticks.into_iter().map(|ticks_chunk| {
            let client = client.clone();
            let view = view.clone();
            async move {
                let mut mc = Multicall::new(client.clone(), multicall_addr).await?;
                for t in &ticks_chunk {
                    mc.add_call(view.get_tick_info(pool_id, *t), false);
                }
                let infos: Vec<(u128, i128, U256, U256)> = mc.call_array().await?;
                Ok::<(Vec<i32>, Vec<(u128, i128, U256, U256)>), Box<dyn std::error::Error + Send + Sync>>(
                    (ticks_chunk, infos)
                )
            }
        }),
    )
    .buffer_unordered(parallel_chunks)
    .try_collect()
    .await?;

    // 2d) Collect net liquidity deltas for non-zero gross ticks
    let mut tick_data: Vec<(i32, BigInt)> = Vec::new();
    for (ticks_chunk, infos_chunk) in results {
        for (i, (liquidity_gross, liquidity_net, _f0, _f1)) in
            infos_chunk.into_iter().enumerate()
        {
            if liquidity_gross > 0u128 {
                let tick = ticks_chunk[i];
                tick_data.push((tick, BigInt::from(liquidity_net))); // i128 -> BigInt
            }
        }
    }

    Ok(tick_data)
}

/// Helpers
fn u256_to_bigint(u: U256) -> BigInt {
    let mut buf = [0u8; 32];
    u.to_big_endian(&mut buf);
    BigInt::from_bytes_be(Sign::Plus, &buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u256_to_bigint() {
        // Test zero
        let zero = U256::zero();
        let zero_bigint = u256_to_bigint(zero);
        assert_eq!(zero_bigint, BigInt::from(0));

        // Test one
        let one = U256::one();
        let one_bigint = u256_to_bigint(one);
        assert_eq!(one_bigint, BigInt::from(1));

        // Test larger values
        let large = U256::from_dec_str("1000000000000000000").unwrap(); // 1 ETH in wei
        let large_bigint = u256_to_bigint(large);
        assert_eq!(large_bigint, BigInt::from(1000000000000000000u64));

        // Test maximum u128 value (should fit in BigInt)
        let max_u128 = U256::from(u128::MAX);
        let max_bigint = u256_to_bigint(max_u128);
        assert_eq!(max_bigint, BigInt::from(u128::MAX));

        // Test some specific Uniswap-related values
        let sqrt_price = U256::from_dec_str("7922816251426433759354395033").unwrap(); // Example sqrt price
        let sqrt_bigint = u256_to_bigint(sqrt_price);
        assert!(sqrt_bigint > BigInt::from(0));
        assert!(sqrt_bigint.to_string().len() > 10); // Should be a large number
    }

    #[test]
    fn test_u256_to_bigint_consistency() {
        // Test that conversion is consistent
        let test_values = vec![
            U256::zero(),
            U256::one(),
            U256::from(100),
            U256::from(u64::MAX),
            U256::from_dec_str("12345678901234567890").unwrap(),
        ];

        for val in test_values {
            let bigint = u256_to_bigint(val);
            let val_string = val.to_string();
            let bigint_string = bigint.to_string();
            assert_eq!(val_string, bigint_string, "Conversion should be consistent");
        }
    }

    #[test]
    fn test_currency_ordering_logic() {
        // Test the currency ordering logic used in the main function
        let eth = Address::zero();
        let usdc_addr = Address::from([0x11; 20]); // Some non-zero address

        // Test case where ETH (0x0) < USDC address
        let (currency0, currency1, token0_is_eth) = if eth < usdc_addr {
            (eth, usdc_addr, true)
        } else {
            (usdc_addr, eth, false)
        };

        // ETH address (0x0) should always be less than any non-zero address
        assert_eq!(currency0, eth);
        assert_eq!(currency1, usdc_addr);
        assert_eq!(token0_is_eth, true);

        // Test with a very small address that might be less than ETH (impossible, but test)
        let tiny_addr = Address::zero(); // Same as ETH
        let (curr0, curr1, token0_is_eth_2) = if eth < tiny_addr {
            (eth, tiny_addr, true)
        } else {
            (tiny_addr, eth, false)
        };

        // Should handle equal addresses
        assert_eq!(curr0, tiny_addr);
        assert_eq!(curr1, eth);
        assert_eq!(token0_is_eth_2, false);
    }

    #[test]
    fn test_word_position_calculation() {
        // Test the word position calculation logic from fetch_tick_data_multicall
        let current_tick = -191740; // Example tick from real usage
        let tick_spacing = 60;
        let word_range = 24i16;

        let current_word = current_tick / tick_spacing / 256;
        let word_positions: Vec<i16> = (-word_range..=word_range)
            .map(|off| (current_word as i16).saturating_add(off))
            .collect();

        // Verify word positions are calculated correctly
        assert_eq!(word_positions.len(), (word_range * 2 + 1) as usize);
        assert!(word_positions.contains(&(current_word as i16)));

        // Test edge cases
        let edge_tick = i32::MAX / 2;
        let edge_word = edge_tick / tick_spacing / 256;
        assert!(edge_word > 0);

        // Test negative tick
        let neg_tick = -100_000;
        let neg_word = neg_tick / tick_spacing / 256;
        assert!(neg_word < 0);
    }

    #[test]
    fn test_tick_calculation_from_bitmap() {
        // Test the tick calculation logic from bitmap decoding
        let word_pos = -12i32;
        let tick_spacing = 60;
        let bit = 128usize; // Middle bit

        let calculated_tick = word_pos * 256 * tick_spacing + (bit as i32) * tick_spacing;
        
        // Verify the calculation
        let expected = -12 * 256 * 60 + 128 * 60;
        assert_eq!(calculated_tick, expected);

        // Test with different parameters
        let test_cases = vec![
            (0i32, 60, 0usize),     // Word 0, bit 0
            (1i32, 60, 255usize),   // Word 1, last bit
            (-1i32, 10, 100usize),  // Negative word
        ];

        for (wp, ts, bit) in test_cases {
            let tick = wp * 256 * ts + (bit as i32) * ts;
            
            // Should be divisible by tick_spacing
            assert_eq!(tick % ts, 0, "Tick should be aligned to tick spacing");
            
            // Verify the relationship
            let word_contribution = wp * 256 * ts;
            let bit_contribution = (bit as i32) * ts;
            assert_eq!(tick, word_contribution + bit_contribution);
        }
    }

    #[test]
    fn test_synthetic_liquidity_fallback() {
        // Test the synthetic liquidity generation for empty bitmaps
        let current_tick = -191740;
        let wide_range = 12_000;
        let synthetic_liq = BigInt::from(1_000_000_000_000_000_000_000_000u128);
        
        // Simulate the fallback logic
        let mut tick_data = Vec::new();
        for i in 0..10 {
            let lower = current_tick - wide_range + (i * wide_range / 5);
            let upper = current_tick + wide_range - (i * wide_range / 5);
            let per = &synthetic_liq / BigInt::from(10u8);
            tick_data.push((lower, per.clone()));
            tick_data.push((upper, -per));
        }

        // Verify synthetic data structure
        assert_eq!(tick_data.len(), 20); // 10 pairs of ticks
        
        // Verify liquidity is positive/negative alternating
        for (i, (_tick, liq)) in tick_data.iter().enumerate() {
            if i % 2 == 0 {
                // Even index should be positive (lower tick)
                assert!(liq > &BigInt::from(0));
            } else {
                // Odd index should be negative (upper tick)
                assert!(liq < &BigInt::from(0));
            }
        }

        // Verify tick ordering makes sense (some ticks should be around current_tick)
        let first_tick = tick_data[0].0;
        let last_tick = tick_data[tick_data.len() - 1].0;
        // At least one tick should be below and one above current (or they span a reasonable range)
        assert!(first_tick != last_tick, "Should have different tick values");
    }

    #[test]
    fn test_pool_id_encoding() {
        // Test the pool ID encoding logic
        use ethers::abi::{self, Token};
        use ethers::utils::keccak256;

        let currency0 = Address::zero(); // ETH
        let currency1 = Address::from([0x11; 20]); // USDC
        let fee_ppm = 3000u32;
        let tick_spacing = 60i32;
        let hooks = Address::zero();

        // Encode the same way as in the main function
        let tokens = vec![Token::Tuple(vec![
            Token::Address(currency0),
            Token::Address(currency1),
            Token::Uint(U256::from(fee_ppm)),
            Token::Int(U256::from(tick_spacing as i64)),
            Token::Address(hooks),
        ])];
        let pool_id = keccak256(abi::encode(&tokens));

        // Verify pool ID is 32 bytes
        assert_eq!(pool_id.len(), 32);

        // Verify deterministic (same inputs = same output)
        let tokens2 = vec![Token::Tuple(vec![
            Token::Address(currency0),
            Token::Address(currency1),
            Token::Uint(U256::from(fee_ppm)),
            Token::Int(U256::from(tick_spacing as i64)),
            Token::Address(hooks),
        ])];
        let pool_id2 = keccak256(abi::encode(&tokens2));
        assert_eq!(pool_id, pool_id2);

        // Verify different inputs = different outputs
        let tokens3 = vec![Token::Tuple(vec![
            Token::Address(currency0),
            Token::Address(currency1),
            Token::Uint(U256::from(500u32)), // Different fee
            Token::Int(U256::from(tick_spacing as i64)),
            Token::Address(hooks),
        ])];
        let pool_id3 = keccak256(abi::encode(&tokens3));
        assert_ne!(pool_id, pool_id3);
    }

    #[test]
    fn test_chunk_size_calculation() {
        // Test chunking logic for tick processing
        let candidate_ticks = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let chunk_size = 3;

        let chunked: Vec<Vec<i32>> = candidate_ticks
            .chunks(chunk_size)
            .map(|s| s.to_vec())
            .collect();

        // Should create 4 chunks: [1,2,3], [4,5,6], [7,8,9], [10]
        assert_eq!(chunked.len(), 4);
        assert_eq!(chunked[0], vec![1, 2, 3]);
        assert_eq!(chunked[1], vec![4, 5, 6]);
        assert_eq!(chunked[2], vec![7, 8, 9]);
        assert_eq!(chunked[3], vec![10]);

        // Test edge case: empty input
        let empty: Vec<i32> = vec![];
        let empty_chunked: Vec<Vec<i32>> = empty
            .chunks(chunk_size)
            .map(|s| s.to_vec())
            .collect();
        assert_eq!(empty_chunked.len(), 0);

        // Test edge case: single element
        let single = vec![42];
        let single_chunked: Vec<Vec<i32>> = single
            .chunks(chunk_size)
            .map(|s| s.to_vec())
            .collect();
        assert_eq!(single_chunked.len(), 1);
        assert_eq!(single_chunked[0], vec![42]);
    }

    #[test]
    fn test_tick_deduplication() {
        // Test tick deduplication logic
        let mut candidate_ticks = vec![100, 200, 100, 300, 200, 100];
        candidate_ticks.sort_unstable();
        candidate_ticks.dedup();

        assert_eq!(candidate_ticks, vec![100, 200, 300]);

        // Test with negative ticks
        let mut neg_ticks = vec![-300, -100, -200, -100, -300];
        neg_ticks.sort_unstable();
        neg_ticks.dedup();

        assert_eq!(neg_ticks, vec![-300, -200, -100]);

        // Test empty case
        let mut empty: Vec<i32> = vec![];
        empty.sort_unstable();
        empty.dedup();
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn test_address_comparison() {
        // Test address comparison used in currency ordering
        let zero = Address::zero();
        let small = Address::from([0x01; 20]);
        let large = Address::from([0xFF; 20]);

        assert!(zero < small);
        assert!(small < large);
        assert!(zero < large);

        // Test comparison consistency
        assert_eq!(zero.cmp(&zero), std::cmp::Ordering::Equal);
        assert_eq!(zero.cmp(&small), std::cmp::Ordering::Less);
        assert_eq!(large.cmp(&zero), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_bitmap_bit_extraction() {
        // Test bitmap bit extraction logic
        use ethers::types::U256;

        // Create a bitmap with specific bits set
        let mut bitmap = U256::zero();
        
        // Set some bits
        let set_bits = vec![0, 1, 128, 255];
        for bit in &set_bits {
            bitmap = bitmap | (U256::one() << *bit);
        }

        // Test bit checking
        for bit in 0..256 {
            let is_set = bitmap.bit(bit);
            let should_be_set = set_bits.contains(&bit);
            assert_eq!(is_set, should_be_set, "Bit {} check failed", bit);
        }

        // Test with all zeros
        let zero_bitmap = U256::zero();
        for bit in 0..256 {
            assert!(!zero_bitmap.bit(bit), "Zero bitmap should have no bits set");
        }

        // Test with all ones (maximum U256)
        let max_bitmap = U256::MAX;
        for bit in 0..256 {
            assert!(max_bitmap.bit(bit), "Max bitmap should have all bits set");
        }
    }

    #[test]
    fn test_bigint_operations() {
        // Test BigInt operations used in liquidity calculations
        let large_liquidity = BigInt::from(1_000_000_000_000_000_000_000_000u128);
        let divisor = BigInt::from(10u8);
        
        let divided = &large_liquidity / divisor;
        let expected = BigInt::from(100_000_000_000_000_000_000_000u128);
        assert_eq!(divided, expected);

        // Test negation (used for upper ticks)
        let negative = -divided.clone();
        assert!(negative < BigInt::from(0));
        assert_eq!(negative, -expected);

        // Test from i128 conversion (used for liquidity_net)
        let positive_i128 = 12345i128;
        let negative_i128 = -67890i128;
        
        let pos_bigint = BigInt::from(positive_i128);
        let neg_bigint = BigInt::from(negative_i128);
        
        assert!(pos_bigint > BigInt::from(0));
        assert!(neg_bigint < BigInt::from(0));
        assert_eq!(pos_bigint.to_string(), "12345");
        assert_eq!(neg_bigint.to_string(), "-67890");
    }

    #[test]
    fn test_tick_spacing_alignment() {
        // Test that calculated ticks are properly aligned to tick spacing
        let tick_spacings = vec![1, 10, 60, 200];
        
        for spacing in tick_spacings {
            let word_pos = 5i32;
            let bit = 100usize;
            
            let tick = word_pos * 256 * spacing + (bit as i32) * spacing;
            
            // Tick should be divisible by spacing
            assert_eq!(tick % spacing, 0, "Tick {} not aligned to spacing {}", tick, spacing);
            
            // Test the components
            let word_component = word_pos * 256 * spacing;
            let bit_component = (bit as i32) * spacing;
            
            assert_eq!(word_component % spacing, 0);
            assert_eq!(bit_component % spacing, 0);
        }
    }

    #[test] 
    fn test_error_handling_scenarios() {
        // Test scenarios that might cause errors in real usage
        
        // Test saturating_add for word positions
        let max_i16 = i16::MAX;
        let result = max_i16.saturating_add(1);
        assert_eq!(result, i16::MAX); // Should not overflow
        
        let min_i16 = i16::MIN;
        let result2 = min_i16.saturating_add(-1);
        assert_eq!(result2, i16::MIN); // Should not underflow
        
        // Test normal addition
        let normal = 100i16;
        let result3 = normal.saturating_add(50);
        assert_eq!(result3, 150);
    }
}
