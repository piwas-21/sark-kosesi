//! P1 dependency probe used by supervision; durable effects begin with P3 outbox work.
#![forbid(unsafe_code)]
use commerce_infrastructure::{connect, readiness};

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt().json().init();
    if !matches!(std::env::var("APP_ENV").as_deref(), Ok("local" | "test")) {
        return Err(std::io::Error::other("production is disabled"));
    }
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| std::io::Error::other("DATABASE_URL required"))?;
    let pool = connect(&database_url)
        .await
        .map_err(|_| std::io::Error::other("database connection failed"))?;
    readiness(&pool)
        .await
        .map_err(|_| std::io::Error::other("database unavailable"))?;
    tracing::info!(code = "worker_dependencies_ready", effects_enabled = false);
    Ok(())
}
