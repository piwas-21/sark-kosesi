#![forbid(unsafe_code)]
use commerce_api::{
    AppState, PREVIEW_REQUEST_CAPACITY, config::Config, contract, router, router_with_guest,
};
use commerce_infrastructure::runtime::{SecureIdentifiers, SystemClock};
use commerce_infrastructure::{MIGRATOR, connect};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("openapi") {
        println!("{}", contract::openapi()?);
        return Ok(());
    }
    tracing_subscriber::fmt().json().init();
    let config = Config::parse(&std::env::vars().collect()).map_err(std::io::Error::other)?;
    let pool = connect(&config.database_url)
        .await
        .map_err(|_| std::io::Error::other("database connection failed"))?;
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|_| std::io::Error::other("migration failed"))?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let state = AppState {
        pool,
        preview_token: Arc::from(config.preview_token),
        capacity: Arc::new(Semaphore::new(PREVIEW_REQUEST_CAPACITY)),
        clock: Arc::new(SystemClock),
        identifiers: Arc::new(SecureIdentifiers),
    };
    let app = match config.guest_origin {
        Some(origin) => router_with_guest(state, origin)
            .map_err(|_| std::io::Error::other("guest configuration invalid"))?,
        None => router(state),
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
