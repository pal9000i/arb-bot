use rocket::{get, State};
use std::sync::Arc;
use crate::web::dto::{ArbitrageQuery, ArbitrageResponse, UniswapDetails, AerodromeDetails, ArbitrageSummary, OptimalArbitrageQuery, OptimalArbitrageResponse};
use crate::engine::service::{analyze_arbitrage, find_optimal_arbitrage};
use crate::bootstrap::AppState;

#[get("/api/v1/arbitrage-opportunity?<query..>")]
pub async fn arbitrage_opportunity(
    query: ArbitrageQuery,
    app_state: &State<Arc<AppState>>,
) -> rocket::serde::json::Json<ArbitrageResponse> {
    let trade_size = query.trade_size_eth.unwrap_or(10.0).max(0.0).min(10_000.0);

    match analyze_arbitrage(
        app_state.eth_provider.clone(),
        app_state.base_provider.clone(),
        app_state.uniswap_state_view,
        &app_state.cex_client,
        trade_size,
        app_state.eth_usdc_address,
        app_state.base_weth_address,
        app_state.base_usdc_address,
        app_state.aerodrome_factory_address,
        app_state.aerodrome_weth_usdc_volatile_pool,
        app_state.gas_uniswap_v4_total,
        app_state.gas_aerodrome_swap,
    ).await {
        Ok(analysis) => {
            rocket::serde::json::Json(ArbitrageResponse {
                timestamp_utc: analysis.timestamp_utc,
                trade_size_eth: analysis.trade_size_eth,
                reference_cex_price_usd: analysis.reference_cex_price_usd,
                uniswap_v4_details: UniswapDetails {
                    sell_price_usdc_per_eth: analysis.uni_sell_price,
                    buy_price_usdc_per_eth: analysis.uni_buy_price,
                    price_impact_percent: analysis.uniswap_price_impact,
                    estimated_gas_cost_usd: analysis.uni_gas_usd,
                },
                aerodrome_details: AerodromeDetails {
                    sell_price_usdc_per_eth: analysis.aero_sell_price,
                    buy_price_usdc_per_eth: analysis.aero_buy_price,
                    price_impact_percent: analysis.aerodrome_price_impact,
                    estimated_gas_cost_usd: analysis.aero_gas_usd,
                },
                arbitrage_summary: ArbitrageSummary {
                    spread_uni_to_aero: analysis.gross_spread_sell_uni_buy_aero,
                    spread_aero_to_uni: analysis.gross_spread_sell_aero_buy_uni,
                    gross_profit_uni_to_aero_usd: analysis.gross_profit_uni_to_aero_usd,
                    gross_profit_aero_to_uni_usd: analysis.gross_profit_aero_to_uni_usd,
                    total_gas_cost_usd: analysis.total_gas_cost_usd,
                    bridge_cost_usd: analysis.bridge_cost_usd,
                    net_profit_best_usd: analysis.net_profit_best_usd,
                    recommended_action: analysis.recommended_action,
                },
            })
        }
        Err(e) => {
            log::error!("Failed to calculate arbitrage: {}", e);
            rocket::serde::json::Json(ArbitrageResponse {
                timestamp_utc: chrono::Utc::now().to_rfc3339(),
                trade_size_eth: trade_size,
                reference_cex_price_usd: 0.0,
                uniswap_v4_details: UniswapDetails {
                    sell_price_usdc_per_eth: 0.0,
                    buy_price_usdc_per_eth: 0.0,
                    price_impact_percent: 0.0,
                    estimated_gas_cost_usd: 0.0,
                },
                aerodrome_details: AerodromeDetails {
                    sell_price_usdc_per_eth: 0.0,
                    buy_price_usdc_per_eth: 0.0,
                    price_impact_percent: 0.0,
                    estimated_gas_cost_usd: 0.0,
                },
                arbitrage_summary: ArbitrageSummary {
                    spread_uni_to_aero: 0.0,
                    spread_aero_to_uni: 0.0,
                    gross_profit_uni_to_aero_usd: 0.0,
                    gross_profit_aero_to_uni_usd: 0.0,
                    total_gas_cost_usd: 0.0,
                    bridge_cost_usd: 0.0,
                    net_profit_best_usd: 0.0,
                    recommended_action: format!("ERROR: {}", e),
                },
            })
        }
    }
}

#[get("/api/v1/optimal-arbitrage?<query..>")]
pub async fn optimal_arbitrage_opportunity(
    query: OptimalArbitrageQuery,
    app_state: &State<Arc<AppState>>,
) -> rocket::serde::json::Json<OptimalArbitrageResponse> {
    let max_size = query.max_size_eth.unwrap_or(100.0).max(0.1).min(1000.0);

    match find_optimal_arbitrage(
        app_state.eth_provider.clone(),
        app_state.base_provider.clone(),
        app_state.uniswap_state_view,
        &app_state.cex_client,
        max_size,
        app_state.eth_usdc_address,
        app_state.base_weth_address,
        app_state.base_usdc_address,
        app_state.aerodrome_factory_address,
        app_state.aerodrome_weth_usdc_volatile_pool,
        app_state.gas_uniswap_v4_total,
        app_state.gas_aerodrome_swap,
    ).await {
        Ok(analysis) => {
            rocket::serde::json::Json(OptimalArbitrageResponse {
                timestamp_utc: analysis.timestamp_utc,
                reference_cex_price_usd: analysis.reference_cex_price_usd,
                optimal_trade_size_eth: analysis.optimal_trade_size_eth,
                optimal_direction: analysis.optimal_direction,
                net_profit_usd: analysis.net_profit_usd,
                gross_profit_usd: analysis.gross_profit_usd,
                total_costs_usd: analysis.total_costs_usd,
                effective_sell_price_usdc_per_eth: analysis.effective_sell_price_usdc_per_eth,
                effective_buy_price_usdc_per_eth: analysis.effective_buy_price_usdc_per_eth,
                gas_cost_usd: analysis.gas_cost_usd,
                bridge_cost_usd: analysis.bridge_cost_usd,
                recommended_action: analysis.recommended_action,
            })
        }
        Err(e) => {
            log::error!("Failed to find optimal arbitrage: {}", e);
            rocket::serde::json::Json(OptimalArbitrageResponse {
                timestamp_utc: chrono::Utc::now().to_rfc3339(),
                reference_cex_price_usd: 0.0,
                optimal_trade_size_eth: 0.0,
                optimal_direction: "ERROR".to_string(),
                net_profit_usd: 0.0,
                gross_profit_usd: 0.0,
                total_costs_usd: 0.0,
                effective_sell_price_usdc_per_eth: 0.0,
                effective_buy_price_usdc_per_eth: 0.0,
                gas_cost_usd: 0.0,
                bridge_cost_usd: 0.0,
                recommended_action: format!("ERROR: {}", e),
            })
        }
    }
}

#[get("/health")]
pub fn health() -> &'static str {
    "OK"
}

#[get("/metrics")]
pub fn metrics() -> &'static str {
    // Basic Prometheus metrics format
    // In production, you'd use a proper metrics library like prometheus crate
    "# TYPE arrakis_uptime_seconds counter\n\
     arrakis_uptime_seconds 1\n\
     # TYPE arrakis_requests_total counter\n\
     arrakis_requests_total{endpoint=\"health\"} 1\n\
     # TYPE arrakis_info gauge\n\
     arrakis_info{version=\"0.1.0\",service=\"arbitrage\"} 1\n"
}