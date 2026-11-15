//! Worker command rejects production and uninstalled/live keys before provider I/O.
use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, PgPool};
use std::process::{Command, Output};
fn command() -> Command {
    let mut command = worker();
    command.arg("--reconcile-payments");
    command
}
fn worker() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    c.env("APP_ENV", "test")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID")
        .env_remove("PAYMENT_RECONCILIATION_BATCH");
    c
}

#[test]
fn checkpoint_inspection_rejects_wrong_environment_and_malformed_arguments() {
    const ATTEMPT: &str = "0123456789abcdef0123456789abcdef";
    for environment in ["staging", "production"] {
        let output = worker()
            .args(["--inspect-payment-recovery", ATTEMPT])
            .env("APP_ENV", environment)
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!output.status.success());
        assert!(log(&output).contains("production is disabled"));
        assert!(!log(&output).contains(ATTEMPT));
    }
    for args in [
        vec!["--inspect-payment-recovery"],
        vec!["--inspect-payment-recovery", "invalid-attempt"],
        vec!["--inspect-payment-recovery", ATTEMPT, "unexpected-argument"],
    ] {
        let output = worker()
            .args(&args)
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!output.status.success());
        let text = log(&output);
        assert!(text.contains("unsupported worker command"));
        for argument in args.iter().skip(1) {
            assert!(!text.contains(argument));
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_checkpoint_is_reported_without_provider_configuration_or_effects(pool: PgPool) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let attempt = "0123456789abcdef0123456789abcdef";
    let output = worker()
        .args(["--inspect-payment-recovery", attempt])
        .env("DATABASE_URL", &database)
        .output()
        .expect("worker");
    assert!(output.status.success());
    let text = log(&output);
    assert!(text.contains(r#""code":"payment_recovery_checkpoint""#));
    assert!(text.contains(r#""state":"absent""#));
    assert!(!text.contains(attempt));
    assert!(!text.contains(&database));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("no recovery jobs written");
    assert_eq!(count, 0);
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
