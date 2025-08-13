use rocket::{launch, routes};
use std::sync::Arc;

mod config;
mod bootstrap;
mod chain;
mod math;
mod engine;
mod web;

use crate::web::routes::{arbitrage_opportunity, optimal_arbitrage_opportunity, health, metrics};

#[launch]
async fn rocket() -> _ {
    env_logger::init();

    // Load configuration
    let config = config::Config::from_env()
        .expect("Failed to load configuration");

    // Build application state
    let app_state = Arc::new(
        bootstrap::AppState::new(&config)
            .expect("Failed to initialize application state")
    );

    // Configure Rocket
    let figment = rocket::Config::figment()
        .merge(("port", config.port))
        .merge(("address", "0.0.0.0"));

    rocket::custom(figment)
        .manage(app_state)
        .mount("/", routes![arbitrage_opportunity, optimal_arbitrage_opportunity, health, metrics])
}