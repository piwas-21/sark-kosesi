//! Local/test command routing and configuration fail closed before provider I/O.
use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, PgPool};
use std::process::{Command, Output};

fn command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    command
        .args(args)
        .env("APP_ENV", "test")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID")
        .env_remove("REVERSAL_ITEM_BATCH");
    command
}
fn log(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
#[test]
fn production_and_malformed_routes_fail_before_database_or_provider() {
    let id = "0123456789abcdef0123456789abcdef";
    for args in [
        vec!["--reconcile-reversal-items"],
        vec!["--enroll-reversal-item", id, id, id, "1", "key"],
        vec!["--inspect-reversal-item", id],
    ] {
        let output = command(&args)
            .env("APP_ENV", "production")
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!output.status.success());
        assert!(log(&output).contains("production is disabled"));
    }
    for args in [
        vec!["--reconcile-reversal-items", "extra"],
        vec!["--enroll-reversal-item", id, id, id, "1"],
        vec!["--enroll-reversal-item", id, id, id, "0", "secret-key"],
        vec!["--inspect-reversal-item", "bad"],
    ] {
        let output = command(&args)
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!output.status.success());
        assert!(log(&output).contains("unsupported worker command"));
        assert!(!log(&output).contains("secret-key"));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_configuration_and_read_only_inspection_do_not_create_jobs(pool: PgPool) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let id = "0123456789abcdef0123456789abcdef";
    let key = format!("test_{}", "synthetic".repeat(4));
    for args in [
        vec!["--reconcile-reversal-items"],
        vec!["--enroll-reversal-item", id, id, id, "1", "key"],
    ] {
        let missing = command(&args)
            .env("DATABASE_URL", &database)
            .output()
            .expect("missing config");
        assert!(!missing.status.success());
        for (credential, profile, batch) in [
            ("live_DO_NOT_USE", "pfl_SYNTHETIC", "10"),
            ("test_short", "pfl_SYNTHETIC", "10"),
            (key.as_str(), "wrong", "10"),
            (key.as_str(), "pfl_SYNTHETIC", "0"),
            (key.as_str(), "pfl_SYNTHETIC", "26"),
        ] {
            let output = command(&args)
                .env("DATABASE_URL", &database)
                .env("MOLLIE_TEST_API_KEY", credential)
                .env("MOLLIE_TEST_PROFILE_ID", profile)
                .env("REVERSAL_ITEM_BATCH", batch)
                .output()
                .expect("invalid config");
            assert!(!output.status.success());
            for secret in [credential, profile, database.as_str()] {
                assert!(!log(&output).contains(secret));
            }
        }
    }
    let inspect = command(&["--inspect-reversal-item", id])
        .env("DATABASE_URL", &database)
        .output()
        .expect("inspect");
    assert!(!inspect.status.success());
    assert!(log(&inspect).contains("reversal item not found"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM reversal_item_jobs")
        .fetch_one(&pool)
        .await
        .expect("no effects");
    assert_eq!(count, 0);
}
