use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ArbitrageResponse {
    pub timestamp_utc: DateTime<Utc>,
    pub trade_size_eth: f64,
    pub reference_cex_price_usd: f64,
    pub uniswap_v4_details: DexDetails,
    pub aerodrome_details: DexDetails,
    pub arbitrage_summary: ArbitrageSummary,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DexDetails {
    pub effective_price_usd: f64,
    pub price_impact_percent: f64,
    pub estimated_gas_cost_usd: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArbitrageSummary {
    pub potential_profit_usd: f64,
    pub total_gas_cost_usd: f64,
    pub net_profit_usd: f64,
    pub recommended_action: String,
}

#[derive(Debug, Clone)]
pub struct PoolState {
    pub sqrt_price_x96: u128,
    pub tick: i32,
    pub liquidity: u128,
}

#[derive(Debug, Clone)]
pub struct GasEstimate {
    pub gas_price: u64,
    pub gas_limit: u64,
    pub eth_price_usd: f64,
}

#[derive(Debug, Clone)]
pub struct V4ExecutionResult {
    pub effective_price_usd: f64,
    pub price_impact_percent: f64,
    pub pool_used: String, // Pool identifier
    pub current_price: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    pub profitable: bool,
    pub direction: String,
    pub optimal_trade_size_eth: f64,
    pub expected_profit_usd: f64,
    pub confidence_score: f64,
    pub execution_plan: serde_json::Value,
    pub market_conditions: serde_json::Value,
    pub gas_estimates: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

impl ArbitrageResponse {
    pub fn error(message: String) -> Self {
        Self {
            timestamp_utc: Utc::now(),
            trade_size_eth: 0.0,
            reference_cex_price_usd: 0.0,
            uniswap_v4_details: DexDetails {
                effective_price_usd: 0.0,
                price_impact_percent: 0.0,
                estimated_gas_cost_usd: 0.0,
            },
            aerodrome_details: DexDetails {
                effective_price_usd: 0.0,
                price_impact_percent: 0.0,
                estimated_gas_cost_usd: 0.0,
            },
            arbitrage_summary: ArbitrageSummary {
                potential_profit_usd: 0.0,
                total_gas_cost_usd: 0.0,
                net_profit_usd: 0.0,
                recommended_action: format!("ERROR: {}", message),
            },
        }
    }
}