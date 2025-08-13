use ethers::prelude::*;
use std::sync::Arc;

pub fn create_ethereum_provider(rpc_url: &str) -> Result<Arc<Provider<Http>>, Box<dyn std::error::Error>> {
    let provider = Provider::<Http>::try_from(rpc_url)?;
    // Could add middleware for retries, timeouts, etc.
    Ok(Arc::new(provider))
}

pub fn create_base_provider(rpc_url: &str) -> Result<Arc<Provider<Http>>, Box<dyn std::error::Error>> {
    let provider = Provider::<Http>::try_from(rpc_url)?;
    Ok(Arc::new(provider))
}