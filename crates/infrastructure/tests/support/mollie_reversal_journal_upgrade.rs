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

async fn verified_empty_page(attempt: &AttemptSnapshot) -> crate::mollie::ReversalPage {
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(attempt)),
        Reply::Json(
            200,
            json!({"count":0,"_embedded":{"refunds":[]},"_links":{"next":null}}),
        ),
    ]);
    local(&server)
        .verify_profile()
        .await
        .expect("test profile")
        .into_reconciliation()
        .reversal_page(attempt, PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("authenticated empty page")
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

    let verified = verified_empty_page(frozen.snapshot()).await;
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

#[sqlx::test(migrations = false)]
async fn schema46_preserves_page_evidence_and_pairs_native_scan_transitions(pool: PgPool) {
    MIGRATOR.run_to(45, &pool).await.expect("schema 45");
    let (_, frozen) = observation_cases::prepared(&pool, true).await;
    let binding = &frozen.snapshot().binding;
    let ids = observation_cases::ids();
    let observed = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let paid = record_verified(
        &pool,
        &At(1500),
        &ids,
        &binding.id,
        "before-scan",
        &observed,
    )
    .await
    .expect("original paid custody");
    let page = verified_empty_page(frozen.snapshot()).await;
    let original = crate::payment_attempts::reversal_pages::record(
        &pool,
        &At(1500),
        &ids,
        "old-page",
        None,
        &page,
    )
    .await
    .expect("pre-upgrade page evidence");
    let before = all_events(&pool).await;

    MIGRATOR.run_to(46, &pool).await.expect("schema 46");
    assert_eq!(all_events(&pool).await, before);
    let saved =
        crate::payment_attempts::reversal_pages::read_page(&pool, &binding.id, &original.id)
            .await
            .expect("original page and event survive");
    assert!(saved.candidates.is_empty());
    let job = "a".repeat(32);
    let operation = "b".repeat(32);
    sqlx::query("INSERT INTO reversal_scan_jobs(id,attempt_id,observation_id,enrollment_key,profile_id,payment_id,reversal_kind,credential_fingerprint,state,generation,failures,created_at,updated_at,due_at) VALUES($1,$2,$3,'native-upgrade',$4,$5,'refund',$6,'pending',0,0,1500,1500,1500)")
        .bind(&job).bind(&binding.id).bind(&paid.snapshot.id).bind(&binding.profile.id)
        .bind(PAYMENT).bind(observed.credential_fingerprint()).execute(&pool).await.expect("exact native enrollment");

    let start = "INSERT INTO reversal_scan_operations(id,job_id,generation,started_at,lease_until) VALUES($1,$2,1,1501,1561)";
    let lease = "UPDATE reversal_scan_jobs SET state='leased',generation=1,updated_at=1501,lease_until=1561 WHERE id=$1";
    assert!(
        sqlx::query(lease).bind(&job).execute(&pool).await.is_err(),
        "lease requires its operation"
    );
    let mut orphan = pool.begin().await.expect("orphan test");
    sqlx::query(start)
        .bind(&operation)
        .bind(&job)
        .execute(&mut *orphan)
        .await
        .expect("stage native start");
    assert!(orphan.commit().await.is_err(), "orphan start cannot commit");
    let mut paired = pool.begin().await.expect("paired start");
    sqlx::query(start)
        .bind(&operation)
        .bind(&job)
        .execute(&mut *paired)
        .await
        .expect("paired operation");
    sqlx::query(lease)
        .bind(&job)
        .execute(&mut *paired)
        .await
        .expect("paired lease");
    paired.commit().await.expect("durable start");

    let stop =
        "UPDATE reversal_scan_operations SET outcome='uncertain',finished_at=1561 WHERE id=$1";
    assert!(
        sqlx::query(
            "UPDATE reversal_scan_operations SET outcome='uncertain',finished_at=1560 WHERE id=$1"
        )
        .bind(&operation)
        .execute(&pool)
        .await
        .is_err(),
        "live lease cannot be parked as uncertain"
    );
    let mut orphan = pool.begin().await.expect("orphan outcome test");
    sqlx::query(stop)
        .bind(&operation)
        .execute(&mut *orphan)
        .await
        .expect("stage native outcome");
    assert!(
        orphan.commit().await.is_err(),
        "orphan outcome cannot commit"
    );
    let mut paired = pool.begin().await.expect("paired uncertainty");
    sqlx::query(stop)
        .bind(&operation)
        .execute(&mut *paired)
        .await
        .expect("expired operation");
    sqlx::query("UPDATE reversal_scan_jobs SET state='review',review_reason='uncertain',lease_until=NULL,updated_at=1561,due_at=1561 WHERE id=$1")
        .bind(&job).execute(&mut *paired).await.expect("park uncertain");
    paired.commit().await.expect("durable uncertainty");
    let late = crate::payment_attempts::reversal_pages::record(
        &pool,
        &At(1562),
        &ids,
        &operation,
        None,
        &page,
    )
    .await
    .expect("late evidence can still be retained");
    assert_eq!(late.effect_key, operation);
    let state: (String, Option<String>, Option<String>, i32) = sqlx::query_as(
        "SELECT state,review_reason,tip_page_id,generation FROM reversal_scan_jobs WHERE id=$1",
    )
    .bind(job)
    .fetch_one(&pool)
    .await
    .expect("retained uncertain checkpoint");
    assert_eq!(state, ("review".into(), Some("uncertain".into()), None, 1));
    let after = all_events(&pool).await;
    assert_eq!(after.len(), before.len() + 1);
    assert!(before.iter().all(|row| after.contains(row)));
}
