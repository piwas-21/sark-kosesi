//! Worker command rejects production and uninstalled/live keys before provider I/O.
use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, PgPool};
use std::process::{Command, Output};
fn command() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    c.arg("--reconcile-payments")
        .env("APP_ENV", "test")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID")
        .env_remove("PAYMENT_RECONCILIATION_BATCH");
    c
}
fn log(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
#[test]
fn production_never_enters_payment_recovery() {
    let output = command()
        .env("APP_ENV", "production")
        .env_remove("DATABASE_URL")
        .output()
        .expect("worker");
    assert!(!output.status.success());
    assert!(log(&output).contains("production is disabled"));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_live_invalid_and_nonunicode_configuration_is_redacted(pool: PgPool) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let missing = command()
        .env("DATABASE_URL", &database)
        .output()
        .expect("worker");
    assert!(!missing.status.success());
    assert!(log(&missing).contains("test payment configuration required"));
    assert!(!log(&missing).contains(&database));
    let synthetic = format!("test_{}", "synthetic".repeat(4));
    for (key, profile, batch) in [
        ("live_DO_NOT_USE", "pfl_SYNTHETIC", "10"),
        ("test_short", "pfl_SYNTHETIC", "10"),
        (synthetic.as_str(), "wrong", "10"),
        (synthetic.as_str(), "pfl_SYNTHETIC", "26"),
    ] {
        let output = command()
            .env("DATABASE_URL", &database)
            .env("MOLLIE_TEST_API_KEY", key)
            .env("MOLLIE_TEST_PROFILE_ID", profile)
            .env("PAYMENT_RECONCILIATION_BATCH", batch)
            .output()
            .expect("worker");
        assert!(!output.status.success());
        let l = log(&output);
        assert!(!l.contains(key));
        assert!(!l.contains(profile));
        assert!(!l.contains(&database));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(vec![0xff]);
        let output = command()
            .env("DATABASE_URL", &database)
            .env("MOLLIE_TEST_API_KEY", &synthetic)
            .env("MOLLIE_TEST_PROFILE_ID", "pfl_SYNTHETIC")
            .env("PAYMENT_RECONCILIATION_BATCH", invalid)
            .output()
            .expect("invalid batch");
        assert!(!output.status.success());
        assert!(log(&output).contains("invalid payment reconciliation batch"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("no effects");
    assert_eq!(count, 0);
}
