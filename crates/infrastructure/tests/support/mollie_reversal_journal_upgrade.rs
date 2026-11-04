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
