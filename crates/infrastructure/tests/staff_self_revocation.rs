//! Isolated tests for relinquishing, never granting, staff authority.
use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::inventory::WarehouseChange;
use commerce_infrastructure::{
    MIGRATOR,
    staff_access::{RevocationError, SessionToken, StaffError, revoke_self},
    staff_operations::{StaffWarehouseRequest, warehouse_change},
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct Time(u64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
struct Backward(AtomicUsize);
impl Clock for Backward {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            1200
        } else {
            1100
        })
    }
}
struct LateBackward(AtomicUsize);
impl Clock for LateBackward {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(if self.0.fetch_add(1, Ordering::SeqCst) < 2 {
            1200
        } else {
            1100
        })
    }
}
fn token(byte: u8) -> SessionToken {
    SessionToken::from_bytes([byte; 32])
}
fn request(key: &str) -> StaffWarehouseRequest<'_> {
    StaffWarehouseRequest {
        effect_key: key,
        component_code: "DEMO-COVER",
        change: WarehouseChange::Receive(1),
        reason: "synthetic test",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("isolated catalogue");
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('reader',true),('other',true); INSERT INTO staff_capabilities(staff_id,capability) VALUES('reader','warehouse.manage'),('other','warehouse.manage'); INSERT INTO stock_balances(component_code) VALUES('DEMO-COVER')").await.expect("isolated staff and stock");
    for (byte, staff, expires, mfa) in [
        (1_u8, "reader", 4600_i64, Some(1100_i64)),
        (2, "reader", 4600, Some(1100)),
        (3, "other", 4600, Some(1100)),
        (4, "reader", 1150, None),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,$3,$4)")
            .bind(token(byte).digest().as_slice()).bind(staff).bind(expires).bind(mfa).execute(pool).await.expect("isolated session");
    }
}
async fn revoked_at(pool: &PgPool, byte: u8) -> Option<i64> {
    sqlx::query_scalar("SELECT revoked_at FROM staff_sessions WHERE token_hash=$1")
        .bind(token(byte).digest().as_slice())
        .fetch_one(pool)
        .await
        .expect("fixture session")
}
async fn wait_for_lock(pool: &PgPool, prefix: &str) {
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)")
                .bind(format!("{prefix}%")).fetch_one(pool).await.expect("lock observation");
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("operation reached lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_revoke_replays_without_disclosure_and_preserves_other_sessions(pool: PgPool) {
    seed(&pool).await;
    assert!(revoke_self(&pool, &token(99), &Time(2001)).await.is_ok());
    assert!(revoke_self(&pool, &token(1), &Time(2001)).await.is_ok());
    assert_eq!(revoked_at(&pool, 1).await, Some(2001));
    assert!(revoke_self(&pool, &token(1), &Time(2002)).await.is_ok());
    assert_eq!(revoked_at(&pool, 1).await, Some(2001));
    // A clock earlier than this existing revocation fails closed instead of clamping.
    assert!(matches!(
        revoke_self(&pool, &token(1), &Time(2000)).await,
        Err(RevocationError::ClockUnavailable)
    ));
    assert_eq!(revoked_at(&pool, 2).await, None);
    assert_eq!(revoked_at(&pool, 3).await, None);
    // Expired and never-MFA-verified sessions can also relinquish authority.
    assert!(revoke_self(&pool, &token(4), &Time(2001)).await.is_ok());
    assert_eq!(revoked_at(&pool, 4).await, Some(2001));
    pool.execute("UPDATE staff_accounts SET enabled=false WHERE id='other'; DELETE FROM staff_capabilities WHERE staff_id='other'")
        .await.expect("disabled unprivileged staff");
    assert!(revoke_self(&pool, &token(3), &Time(2001)).await.is_ok());
    assert_eq!(revoked_at(&pool, 3).await, Some(2001));
    assert!(matches!(
        warehouse_change(&pool, &token(1), &Time(2001), request("revoked")).await,
        Err(StaffError::Denied)
    ));
    assert!(
        warehouse_change(&pool, &token(2), &Time(1100), request("other-session"))
            .await
            .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_failure_or_backward_observation_cannot_revoke(pool: PgPool) {
    seed(&pool).await;
    assert!(matches!(
        revoke_self(&pool, &token(1), &Backward(AtomicUsize::new(0))).await,
        Err(RevocationError::ClockUnavailable)
    ));
    assert!(matches!(
        revoke_self(&pool, &token(1), &Time(999)).await,
        Err(RevocationError::ClockUnavailable)
    ));
    assert!(matches!(
        revoke_self(&pool, &token(1), &Time(u64::MAX)).await,
        Err(RevocationError::ClockUnavailable)
    ));
    assert!(matches!(
        revoke_self(&pool, &token(1), &LateBackward(AtomicUsize::new(0))).await,
        Err(RevocationError::ClockUnavailable)
    ));
    assert_eq!(revoked_at(&pool, 1).await, None);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_or_altered_write_rolls_back(pool: PgPool) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION suppress_revoke() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER suppress_revoke BEFORE UPDATE ON staff_sessions FOR EACH ROW EXECUTE FUNCTION suppress_revoke()")
        .await.expect("synthetic suppression");
    assert!(matches!(
        revoke_self(&pool, &token(1), &Time(1100)).await,
        Err(RevocationError::Unavailable)
    ));
    assert_eq!(revoked_at(&pool, 1).await, None);
    pool.execute("DROP TRIGGER suppress_revoke ON staff_sessions; CREATE FUNCTION alter_revoke() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.revoked_at := NEW.revoked_at + 1; RETURN NEW; END $$; CREATE TRIGGER alter_revoke BEFORE UPDATE ON staff_sessions FOR EACH ROW EXECUTE FUNCTION alter_revoke()")
        .await.expect("synthetic altered result");
    assert!(matches!(
        revoke_self(&pool, &token(1), &Time(1100)).await,
        Err(RevocationError::Unavailable)
    ));
    assert_eq!(revoked_at(&pool, 1).await, None);
    pool.execute("DROP TRIGGER alter_revoke ON staff_sessions")
        .await
        .expect("restore");
    assert!(revoke_self(&pool, &token(1), &Time(1100)).await.is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn database_diagnostic_is_closed_and_rolls_back(pool: PgPool) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION fail_revoke() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'SYNTHETIC_SECRET_MARKER' USING DETAIL = 'SYNTHETIC_PRIVATE_DETAIL'; END $$; CREATE TRIGGER fail_revoke BEFORE UPDATE ON staff_sessions FOR EACH ROW EXECUTE FUNCTION fail_revoke()")
        .await.expect("synthetic error trigger");
    let error = revoke_self(&pool, &token(1), &Time(1100))
        .await
        .expect_err("write must fail");
    assert_eq!(error, RevocationError::Unavailable);
    let diagnostic = format!("{error:?}");
    assert!(!diagnostic.contains("SYNTHETIC_SECRET_MARKER"));
    assert!(!diagnostic.contains("SYNTHETIC_PRIVATE_DETAIL"));
    assert_eq!(revoked_at(&pool, 1).await, None);
    assert_eq!(revoked_at(&pool, 2).await, None);
    pool.execute("DROP TRIGGER fail_revoke ON staff_sessions")
        .await
        .expect("restore");
    assert!(revoke_self(&pool, &token(1), &Time(1100)).await.is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn active_operation_finishes_before_waiting_revocation(pool: PgPool) {
    seed(&pool).await;
    let mut block = pool.begin().await.expect("stock blocker");
    sqlx::query(
        "SELECT component_code FROM stock_balances WHERE component_code='DEMO-COVER' FOR UPDATE",
    )
    .execute(&mut *block)
    .await
    .expect("block stock");
    let p = pool.clone();
    let work = tokio::spawn(async move {
        warehouse_change(&p, &token(1), &Time(1100), request("held-operation")).await
    });
    wait_for_lock(
        &pool,
        "SELECT on_hand,quarantine,reserved FROM stock_balances",
    )
    .await;
    let p = pool.clone();
    let revoke = tokio::spawn(async move { revoke_self(&p, &token(1), &Time(1101)).await });
    wait_for_lock(&pool, "SELECT created_at,revoked_at FROM staff_sessions").await;
    block.rollback().await.expect("release stock");
    assert!(work.await.expect("operation task").is_ok());
    assert!(revoke.await.expect("revoke task").is_ok());
    assert_eq!(revoked_at(&pool, 1).await, Some(1101));
    assert!(matches!(
        warehouse_change(&pool, &token(1), &Time(1102), request("after-revoke")).await,
        Err(StaffError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn queued_revocation_precedes_waiting_staff_operation(pool: PgPool) {
    seed(&pool).await;
    let mut block = pool.begin().await.expect("session blocker");
    sqlx::query("SELECT token_hash FROM staff_sessions WHERE token_hash=$1 FOR UPDATE")
        .bind(token(1).digest().as_slice())
        .execute(&mut *block)
        .await
        .expect("block exact session");
    let p = pool.clone();
    let revoke = tokio::spawn(async move { revoke_self(&p, &token(1), &Time(1101)).await });
    wait_for_lock(&pool, "SELECT created_at,revoked_at FROM staff_sessions").await;
    let p = pool.clone();
    let work = tokio::spawn(async move {
        warehouse_change(&p, &token(1), &Time(1101), request("queued-operation")).await
    });
    wait_for_lock(&pool, "SELECT s.staff_id,a.enabled,s.created_at").await;
    block.rollback().await.expect("release session");
    assert!(revoke.await.expect("revoke task").is_ok());
    assert!(matches!(
        work.await.expect("operation task"),
        Err(StaffError::Denied)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM stock_movements")
        .fetch_one(&pool)
        .await
        .expect("no movement");
    assert_eq!(count, 0);
}
