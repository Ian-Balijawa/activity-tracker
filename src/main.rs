mod config;
mod error;
mod github;
mod models;
mod routes;
mod service;

use std::sync::Arc;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::routes::{router, AppState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("activity_tracker=debug,tower_http=info")),
        )
        .init();

    let config = Arc::new(Config::from_env());
    tracing::info!("Configuration: {:?}", config);   
     
    let state = AppState {
        http: github::build_http_client()?,
        config: Arc::clone(&config),
    };

    let listener = TcpListener::bind(("0.0.0.0", config.port)).await?;
    tracing::info!("listening on http://localhost:{}", config.port);

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
