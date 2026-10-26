#![forbid(unsafe_code)]
use commerce_api::{AppState, config::Config, contract, router};
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
    let app = router(AppState {
        pool,
        preview_token: Arc::from(config.preview_token),
        capacity: Arc::new(Semaphore::new(64)),
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
