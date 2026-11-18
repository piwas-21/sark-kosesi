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

// Compare complete row multisets, including NULLs and duplicates, without printing
// them. One read-only repeatable snapshot covers discovery and every table read.
async fn database_snapshot(pool: &PgPool) -> Vec<(String, String, Vec<String>)> {
    let mut tx = pool.begin().await.expect("begin snapshot");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .expect("configure snapshot");
    for setting in [
        "SET LOCAL TIME ZONE 'UTC'",
        "SET LOCAL DateStyle = 'ISO, YMD'",
        "SET LOCAL IntervalStyle = 'postgres'",
        "SET LOCAL extra_float_digits = 3",
        "SET LOCAL bytea_output = 'hex'",
    ] {
        sqlx::query(setting)
            .execute(&mut *tx)
            .await
            .expect("configure snapshot serialization");
    }
    let tables: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT n.nspname::text, c.relname::text, format('%I.%I', n.nspname, c.relname) \
         FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p')",
    )
    .fetch_all(&mut *tx)
    .await
    .expect("discover base tables");
    let mut snapshot = Vec::with_capacity(tables.len());
    for (schema, table, identifier) in tables {
        // PostgreSQL's format(%I) quotes each catalog-derived identifier separately.
        let query =
            format!("SELECT row_to_json(snapshot_row)::text FROM {identifier} AS snapshot_row");
        let mut rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .fetch_all(&mut *tx)
            .await
            .unwrap_or_else(|_| panic!("snapshot base table failed"));
        rows.sort_unstable();
        snapshot.push((schema, table, rows));
    }
    tx.commit().await.expect("finish snapshot");
    snapshot.sort_unstable();
    snapshot
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_detects_row_changes_and_ignores_physical_order(pool: PgPool) {
    sqlx::query("CREATE TABLE \"snapshot \"\"controls\" (value text, extra text)")
        .execute(&pool)
        .await
        .expect("create control table");
    let insert = "INSERT INTO \"snapshot \"\"controls\" (value, extra) VALUES \
                  ('AB', NULL), ('A-C', 'quoted \"value\"'), \
                  ('AB', NULL), (NULL, 'null control')";
    sqlx::query(insert)
        .execute(&pool)
        .await
        .expect("seed controls");
    let original = database_snapshot(&pool).await;
    let control = original
        .iter()
        .find(|(_, table, _)| table == "snapshot \"controls")
        .expect("control table captured");
    let mut expected = vec![
        r#"{"value":"AB","extra":null}"#.to_owned(),
        r#"{"value":"A-C","extra":"quoted \"value\""}"#.to_owned(),
        r#"{"value":"AB","extra":null}"#.to_owned(),
        r#"{"value":null,"extra":"null control"}"#.to_owned(),
    ];
    expected.sort_unstable();
    assert!(
        control.2 == expected,
        "control rows must preserve exact values and duplicates"
    );
    assert!(
        database_snapshot(&pool).await == original,
        "unchanged rows must match"
    );

    sqlx::query("TRUNCATE TABLE \"snapshot \"\"controls\"")
        .execute(&pool)
        .await
        .expect("reorder controls");
    sqlx::query(
        "INSERT INTO \"snapshot \"\"controls\" (value, extra) VALUES \
                 (NULL, 'null control'), ('AB', NULL), \
                 ('A-C', 'quoted \"value\"'), ('AB', NULL)",
    )
    .execute(&pool)
    .await
    .expect("reinsert controls");
    assert!(
        database_snapshot(&pool).await == original,
        "row order must not matter"
    );

    sqlx::query("UPDATE \"snapshot \"\"controls\" SET value = 'changed' WHERE value IS NULL")
        .execute(&pool)
        .await
        .expect("update control");
    assert!(
        database_snapshot(&pool).await != original,
        "updates must be detected"
    );
    sqlx::query("UPDATE \"snapshot \"\"controls\" SET value = NULL WHERE value = 'changed'")
        .execute(&pool)
        .await
        .expect("restore update");
    assert!(
        database_snapshot(&pool).await == original,
        "restored NULL must match"
    );

    sqlx::query("INSERT INTO \"snapshot \"\"controls\" VALUES ('AB', NULL)")
        .execute(&pool)
        .await
        .expect("insert duplicate");
    assert!(
        database_snapshot(&pool).await != original,
        "duplicate insertion must be detected"
    );
    sqlx::query(
        "DELETE FROM \"snapshot \"\"controls\" WHERE ctid = \
                 (SELECT ctid FROM \"snapshot \"\"controls\" WHERE value = 'AB' LIMIT 1)",
    )
    .execute(&pool)
    .await
    .expect("remove duplicate");
    assert!(
        database_snapshot(&pool).await == original,
        "duplicate count must be restored"
    );
    sqlx::query("DELETE FROM \"snapshot \"\"controls\" WHERE value = 'A-C'")
        .execute(&pool)
        .await
        .expect("delete control");
    assert!(
        database_snapshot(&pool).await != original,
        "deletions must be detected"
    );
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
