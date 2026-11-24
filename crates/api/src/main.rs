#![forbid(unsafe_code)]
mod migration_manifest;
mod preapplied;

use commerce_api::{
    AppState, PREVIEW_REQUEST_CAPACITY, TestGuestOperations, config::Config, contract,
    router_with_test_guest_operations,
};
use commerce_infrastructure::image_ingestion::configure_decoder;
use commerce_infrastructure::runtime::{SecureIdentifiers, SystemClock};
use commerce_infrastructure::{MIGRATOR, connect};
use std::sync::Arc;
use tokio::sync::Semaphore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os();
    arguments.next();
    let first = arguments.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("migration-manifest")) {
        if arguments.next().is_some() {
            return Err(
                std::io::Error::other("migration-manifest accepts no extra arguments").into(),
            );
        }
        migration_manifest::print()?;
        return Ok(());
    }
    if first.as_deref() == Some(std::ffi::OsStr::new("openapi")) {
        println!("{}", contract::openapi()?);
        return Ok(());
    }
    run_server()
}

#[tokio::main]
async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().json().init();
    let config = Config::parse(&std::env::vars().collect()).map_err(std::io::Error::other)?;
    if let Some(endpoint) = config.image_decoder {
        configure_decoder(endpoint).map_err(std::io::Error::other)?;
    }
    let pool = if config.preview_database_preapplied {
        let pool = preapplied::connect(&config.database_url)
            .await
            .map_err(|_| std::io::Error::other("database connection failed"))?;
        preapplied::validate(&pool)
            .await
            .map_err(|_| std::io::Error::other("preapplied migration validation failed"))?;
        pool
    } else {
        let pool = connect(&config.database_url)
            .await
            .map_err(|_| std::io::Error::other("database connection failed"))?;
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|_| std::io::Error::other("migration failed"))?;
        pool
    };
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let state = AppState {
        pool,
        preview_token: Arc::from(config.preview_token),
        capacity: Arc::new(Semaphore::new(PREVIEW_REQUEST_CAPACITY)),
        clock: Arc::new(SystemClock),
        identifiers: Arc::new(SecureIdentifiers),
    };
    let app = router_with_test_guest_operations(
        state,
        config.guest_origin,
        config.callback_profile,
        config.verification,
        config.test_login,
        TestGuestOperations {
            order: config.test_guest_order,
            cancellation: config.test_guest_cancellation,
            pending_cancellation: config.test_guest_pending_cancellation,
            order_progress: config.test_guest_order_progress,
        },
    )
    .map_err(|_| std::io::Error::other("API configuration invalid"))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
