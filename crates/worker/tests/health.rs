use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, PgPool};
use std::process::Command;
#[test]
fn production_probe_is_disabled() {
    let result = Command::new(env!("CARGO_BIN_EXE_commerce-worker"))
        .env("APP_ENV", "production")
        .output()
        .expect("probe");
    assert!(!result.status.success());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn real_worker_dependency_probe(pool: PgPool) {
    let result = Command::new(env!("CARGO_BIN_EXE_commerce-worker"))
        .env("APP_ENV", "test")
        .env(
            "DATABASE_URL",
            pool.connect_options().to_url_lossy().to_string(),
        )
        .output()
        .expect("probe");
    assert!(result.status.success());
    pool.close().await;
}
#[test]
fn missing_config_and_unavailable_database_fail() {
    let binary = env!("CARGO_BIN_EXE_commerce-worker");
    let missing = Command::new(binary)
        .env("APP_ENV", "test")
        .env_remove("DATABASE_URL")
        .output()
        .expect("probe");
    assert!(!missing.status.success());
    let unavailable = Command::new(binary)
        .env("APP_ENV", "test")
        .env(
            "DATABASE_URL",
            "postgresql://p1_test@127.0.0.1:1/unavailable",
        )
        .output()
        .expect("probe");
    assert!(!unavailable.status.success());
}
