use super::*;
use crate::payment_attempts::observations::record_verified;
use commerce_application::request_context::{Clock, ContextError};

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}

async fn all_events(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e ORDER BY id")
        .fetch_all(pool)
        .await
        .expect("complete event rows")
}

#[sqlx::test(migrations = false)]
async fn schema44_preserves_prior_outbox_and_accepts_both_evidence_kinds(pool: PgPool) {
    MIGRATOR.run_to(43, &pool).await.expect("schema 43");
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    record_verified(&pool, time.as_ref(), &ids, attempt, "legacy-paid", &paid)
        .await
        .expect("legacy authenticated payment event");
    let before = all_events(&pool).await;

    MIGRATOR.run_to(44, &pool).await.expect("schema 44");
    assert_eq!(all_events(&pool).await, before);
    record_verified(&pool, time.as_ref(), &ids, attempt, "legacy-new", &paid)
        .await
        .expect("legacy guard accepts another exact payment event");
    let fact = fetched(frozen.snapshot(), refund("refunded", "395.00"), false).await;
    let stored = record(&pool, &At(1501), &ids, "upgrade-reversal", &fact)
        .await
        .expect("new guard accepts exact reversal");
    assert_eq!(stored.amount_minor, 39500);
    assert_eq!(stored.recorded_at, 1501);
    let after = all_events(&pool).await;
    assert_eq!(after.len(), before.len() + 2);
    assert!(before.iter().all(|row| after.contains(row)));

    let error = sqlx::query("INSERT INTO payment_reversal_observations(id,attempt_id,order_id,effect_key,payment_id,child_id,phase,amount_minor,credential_fingerprint,source_created_seconds,source_created_nanos,recorded_at,event_id) SELECT $1,attempt_id,order_id,'native-rewind',payment_id,child_id,phase,amount_minor,credential_fingerprint,source_created_seconds,source_created_nanos,1500,$2 FROM payment_reversal_observations WHERE id=$3")
        .bind("a".repeat(32))
        .bind("b".repeat(32))
        .bind(&stored.id)
        .execute(&pool)
        .await
        .expect_err("database rejects recording time below existing evidence");
    let database = error.as_database_error().expect("native custody error");
    assert_eq!(database.code().as_deref(), Some("23514"));
    assert_eq!(
        database.message(),
        "Reversal observation recording time must not regress"
    );
    assert_eq!(all_events(&pool).await, after);
}

#[sqlx::test(migrations = false)]
async fn schema45_preserves_exact_observations_and_adds_page_custody(pool: PgPool) {
    MIGRATOR.run_to(44, &pool).await.expect("schema 44");
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let original = record(&pool, time.as_ref(), &ids, "before-page-upgrade", &fact)
        .await
        .expect("existing reversal custody");
    let before = all_events(&pool).await;

    MIGRATOR.run_to(45, &pool).await.expect("schema 45");
    assert_eq!(all_events(&pool).await, before);
    let preserved = lookup(&pool, attempt, "before-page-upgrade")
        .await
        .expect("existing digest still verifies")
        .expect("existing receipt");
    assert_eq!(preserved.id, original.id);
    assert_eq!(preserved.amount_minor, 1234);
    assert!(preserved.replayed);
    record(&pool, &At(1501), &ids, "after-page-upgrade", &fact)
        .await
        .expect("updated guard still accepts observations");

    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(
            200,
            json!({"count":0,"_embedded":{"refunds":[]},"_links":{"next":null}}),
        ),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("test profile")
        .into_reconciliation();
    let verified = client
        .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("authenticated empty page");
    let page = crate::payment_attempts::reversal_pages::record(
        &pool,
        &At(1501),
        &ids,
        "page-after-upgrade",
        None,
        &verified,
    )
    .await
    .expect("new guard accepts exact page custody");
    assert_eq!(page.scan_id, page.id);
    assert_eq!(page.page_number, 1);
    assert!(page.candidates.is_empty());
    assert!(page.next_cursor.is_none());
    let after = all_events(&pool).await;
    assert_eq!(after.len(), before.len() + 2);
    assert!(before.iter().all(|row| after.contains(row)));
}
