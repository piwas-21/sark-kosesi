//! Private CLI boundary tests with isolated PostgreSQL; no provider access.
use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, PgPool};
use std::process::{Command, Output};

fn command(args: &[&str]) -> Command {
    let mut process = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    process
        .args(args)
        .env("APP_ENV", "test")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID");
    process
}
fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn assert_redacted(output: &Output, secrets: &[&str]) {
    let text = output_text(output);
    for secret in secrets {
        assert!(
            !text.contains(secret),
            "worker output must not echo private inputs"
        );
    }
}

#[test]
fn production_and_malformed_commands_stop_before_database_or_provider() {
    let attempt = "0123456789abcdef0123456789abcdef";
    let effect = "synthetic-private-effect";
    let database = "postgresql://private-database-marker@127.0.0.1:1/not-a-database";
    for environment in ["production", "staging"] {
        let output = command(&["--inspect-paid-parent", attempt, effect])
            .env("APP_ENV", environment)
            .env("DATABASE_URL", database)
            .output()
            .expect("worker");
        assert!(!output.status.success());
        assert!(output_text(&output).contains("production is disabled"));
        assert_redacted(&output, &[database, attempt, effect]);
        let output = command(&["--inspect-paid-parent-set", attempt])
            .env("APP_ENV", environment)
            .env("DATABASE_URL", database)
            .output()
            .expect("worker set");
        assert!(!output.status.success());
        assert!(output_text(&output).contains("production is disabled"));
        assert_redacted(&output, &[database, attempt]);
    }
    let long_effect = "x".repeat(129);
    for args in [
        vec!["--inspect-paid-parent"],
        vec!["--inspect-paid-parent", attempt],
        vec!["--inspect-paid-parent", attempt, effect, "extra"],
        vec!["--inspect-paid-parent", "invalid-attempt", effect],
        vec![
            "--inspect-paid-parent",
            "ABCDEF0123456789abcdef0123456789",
            effect,
        ],
        vec!["--inspect-paid-parent", attempt, ""],
        vec!["--inspect-paid-parent", attempt, "invalid_effect"],
        vec!["--inspect-paid-parent", attempt, long_effect.as_str()],
        vec!["--inspect-paid-parent-set"],
        vec!["--inspect-paid-parent-set", "invalid-attempt"],
        vec!["--inspect-paid-parent-set", attempt, "extra"],
    ] {
        let output = command(&args)
            .env("DATABASE_URL", database)
            .output()
            .expect("worker");
        assert!(!output.status.success());
        assert!(output_text(&output).contains("unsupported worker command"));
        assert_redacted(
            &output,
            &[
                database,
                attempt,
                effect,
                "invalid-attempt",
                "invalid_effect",
                &long_effect,
            ],
        );
    }
}

// Compare all base-table row snapshots without emitting their values on failure.
// This catches updates and deletions as well as unexpected insertions.
async fn database_snapshot(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT c.relname::text,table_to_xml(c.oid::regclass,true,true,'')::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relkind IN ('r','p') ORDER BY c.relname")
        .fetch_all(pool).await.expect("isolated base-table row snapshots")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unknown_attempt_inspection_needs_no_provider_config_and_changes_no_rows(pool: PgPool) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let attempt = "0123456789abcdef0123456789abcdef";
    let effect = "synthetic-private-effect";
    let before = database_snapshot(&pool).await;
    for environment in ["test", "local"] {
        let output = command(&["--inspect-paid-parent", attempt, effect])
            .env("APP_ENV", environment)
            .env("DATABASE_URL", &database)
            .output()
            .expect("worker inspection");
        assert!(!output.status.success());
        assert!(output_text(&output).contains("paid-parent evidence unavailable"));
        assert_redacted(&output, &[database.as_str(), attempt, effect]);
        assert!(
            database_snapshot(&pool).await == before,
            "inspection must not modify persisted rows"
        );
        let output = command(&["--inspect-paid-parent-set", attempt])
            .env("APP_ENV", environment)
            .env("DATABASE_URL", &database)
            .output()
            .expect("worker set");
        assert!(!output.status.success());
        assert!(output_text(&output).contains("paid-parent evidence set unavailable"));
        assert_redacted(&output, &[database.as_str(), attempt]);
        assert!(database_snapshot(&pool).await == before);
    }
}
