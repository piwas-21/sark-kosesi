//! Isolated original-attempt custody inspection; only synthetic local providers.
use super::observation_cases::{fetched, ids, prepared};
use super::*;
use crate::{
    MIGRATOR,
    payment_attempts::{
        identities,
        observations::record_verified,
        reconciliation::{custody::inspect_custody, *},
    },
};
use sqlx::{Executor, PgPool};

async fn total(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_dispatch_starts),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM payment_reconciliation_jobs),(SELECT count(*) FROM payment_observations),(SELECT count(*) FROM payment_identity_resolutions)").fetch_one(pool).await.expect("counts")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn start_without_job_is_inspected_without_creating_a_job(pool: PgPool) {
    let (_, f) = prepared(&pool, true).await;
    let before = total(&pool).await;
    let view = inspect_custody(&pool, &f.snapshot().binding.id)
        .await
        .expect("validated start");
    assert_eq!(
        (
            view.checkpoint_phase,
            view.checkpoint_review,
            view.checkpoint_page
        ),
        (None, None, false)
    );
    assert_eq!(
        (
            view.observations,
            view.identities,
            view.paid_observed,
            view.resolution_current
        ),
        (0, 0, false, None)
    );
    assert_eq!(total(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn nonexistent_unstarted_and_damaged_recovery_event_fail_closed(pool: PgPool) {
    let (time, f) = prepared(&pool, false).await;
    assert!(matches!(
        inspect_custody(&pool, "bad").await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(inspect_custody(&pool, &"f".repeat(32)).await.is_err());
    assert!(
        inspect_custody(&pool, &f.snapshot().binding.id)
            .await
            .is_err()
    );
    let id = &f.snapshot().binding.id;
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids(), id, &config(&credential())).await,
        Ok(DispatchOutcome::Start(_))
    ));
    assert!(inspect_custody(&pool, id).await.is_ok());
    let body: String = sqlx::query_scalar("SELECT request_body FROM payment_attempts WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("original request");
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt")
        .await
        .expect("fixture guard");
    sqlx::query("UPDATE payment_attempts SET request_body='{}' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("corrupt frozen bytes");
    assert!(matches!(
        inspect_custody(&pool, id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_attempts SET request_body=$2 WHERE id=$1")
        .bind(id)
        .bind(body)
        .execute(&pool)
        .await
        .expect("restore bytes");
    pool.execute("ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")
        .await
        .expect("restore guard");
    assert!(inspect_custody(&pool, id).await.is_ok());
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox; UPDATE commerce_outbox SET payload='{}' WHERE kind='payment.reconciliation_required.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox").await.expect("synthetic damaged event");
    assert!(matches!(
        inspect_custody(&pool, id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn identity_evidence_is_bounded_redacted_and_resolution_staleness_is_visible(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let id = &f.snapshot().binding.id;
    let i = ids();
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        id,
        "first",
        &fetched(f.snapshot(), "tr_FIRST", "paid", false).await,
    )
    .await
    .expect("paid fact");
    identities::resolve(&pool, time.as_ref(), &i, id, "resolution")
        .await
        .expect("resolution");
    let one = inspect_custody(&pool, id).await.expect("current");
    assert_eq!(
        (
            one.observations,
            one.identities,
            one.paid_observed,
            one.resolution_current
        ),
        (1, 1, true, Some(true))
    );
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        id,
        "second",
        &fetched(f.snapshot(), "tr_SECOND", "open", false).await,
    )
    .await
    .expect("second fact");
    let before = total(&pool).await;
    let two = inspect_custody(&pool, id).await.expect("ambiguous");
    assert_eq!(
        (
            two.observations,
            two.identities,
            two.paid_observed,
            two.resolution_current
        ),
        (2, 2, true, Some(false))
    );
    assert_eq!(total(&pool).await, before);
    let text = format!("{two:?}");
    assert!(!text.contains("tr_FIRST") && !text.contains(id));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox; UPDATE commerce_outbox SET payload='{}' WHERE kind='payment.observation_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox").await.expect("damage observation events");
    assert!(matches!(
        inspect_custody(&pool, id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn checkpoint_page_chain_is_checked_without_provider_io_or_database_writes(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let id = &f.snapshot().binding.id;
    let server = Server::start(vec![
        profile(),
        Reply::Json(
            200,
            json!({"count":0,"_embedded":{"payments":[]},"_links":{"next":null}}),
        ),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let i = ids();
    assert_eq!(
        run_batch(&pool, time.as_ref(), &i, &client, 1)
            .await
            .expect("one step")
            .processed,
        1
    );
    let before = total(&pool).await;
    let view = inspect_custody(&pool, id).await.expect("checkpoint chain");
    assert_eq!(view.checkpoint_phase, Some(Phase::Candidates));
    assert!(view.checkpoint_page);
    assert_eq!(total(&pool).await, before);
    assert_eq!(server.count(), 2, "inspection performed no GET");
    pool.execute("ALTER TABLE payment_reconciliation_jobs DISABLE TRIGGER guard_payment_reconciliation_job; UPDATE payment_reconciliation_jobs SET candidate_index=1; ALTER TABLE payment_reconciliation_jobs ENABLE TRIGGER guard_payment_reconciliation_job").await.expect("damage checkpoint index");
    assert!(matches!(
        inspect_custody(&pool, id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE payment_reconciliation_jobs DISABLE TRIGGER guard_payment_reconciliation_job; UPDATE payment_reconciliation_jobs SET candidate_index=0; ALTER TABLE payment_reconciliation_jobs ENABLE TRIGGER guard_payment_reconciliation_job").await.expect("restore index");
    run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("end scan");
    let stopped = inspect_custody(&pool, id).await.expect("review checkpoint");
    assert_eq!(stopped.checkpoint_review, Some(ReviewReason::Unresolved));
    assert_eq!(server.count(), 2);
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox; UPDATE commerce_outbox SET payload='{}' WHERE kind='payment.search_page_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox").await.expect("damage page event");
    assert!(matches!(
        inspect_custody(&pool, id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    assert_eq!(server.count(), 2);
}
