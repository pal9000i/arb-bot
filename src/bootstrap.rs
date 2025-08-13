use std::sync::Arc;
use std::str::FromStr;
use ethers::prelude::*;
use crate::config::Config;
use crate::chain::{providers, cex_client::CexClient};

#[allow(dead_code)]
pub struct AppState {
    pub eth_provider: Arc<Provider<Http>>,
    pub base_provider: Arc<Provider<Http>>,
    pub cex_client: CexClient,
    pub uniswap_state_view: Address,
    
    // Token addresses
    pub eth_weth_address: Address,
    pub eth_usdc_address: Address,
    pub base_weth_address: Address,
    pub base_usdc_address: Address,
    
    // Protocol addresses
    pub uniswap_universal_router: Address,
    pub aerodrome_factory_address: Address,
    pub aerodrome_weth_usdc_volatile_pool: Option<Address>,
    
    // Gas constants
    pub gas_uniswap_v4_total: u64,
    pub gas_aerodrome_swap: u64,
}

impl AppState {
    pub fn new(config: &Config) -> Result<Self, Box<dyn std::error::Error>> {
        let eth_provider = providers::create_ethereum_provider(&config.ethereum_rpc_url)?;
        let base_provider = providers::create_base_provider(&config.base_rpc_url)?;
        let cex_client = CexClient::new(config.cex_api_url.clone());
        let uniswap_state_view = Address::from_str(&config.uniswap_state_view)?;

        Ok(AppState {
            eth_provider,
            base_provider,
            cex_client,
            uniswap_state_view,
            
            // Parse token addresses from config
            eth_weth_address: Address::from_str(&config.eth_weth_address)?,
            eth_usdc_address: Address::from_str(&config.eth_usdc_address)?,
            base_weth_address: Address::from_str(&config.base_weth_address)?,
            base_usdc_address: Address::from_str(&config.base_usdc_address)?,
            
            // Protocol addresses
            uniswap_universal_router: Address::from_str(&config.uniswap_universal_router)?,
            aerodrome_factory_address: Address::from_str(&config.aerodrome_factory_address)?,
            aerodrome_weth_usdc_volatile_pool: config.aerodrome_weth_usdc_volatile_pool
                .as_ref()
                .map(|addr| Address::from_str(addr))
                .transpose()?,
            
            // Gas constants (sum components for Uniswap)
            gas_uniswap_v4_total: config.gas_uniswap_v4_swap_single_base + 
                                  config.gas_uniswap_v4_settle_take_overhead + 
                                  config.gas_uniswap_v4_hook_overhead,
            gas_aerodrome_swap: config.gas_aerodrome_swap,
        })
    }
}