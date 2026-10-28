//! Read-only probe by default; explicit local/test hold-expiry passes are bounded.
#![forbid(unsafe_code)]
use commerce_infrastructure::runtime::SystemClock;
use commerce_infrastructure::{connect, readiness};
use commerce_worker::expire_batch;

fn mode() -> Result<bool, std::io::Error> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => Ok(false),
        [argument] if argument == "--expire-holds" => Ok(true),
        _ => Err(std::io::Error::other("unsupported worker command")),
    }
}
fn batch_limit() -> Result<u16, std::io::Error> {
    let raw = match std::env::var("HOLD_EXPIRY_BATCH") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(25),
        Err(_) => return Err(std::io::Error::other("invalid HOLD_EXPIRY_BATCH")),
    };
    raw.parse::<u16>()
        .ok()
        .filter(|limit| (1..=100).contains(limit))
        .ok_or_else(|| std::io::Error::other("invalid HOLD_EXPIRY_BATCH"))
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    tracing_subscriber::fmt().json().init();
    if !matches!(std::env::var("APP_ENV").as_deref(), Ok("local" | "test")) {
        return Err(std::io::Error::other("production is disabled"));
    }
    let expire = mode()?;
    let limit = if expire { batch_limit()? } else { 0 };
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| std::io::Error::other("DATABASE_URL required"))?;
    let pool = connect(&database_url)
        .await
        .map_err(|_| std::io::Error::other("database connection failed"))?;
    readiness(&pool)
        .await
        .map_err(|_| std::io::Error::other("database unavailable"))?;
    if expire {
        let batch = expire_batch(&pool, &SystemClock, limit)
            .await
            .map_err(|_| std::io::Error::other("hold expiry unavailable"))?;
        tracing::info!(
            code = "hold_expiry_batch",
            expired = batch.expired,
            failed = batch.failed,
            limit
        );
        if batch.failed > 0 {
            return Err(std::io::Error::other("hold expiry requires retry"));
        }
    } else {
        tracing::info!(code = "worker_dependencies_ready", effects_enabled = false);
    }
    Ok(())
}
