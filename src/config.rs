use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub ethereum_rpc_url: String,
    pub base_rpc_url: String,
    pub uniswap_state_view: String,
    pub cex_api_url: String,
    pub port: u16,
    
    // Ethereum token addresses
    pub eth_weth_address: String,
    pub eth_usdc_address: String,
    
    // Base network token addresses
    pub base_weth_address: String,
    pub base_usdc_address: String,
    
    // Protocol addresses
    pub uniswap_universal_router: String,
    pub aerodrome_factory_address: String,
    pub aerodrome_weth_usdc_volatile_pool: Option<String>,
    
    // Gas constants
    pub gas_uniswap_v4_swap_single_base: u64,
    pub gas_uniswap_v4_settle_take_overhead: u64,
    pub gas_uniswap_v4_hook_overhead: u64,
    pub gas_aerodrome_swap: u64,
}

impl Config {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        // Load configuration files (secrets first, then public config)
        dotenv::from_filename("secrets.env").ok();
        dotenv::from_filename("addresses.env").ok();
        dotenv::from_filename("config/addresses.env").ok();
        dotenv::dotenv().ok();

        Ok(Config {
            ethereum_rpc_url: env::var("ETHEREUM_RPC_URL")
                .map_err(|_| "ETHEREUM_RPC_URL must be set")?,
            base_rpc_url: env::var("BASE_RPC_URL")
                .map_err(|_| "BASE_RPC_URL must be set")?,
            uniswap_state_view: env::var("UNISWAP_V4_STATE_VIEW")
                .map_err(|_| "UNISWAP_V4_STATE_VIEW must be set")?,
            cex_api_url: env::var("CEX_API_URL")
                .unwrap_or_else(|_| "https://api.coinbase.com/v2/exchange-rates?currency=ETH".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "8000".to_string())
                .parse()
                .unwrap_or(8000),
                
            // Token addresses from environment
            eth_weth_address: env::var("ETH_WETH_ADDRESS")
                .map_err(|_| "ETH_WETH_ADDRESS must be set")?,
            eth_usdc_address: env::var("ETH_USDC_ADDRESS")
                .map_err(|_| "ETH_USDC_ADDRESS must be set")?,
            base_weth_address: env::var("BASE_WETH_ADDRESS")
                .map_err(|_| "BASE_WETH_ADDRESS must be set")?,
            base_usdc_address: env::var("BASE_USDC_ADDRESS")
                .map_err(|_| "BASE_USDC_ADDRESS must be set")?,
                
            // Protocol addresses
            uniswap_universal_router: env::var("UNISWAP_V4_UNIVERSAL_ROUTER")
                .map_err(|_| "UNISWAP_V4_UNIVERSAL_ROUTER must be set")?,
            aerodrome_factory_address: env::var("AERODROME_FACTORY_ADDRESS")
                .map_err(|_| "AERODROME_FACTORY_ADDRESS must be set")?,
            aerodrome_weth_usdc_volatile_pool: env::var("AERODROME_WETH_USDC_VOLATILE_POOL").ok(),
                
            // Gas constants
            gas_uniswap_v4_swap_single_base: env::var("GAS_UNISWAP_V4_SWAP_SINGLE_BASE")
                .unwrap_or_else(|_| "120000".to_string()).parse().unwrap_or(120000),
            gas_uniswap_v4_settle_take_overhead: env::var("GAS_UNISWAP_V4_SETTLE_TAKE_OVERHEAD")
                .unwrap_or_else(|_| "20000".to_string()).parse().unwrap_or(20000),
            gas_uniswap_v4_hook_overhead: env::var("GAS_UNISWAP_V4_HOOK_OVERHEAD")
                .unwrap_or_else(|_| "0".to_string()).parse().unwrap_or(0),
            gas_aerodrome_swap: env::var("GAS_AERODROME_SWAP")
                .unwrap_or_else(|_| "185000".to_string()).parse().unwrap_or(185000),
        })
    }
}