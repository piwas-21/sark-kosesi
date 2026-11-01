//! Private callback worker rejects invalid environments/configuration without side effects.
use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, PgPool};
use std::process::{Command, Output};
fn command() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    c.arg("--reconcile-callbacks")
        .env("APP_ENV", "test")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID")
        .env_remove("CALLBACK_RECONCILIATION_BATCH");
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
fn callback_supervision_never_starts_in_production_or_staging() {
    for environment in ["production", "staging"] {
        let result = command()
            .env("APP_ENV", environment)
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!result.status.success());
        assert!(log(&result).contains("production is disabled"));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_live_invalid_and_nonunicode_callback_configuration_is_redacted_without_jobs(
    pool: PgPool,
) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let missing = command()
        .env("DATABASE_URL", &database)
        .output()
        .expect("worker");
    assert!(!missing.status.success());
    assert!(!log(&missing).contains(&database));
    let synthetic = format!("test_{}", "synthetic".repeat(4));
    for (key, profile, batch) in [
        ("live_DO_NOT_USE", "pfl_SYNTHETIC", "10"),
        ("test_short", "pfl_SYNTHETIC", "10"),
        (synthetic.as_str(), "wrong", "10"),
        (synthetic.as_str(), "pfl_SYNTHETIC", "0"),
        (synthetic.as_str(), "pfl_SYNTHETIC", "26"),
        (synthetic.as_str(), "pfl_SYNTHETIC", "invalid"),
    ] {
        let output = command()
            .env("DATABASE_URL", &database)
            .env("MOLLIE_TEST_API_KEY", key)
            .env("MOLLIE_TEST_PROFILE_ID", profile)
            .env("CALLBACK_RECONCILIATION_BATCH", batch)
            .output()
            .expect("invalid callback configuration");
        assert!(!output.status.success());
        let diagnostic = log(&output);
        assert!(!diagnostic.contains(key));
        assert!(!diagnostic.contains(profile));
        assert!(!diagnostic.contains(&database));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        for field in [
            "MOLLIE_TEST_API_KEY",
            "MOLLIE_TEST_PROFILE_ID",
            "CALLBACK_RECONCILIATION_BATCH",
        ] {
            let output = command()
                .env("DATABASE_URL", &database)
                .env("MOLLIE_TEST_API_KEY", &synthetic)
                .env("MOLLIE_TEST_PROFILE_ID", "pfl_SYNTHETIC")
                .env(field, std::ffi::OsString::from_vec(vec![0xff]))
                .output()
                .expect("invalid Unicode config");
            assert!(!output.status.success());
            assert!(!log(&output).contains(&database));
            assert!(!log(&output).contains(&synthetic));
        }
    }
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM callback_intakes),(SELECT count(*) FROM callback_inbox_jobs),(SELECT count(*) FROM callback_lookup_operations)").fetch_one(&pool).await.expect("no effects");
    assert_eq!(counts, (0, 0, 0));
}
