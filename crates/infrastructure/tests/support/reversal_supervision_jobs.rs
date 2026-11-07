use super::*;
use crate::payment_attempts::reversal_pages::ReversalPageReceipt;
use crate::{
    MIGRATOR,
    mollie::tests::reversal_supervision_cases::{fixture, verified_test_page},
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
    let (_, frozen, ids, observation) = fixture(pool).await;
    let b = &frozen.snapshot().binding;
    let id = "a".repeat(32);
    sqlx::query("INSERT INTO reversal_scan_jobs(id,attempt_id,observation_id,enrollment_key,profile_id,payment_id,reversal_kind,credential_fingerprint,state,generation,failures,created_at,updated_at,due_at) VALUES($1,$2,$3,'native-fixture',$4,'tr_SYNTHETIC_NotARealPayment','refund',$5,'pending',0,0,1500,1500,1500)")
        .bind(&id).bind(&b.id).bind(observation).bind(&b.profile.id).bind(&b.profile.credential_fingerprint)
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
        sqlx::query_scalar("SELECT count(*) FROM reversal_scan_operations WHERE job_id=$1")
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
    sqlx::query("SELECT id FROM reversal_scan_jobs WHERE id=$1 FOR UPDATE")
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
        ("reversal_scan_operations", "INSERT"),
        ("reversal_scan_jobs", "UPDATE"),
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
            "reversal_scan_operations",
            "INSERT",
            "NEW.id=repeat('f',32);",
        ),
        ("reversal_scan_jobs", "UPDATE", "NEW.due_at=NEW.due_at+1;"),
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
        finish(&pool, &At(1501), &c, Outcome::Page).await,
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
            "reversal_scan_operations",
            "RETURN NULL;",
            RecoveryError::InvalidStoredData,
        ),
        (
            "reversal_scan_jobs",
            "RETURN NULL;",
            RecoveryError::LeaseLost,
        ),
        (
            "reversal_scan_operations",
            "NEW.id=repeat('f',32); RETURN NEW;",
            RecoveryError::InvalidStoredData,
        ),
        (
            "reversal_scan_jobs",
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
            sqlx::query_scalar("SELECT outcome FROM reversal_scan_operations WHERE id=$1")
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

fn example_receipt(recorded_at: i64) -> ReversalPageReceipt {
    ReversalPageReceipt {
        id: "1".repeat(32),
        order_id: "2".repeat(32),
        attempt_id: "3".repeat(32),
        effect_key: "4".repeat(32),
        payment_id: "tr_synthetic".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "5".repeat(64),
        scan_id: "1".repeat(32),
        page_number: 31,
        previous_page_id: Some("6".repeat(32)),
        from_cursor: Some("re_current".into()),
        next_cursor: Some("re_next".into()),
        candidates: vec![],
        recorded_at,
        event_id: "7".repeat(32),
        replayed: true,
    }
}

#[test]
fn completed_page_limits_preserve_the_specific_review_reason() {
    use super::super::transitions::review_reason;
    let mut page = example_receipt(1500);
    assert_eq!(review_reason("page", 0, Some(&page), 127, 39), None);
    assert_eq!(
        review_reason("page", 0, Some(&page), 128, 39),
        Some("scan_limit")
    );
    assert_eq!(
        review_reason("page", 0, Some(&page), 127, 40),
        Some("scan_limit")
    );
    page.page_number = 32;
    assert_eq!(
        review_reason("page", 0, Some(&page), 127, 39),
        Some("scan_limit")
    );
    page.next_cursor = None;
    assert_eq!(
        review_reason("page", 0, Some(&page), 128, 40),
        Some("ended")
    );
    assert_eq!(
        review_reason("transient", 2, None, 0, 40),
        Some("scan_limit")
    );
    assert_eq!(
        review_reason("transient", 3, None, 0, 40),
        Some("retry_exhausted")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn checkpoint_clock_cannot_precede_a_receipt(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    let Some(Work::Claim(c)) = take(&pool, &j, &ids, &At(1500)).await.expect("claim") else {
        panic!("claim expected")
    };
    let mut tx = pool.begin().await.expect("checkpoint transaction");
    let future = example_receipt(1502);
    assert_eq!(
        transition(
            &mut tx,
            &At(1501),
            &c.job,
            &c.operation_id,
            1501,
            ("page", Some(&future), false)
        )
        .await,
        Err(RecoveryError::ClockUnavailable)
    );
    tx.rollback().await.expect("no transition");
    assert_state(&pool, &j.id, "leased", 1, 1).await;
    finish(&pool, &At(1502), &c, Outcome::Invalid)
        .await
        .expect("healthy exact clock");
}

#[test]
fn credential_fingerprint_requires_exact_lowercase_hex() {
    assert!(fingerprint(&"a1".repeat(32)));
    assert!(!fingerprint(&"a".repeat(63)));
    assert!(!fingerprint(&"A".repeat(64)));
}

#[test]
fn store_errors_preserve_clock_and_conflict_while_redacting_database_detail() {
    for (source, expected) in [
        (
            PaymentStoreError::ClockUnavailable,
            RecoveryError::ClockUnavailable,
        ),
        (PaymentStoreError::Conflict, RecoveryError::Conflict),
        (PaymentStoreError::Database, RecoveryError::Storage),
    ] {
        assert_eq!(RecoveryError::from(source), expected);
    }
    assert_eq!(
        RecoveryError::from(sqlx::Error::RowNotFound),
        RecoveryError::Storage
    );
    assert_eq!(kind_name(ReversalKind::Chargeback), "chargeback");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pre_io_fence_checks_receipt_and_tip_again_after_claim(pool: PgPool) {
    let (j, ids) = pending(&pool).await;
    let Some(Work::Claim(c)) = take(&pool, &j, &ids, &At(1500)).await.expect("claim root") else {
        panic!("root claim expected")
    };
    authorize(&pool, &At(1501), &c).await.expect("open root");
    let root_page = verified_test_page(&c.attempt, None, Some("re_NEXT")).await;
    let root = reversal_pages::record(&pool, &At(1501), &ids, &c.operation_id, None, &root_page)
        .await
        .expect("exact page already recorded");
    assert_eq!(
        authorize(&pool, &At(1501), &c).await,
        Err(RecoveryError::InvalidStoredData)
    );
    finish(&pool, &At(1501), &c, Outcome::Page)
        .await
        .expect("checkpoint exact receipt");
    let Some(Work::Claim(c)) = take(&pool, &j, &ids, &At(1502)).await.expect("claim next") else {
        panic!("continuation expected")
    };
    authorize(&pool, &At(1502), &c)
        .await
        .expect("open continuation");
    let cursor = reversal_pages::resume(&pool, &j.attempt_id, &root.id)
        .await
        .expect("validated root")
        .expect("open cursor");
    let next_page = verified_test_page(&c.attempt, Some(&cursor), None).await;
    reversal_pages::record(
        &pool,
        &At(1502),
        &ids,
        "external-next",
        Some(&root.id),
        &next_page,
    )
    .await
    .expect("other authenticated journal path followed tip");
    assert_eq!(
        authorize(&pool, &At(1502), &c).await,
        Err(RecoveryError::InvalidStoredData)
    );
    finish(&pool, &At(1502), &c, Outcome::Invalid)
        .await
        .expect("park followed tip");
    assert_state(&pool, &j.id, "review", 2, 2).await;
}
