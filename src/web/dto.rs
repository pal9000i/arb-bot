use rocket::serde::{Deserialize, Serialize};

#[derive(Deserialize, rocket::FromForm)]
pub struct ArbitrageQuery {
    pub trade_size_eth: Option<f64>,
}

#[derive(Deserialize, rocket::FromForm)]
pub struct OptimalArbitrageQuery {
    pub max_size_eth: Option<f64>,
}

#[derive(Serialize)]
pub struct UniswapDetails {
    pub sell_price_usdc_per_eth: f64,  // ETH->USDC execution price
    pub buy_price_usdc_per_eth: f64,   // USDC->ETH execution price
    pub price_impact_percent: f64,
    pub estimated_gas_cost_usd: f64,
}

#[derive(Serialize)]
pub struct AerodromeDetails {
    pub sell_price_usdc_per_eth: f64,  // ETH->USDC execution price
    pub buy_price_usdc_per_eth: f64,   // USDC->ETH execution price
    pub price_impact_percent: f64,
    pub estimated_gas_cost_usd: f64,
}

#[derive(Serialize)]
pub struct ArbitrageSummary {
    pub spread_uni_to_aero: f64,       // uni_sell - aero_buy
    pub spread_aero_to_uni: f64,       // aero_sell - uni_buy
    pub gross_profit_uni_to_aero_usd: f64,  // spread * size
    pub gross_profit_aero_to_uni_usd: f64,  // spread * size
    pub total_gas_cost_usd: f64,
    pub bridge_cost_usd: f64,
    pub net_profit_best_usd: f64,
    pub recommended_action: String,
}

#[derive(Serialize)]
pub struct ArbitrageResponse {
    pub timestamp_utc: String,
    pub trade_size_eth: f64,
    pub reference_cex_price_usd: f64,
    pub uniswap_v4_details: UniswapDetails,
    pub aerodrome_details: AerodromeDetails,
    pub arbitrage_summary: ArbitrageSummary,
}

#[derive(Serialize)]
pub struct OptimalArbitrageResponse {
    pub timestamp_utc: String,
    pub reference_cex_price_usd: f64,
    pub optimal_trade_size_eth: f64,
    pub optimal_direction: String,
    pub net_profit_usd: f64,
    pub gross_profit_usd: f64,
    pub total_costs_usd: f64,
    pub effective_sell_price_usdc_per_eth: f64,
    pub effective_buy_price_usdc_per_eth: f64,
    pub gas_cost_usd: f64,
    pub bridge_cost_usd: f64,
    pub recommended_action: String,
}