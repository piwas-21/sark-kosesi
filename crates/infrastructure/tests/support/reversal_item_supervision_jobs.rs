use super::*;
use crate::payment_attempts::reversals::ReversalReceipt;
use crate::{
    MIGRATOR,
    mollie::tests::{
        reversal_item_supervision_cases::{page_fixture, verified_test_item},
        reversal_supervision_cases::fixture,
    },
};
use commerce_application::request_context::ContextError;
use sqlx::Executor;
use std::sync::atomic::{AtomicUsize, Ordering};

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
struct Samples(AtomicUsize, [u64; 2]);
impl Clock for Samples {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.1[self.0.fetch_add(1, Ordering::Relaxed).min(1)])
    }
}

async fn pending(pool: &PgPool) -> (Job, crate::payment_test_fixture::Ids) {
    let (time, frozen, ids, observation) = fixture(pool).await;
    let page = page_fixture(
        pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "item-job-source",
    )
    .await;
    let b = &frozen.snapshot().binding;
    let id = "a".repeat(32);
    sqlx::query("INSERT INTO reversal_item_jobs(id,attempt_id,observation_id,page_id,ordinal,child_id,enrollment_key,profile_id,payment_id,reversal_kind,credential_fingerprint,state,generation,failures,created_at,updated_at,due_at) VALUES($1,$2,$3,$4,1,'re_ITEM','native-fixture',$5,'tr_SYNTHETIC_NotARealPayment','refund',$6,'pending',0,0,1500,1500,1500)")
        .bind(&id).bind(&b.id).bind(observation).bind(page).bind(&b.profile.id).bind(&b.profile.credential_fingerprint)
        .execute(pool).await.expect("native pending fixture");
    (
        read_job(pool, &id)
            .await
            .expect("job read")
            .expect("present"),
        ids,
    )
}

async fn take(
    pool: &PgPool,
    j: &Job,
    ids: &dyn Identifiers,
    clock: &dyn Clock,
) -> Result<Option<Work>, RecoveryError> {
    claim(pool, clock, ids, &j.profile_id, &j.credential_fingerprint).await
}

async fn assert_state(pool: &PgPool, id: &str, state: &str, generation: i32, operations: i64) {
    let row = read_job(pool, id)
        .await
        .expect("job read")
        .expect("present");
    assert_eq!((row.state.as_str(), row.generation), (state, generation));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reversal_item_operations WHERE job_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("operations");
    assert_eq!(count, operations);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn claim_rechecks_scope_due_and_a_busy_job(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    let matches = |job: &Job| {
        eligible(
            job,
            &j.attempt_id,
            &j.profile_id,
            &j.credential_fingerprint,
            1500,
        )
    };
    assert!(matches(&j));
    type Mutation = fn(&mut Job);
    let cases: [Mutation; 7] = [
        |job| job.attempt_id = "f".repeat(32),
        |job| job.profile_id = "pfl_other".into(),
        |job| job.credential_fingerprint = "f".repeat(64),
        |job| job.updated_at = 1501,
        |job| job.due_at = 1501,
        |job| {
            job.state = "leased".into();
            job.lease_until = Some(1501);
        },
        |job| job.state = "review".into(),
    ];
    for change in cases {
        let mut stale = j.clone();
        change(&mut stale);
        assert!(!matches(&stale));
    }
    let mut expired = j.clone();
    expired.state = "leased".into();
    expired.lease_until = Some(1500);
    assert!(matches(&expired));

    let mut lock = pool.begin().await.expect("hold job");
    sqlx::query("SELECT id FROM reversal_item_jobs WHERE id=$1 FOR UPDATE")
        .bind(&j.id)
        .fetch_one(&mut *lock)
        .await
        .expect("real row lock");
    assert!(matches!(take(&pool, &j, &ids, &At(1500)).await, Ok(None)));
    lock.rollback().await.expect("release row");
    assert!(matches!(
        take(&pool, &j, &ids, &At(1500)).await,
        Ok(Some(Work::Claim(_)))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_or_suppressed_start_never_returns_a_claim(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    pool.execute("CREATE FUNCTION scan_fixture_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$").await.expect("fault function");
    for (table, event) in [
        ("reversal_item_operations", "INSERT"),
        ("reversal_item_jobs", "UPDATE"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER zz_scan_fixture BEFORE {event} ON {table} FOR EACH ROW EXECUTE FUNCTION scan_fixture_write()"))).execute(&pool).await.expect("suppress half");
        assert!(matches!(
            take(&pool, &j, &ids, &At(1500)).await,
            Err(RecoveryError::InvalidStoredData)
        ));
        assert_state(&pool, &j.id, "pending", 0, 0).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP TRIGGER zz_scan_fixture ON {table}"
        )))
        .execute(&pool)
        .await
        .expect("restore half");
    }
    for (table, event, mutation) in [
        (
            "reversal_item_operations",
            "INSERT",
            "NEW.id=repeat('f',32);",
        ),
        ("reversal_item_jobs", "UPDATE", "NEW.due_at=NEW.due_at+1;"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE OR REPLACE FUNCTION scan_fixture_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN {mutation} RETURN NEW; END; $$"))).execute(&pool).await.expect("alter valid-looking half");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER zz_scan_fixture BEFORE {event} ON {table} FOR EACH ROW EXECUTE FUNCTION scan_fixture_write()"))).execute(&pool).await.expect("mutate after guard");
        assert!(matches!(
            take(&pool, &j, &ids, &At(1500)).await,
            Err(RecoveryError::InvalidStoredData)
        ));
        assert_state(&pool, &j.id, "pending", 0, 0).await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP TRIGGER zz_scan_fixture ON {table}"
        )))
        .execute(&pool)
        .await
        .expect("restore guard");
    }
    for pair in [[1500, 1499], [1500, 1560]] {
        assert!(matches!(
            take(&pool, &j, &ids, &Samples(0.into(), pair)).await,
            Err(RecoveryError::LeaseLost)
        ));
        assert_state(&pool, &j.id, "pending", 0, 0).await;
    }
    assert!(matches!(
        take(&pool, &j, &ids, &At(1500)).await,
        Ok(Some(Work::Claim(_)))
    ));
    assert_state(&pool, &j.id, "leased", 1, 1).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mismatched_claim_clock_and_outcome_leave_the_exact_lease(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    let Some(Work::Claim(mut c)) = take(&pool, &j, &ids, &At(1500)).await.expect("claim") else {
        panic!("claim expected")
    };
    let real_id = c.operation_id.clone();
    c.operation_id = "f".repeat(32);
    assert_eq!(
        authorize(&pool, &At(1501), &c).await,
        Err(RecoveryError::LeaseLost)
    );
    c.operation_id = real_id;
    let original = c.attempt.request_sha256.clone();
    c.attempt.request_sha256 = "f".repeat(64);
    assert_eq!(
        authorize(&pool, &At(1501), &c).await,
        Err(RecoveryError::LeaseLost)
    );
    c.attempt.request_sha256 = original;
    c.job.due_at += 1;
    assert_eq!(
        authorize(&pool, &At(1501), &c).await,
        Err(RecoveryError::LeaseLost)
    );
    c.job.due_at -= 1;
    for clock in [At(1499), At(1560)] {
        assert_eq!(
            authorize(&pool, &clock, &c).await,
            Err(RecoveryError::LeaseLost)
        );
        assert_eq!(
            finish(&pool, &clock, &c, Outcome::Invalid).await,
            Err(RecoveryError::LeaseLost)
        );
    }
    assert_eq!(
        finish(&pool, &At(1501), &c, Outcome::Observed).await,
        Err(RecoveryError::InvalidStoredData)
    );
    assert_eq!(
        finish(&pool, &At(1501), &c, Outcome::Capped).await,
        Err(RecoveryError::InvalidStoredData)
    );
    assert_state(&pool, &j.id, "leased", 1, 1).await;
    authorize(&pool, &At(1501), &c)
        .await
        .expect("exact live claim");
    finish(&pool, &At(1501), &c, Outcome::Invalid)
        .await
        .expect("exact invalid stop");
    assert_state(&pool, &j.id, "review", 1, 1).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_or_altered_checkpoint_rolls_back_both_halves(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    let Some(Work::Claim(c)) = take(&pool, &j, &ids, &At(1500)).await.expect("claim") else {
        panic!("claim expected")
    };
    for (table, body, error) in [
        (
            "reversal_item_operations",
            "RETURN NULL;",
            RecoveryError::InvalidStoredData,
        ),
        (
            "reversal_item_jobs",
            "RETURN NULL;",
            RecoveryError::LeaseLost,
        ),
        (
            "reversal_item_operations",
            "NEW.id=repeat('f',32); RETURN NEW;",
            RecoveryError::InvalidStoredData,
        ),
        (
            "reversal_item_jobs",
            "NEW.due_at=NEW.due_at+1; RETURN NEW;",
            RecoveryError::InvalidStoredData,
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE OR REPLACE FUNCTION scan_fixture_finish() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN {body} END; $$"))).execute(&pool).await.expect("fault function");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER zz_scan_fixture BEFORE UPDATE ON {table} FOR EACH ROW EXECUTE FUNCTION scan_fixture_finish()"))).execute(&pool).await.expect("fault trigger");
        assert_eq!(
            finish(&pool, &At(1501), &c, Outcome::Transient).await,
            Err(error)
        );
        assert_state(&pool, &j.id, "leased", 1, 1).await;
        let outcome: Option<String> =
            sqlx::query_scalar("SELECT outcome FROM reversal_item_operations WHERE id=$1")
                .bind(&c.operation_id)
                .fetch_one(&pool)
                .await
                .expect("original operation");
        assert_eq!(outcome, None);
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP TRIGGER zz_scan_fixture ON {table}"
        )))
        .execute(&pool)
        .await
        .expect("restore guard");
    }
    for pair in [[1501, 1500], [1501, 1560]] {
        assert_eq!(
            finish(&pool, &Samples(0.into(), pair), &c, Outcome::Transient).await,
            Err(RecoveryError::LeaseLost)
        );
        assert_state(&pool, &j.id, "leased", 1, 1).await;
    }
    finish(&pool, &At(1501), &c, Outcome::Transient)
        .await
        .expect("healthy checkpoint");
    assert_state(&pool, &j.id, "pending", 1, 1).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_fence_and_pre_io_check_reject_future_or_already_recorded_evidence(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    let Some(Work::Claim(c)) = take(&pool, &j, &ids, &At(1500)).await.expect("claim") else {
        panic!("claim expected")
    };
    authorize(&pool, &At(1501), &c)
        .await
        .expect("original live claim");
    let fact = verified_test_item(&c.attempt, ReversalKind::Refund).await;
    let receipt: ReversalReceipt =
        reversals::record(&pool, &At(1502), &ids, &c.operation_id, &fact)
            .await
            .expect("authentic later receipt");
    assert_eq!(
        authorize(&pool, &At(1501), &c).await,
        Err(RecoveryError::InvalidStoredData)
    );
    assert_eq!(
        finish(&pool, &At(1501), &c, Outcome::Observed).await,
        Err(RecoveryError::ClockUnavailable)
    );
    assert_state(&pool, &j.id, "leased", 1, 1).await;
    finish(&pool, &At(1502), &c, Outcome::Observed)
        .await
        .expect("exact clock and receipt");
    let completed = read_job(&pool, &j.id)
        .await
        .expect("validated read")
        .expect("observed job");
    assert_eq!(completed.receipt_id.as_deref(), Some(receipt.id.as_str()));
    assert!(
        read_late_receipt(&pool, &j.id)
            .await
            .expect("observed is not late review")
            .is_none()
    );
}
