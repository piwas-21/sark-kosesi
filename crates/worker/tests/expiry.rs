use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::inventory::WarehouseChange;
use commerce_infrastructure::{
    MIGRATOR,
    holds::{HoldRequest, reserve_bom},
    stock::{WarehouseRequest, warehouse_movement},
};
use commerce_worker::{WorkerError, expire_batch};
use sqlx::{ConnectOptions, Executor, PgPool};
use std::process::{Command, Output};

struct FixedClock(Result<u64, ContextError>);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
    }
}
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    command
        .env("APP_ENV", "test")
        .env_remove("HOLD_EXPIRY_BATCH");
    command
}
fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("fixture");
    warehouse_movement(
        pool,
        WarehouseRequest {
            effect_key: "covers",
            component_code: "DEMO-COVER",
            change: WarehouseChange::Receive(10),
            actor: "fixture-warehouse",
            reason: "isolated worker test",
        },
    )
    .await
    .expect("stock");
    for n in 1..=2 {
        reserve_bom(
            pool,
            HoldRequest {
                id: &format!("{n:032x}"),
                owner_id: "private-fixture-owner",
                sku: "DEMO-COVERS",
                revision: 1,
                units: 1,
                created_at: 1000,
                expires_at: 1100,
            },
        )
        .await
        .expect("hold");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn explicit_clocks_fail_before_any_mutation_and_database_errors_are_redacted(pool: PgPool) {
    seed(&pool).await;
    for clock in [
        FixedClock(Err(ContextError::ClockUnavailable)),
        FixedClock(Ok(u64::MAX)),
    ] {
        assert_eq!(
            expire_batch(&pool, &clock, 25).await,
            Err(WorkerError::ClockUnavailable)
        );
    }
    assert_eq!(
        expire_batch(&pool, &FixedClock(Ok(1099)), 25)
            .await
            .expect("future holds")
            .expired,
        0
    );
    assert_eq!(
        expire_batch(&pool, &FixedClock(Ok(1100)), 1)
            .await
            .expect("injected deadline")
            .expired,
        1
    );
    pool.close().await;
    assert_eq!(
        expire_batch(&pool, &FixedClock(Ok(1100)), 25).await,
        Err(WorkerError::ExpiryUnavailable)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn probe_is_read_only_and_expiry_command_has_a_real_batch_limit(pool: PgPool) {
    seed(&pool).await;
    let database = pool.connect_options().to_url_lossy().to_string();
    let probe = command()
        .env("DATABASE_URL", &database)
        .output()
        .expect("probe");
    assert!(probe.status.success());
    assert!(combined(&probe).contains("worker_dependencies_ready"));
    let (active,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_holds WHERE state='active'")
        .fetch_one(&pool)
        .await
        .expect("probe does not expire");
    assert_eq!(active, 2);
    let first = command()
        .arg("--expire-holds")
        .env("HOLD_EXPIRY_BATCH", "1")
        .env("DATABASE_URL", &database)
        .output()
        .expect("expiry command");
    assert!(first.status.success());
    assert!(combined(&first).contains("hold_expiry_batch"));
    assert!(!combined(&first).contains("private-fixture-owner"));
    assert!(!combined(&first).contains(&database));
    let (active,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_holds WHERE state='active'")
        .fetch_one(&pool)
        .await
        .expect("bounded pass");
    assert_eq!(active, 1);
    let remainder = command()
        .arg("--expire-holds")
        .env("DATABASE_URL", &database)
        .output()
        .expect("default limit");
    assert!(remainder.status.success());
    let (reserved,): (i64,) =
        sqlx::query_as("SELECT reserved FROM stock_balances WHERE component_code='DEMO-COVER'")
            .fetch_one(&pool)
            .await
            .expect("released");
    assert_eq!(reserved, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_expiry_command_reports_static_failure_and_preserves_stock(pool: PgPool) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION reject_worker_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.action='expire-hold' THEN RAISE EXCEPTION 'private provider-like diagnostic'; END IF; RETURN NEW; END; $$;
        CREATE TRIGGER reject_worker_expiry BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION reject_worker_expiry()")
        .await.expect("failure control");
    let database = pool.connect_options().to_url_lossy().to_string();
    let result = command()
        .arg("--expire-holds")
        .env("DATABASE_URL", &database)
        .output()
        .expect("worker");
    assert!(!result.status.success());
    let log = combined(&result);
    assert!(log.contains("hold expiry requires retry"));
    assert!(!log.contains("private provider-like diagnostic"));
    assert!(!log.contains("private-fixture-owner"));
    assert!(!log.contains(&database));
    let (reserved,): (i64,) =
        sqlx::query_as("SELECT reserved FROM stock_balances WHERE component_code='DEMO-COVER'")
            .fetch_one(&pool)
            .await
            .expect("not lost");
    assert_eq!(reserved, 4);
}

#[test]
fn invalid_command_limit_and_production_never_start_expiry() {
    for args in [
        vec!["--unknown-private-value"],
        vec!["--expire-holds", "extra"],
    ] {
        let result = command()
            .args(args)
            .env_remove("DATABASE_URL")
            .output()
            .expect("invalid command");
        assert!(!result.status.success());
        assert!(combined(&result).contains("unsupported worker command"));
        assert!(!combined(&result).contains("unknown-private-value"));
    }
    for limit in ["0", "101", "65536", "no-number", ""] {
        let result = command()
            .arg("--expire-holds")
            .env("HOLD_EXPIRY_BATCH", limit)
            .env_remove("DATABASE_URL")
            .output()
            .expect("invalid limit");
        assert!(!result.status.success());
        assert!(combined(&result).contains("invalid HOLD_EXPIRY_BATCH"));
    }
    let result = command()
        .arg("--expire-holds")
        .env("APP_ENV", "production")
        .output()
        .expect("production disabled");
    assert!(!result.status.success());
    assert!(combined(&result).contains("production is disabled"));
}

#[cfg(unix)]
#[test]
fn non_unicode_limit_is_rejected_without_logging_its_value() {
    use std::os::unix::ffi::OsStringExt;
    let result = command()
        .arg("--expire-holds")
        .env(
            "HOLD_EXPIRY_BATCH",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .env_remove("DATABASE_URL")
        .output()
        .expect("invalid encoding");
    assert!(!result.status.success());
    assert!(combined(&result).contains("invalid HOLD_EXPIRY_BATCH"));
}
