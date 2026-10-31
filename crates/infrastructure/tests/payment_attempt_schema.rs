//! Isolated raw-import guards with invented profile/configuration; no provider activity.
use commerce_application::{Locale, payment_requests::*};
use commerce_domain::{Currency, checkout_orders::OrderSnapshot, payment_attempts::*};
use commerce_infrastructure::{
    MIGRATOR, checkout_orders::create_order, checkout_quotes::issue_quote,
    guest_baskets::GuestCredential,
};
use sqlx::{Executor, PgPool, Postgres, Transaction};
#[path = "support/checkout_order_fixture.rs"]
mod orders;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quotes;
async fn candidate(pool: &PgPool) -> (Configuration, FrozenAttempt) {
    quotes::seed(pool).await;
    pool.execute("UPDATE stock_balances SET on_hand=6")
        .await
        .expect("exact scarce physical stock");
    let (store, _) = quotes::store(pool, 1100);
    let (s, c) = quotes::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &key, &c, Locale::Nl, &quotes::request("q"))
        .await
        .expect("quote")
        .snapshot;
    let order = create_order(
        &store,
        &key,
        &c,
        "place",
        &orders::intent(&q.binding.quote_id),
        &orders::seller(),
    )
    .await
    .expect("accepted order owns all six components")
    .snapshot;
    capture(&order)
}
fn capture(order: &OrderSnapshot) -> (Configuration, FrozenAttempt) {
    let cfg = Configuration::capture(ConfigurationSnapshot {
        schema_version: 1,
        profile: ProfileBinding {
            id: "pfl_SYNTHETIC_NotARealProfile".into(),
            credential_fingerprint: "a".repeat(64),
            mode: PaymentMode::Test,
        },
        method: PaymentMethod::Ideal,
        merchant_origin: "https://shop.example.test".into(),
        return_url: "https://shop.example.test/payment-return".into(),
        webhook_url: "https://shop.example.test/api/payment-webhook".into(),
    })
    .expect("synthetic configuration");
    let current = cfg.identity();
    let b = AttemptBinding {
        id: "7".repeat(32),
        order_id: order.identity.id.clone(),
        basket_id: order.identity.basket_id.clone(),
        stock_group_id: order.identity.stock_group_id.clone(),
        profile: current.profile,
        configuration_sha256: current.sha256,
        currency: Currency::EUR,
        amount_minor: 38245,
        created_at: 1500,
        dispatch_before: 2000,
    };
    let frozen = FrozenAttempt::capture(b, &cfg)
        .expect("frozen exact accepted amount after indicative quote expiry");
    (cfg, frozen)
}
async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    cfg: &Configuration,
    f: &FrozenAttempt,
) -> Result<(), sqlx::Error> {
    let s = f.snapshot();
    let b = &s.binding;
    sqlx::query("INSERT INTO payment_attempts(id,order_id,basket_id,effect_key,intent,idempotency_key,created_at,dispatch_before,snapshot,configuration,configuration_material,request_body) VALUES($1,$2,$3,'prepare',jsonb_build_object('order_id',$2::text,'confirmed',true),$4,$5,$6,$7,$8,$9,$10)").bind(&b.id).bind(&b.order_id).bind(&b.basket_id).bind(s.idempotency_key()).bind(b.created_at).bind(b.dispatch_before).bind(serde_json::to_value(s).expect("snapshot")).bind(serde_json::to_value(cfg.snapshot()).expect("policy")).bind(cfg.fingerprint_material()).bind(f.body()).execute(&mut **tx).await?;
    Ok(())
}
async fn prepared_event(tx: &mut Transaction<'_, Postgres>, f: &FrozenAttempt) {
    let b = &f.snapshot().binding;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.attempt_prepared.v1',$3,$4)").bind("8".repeat(32)).bind(&b.order_id).bind(serde_json::json!({"schema_version":1,"order_id":b.order_id,"attempt_id":b.id})).bind(b.created_at).execute(&mut **tx).await.expect("prepared event");
}
async fn start(
    tx: &mut Transaction<'_, Postgres>,
    f: &FrozenAttempt,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,$2,1600)")
        .bind(&f.snapshot().binding.id)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn recovery_event(tx: &mut Transaction<'_, Postgres>, f: &FrozenAttempt) {
    let b = &f.snapshot().binding;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reconciliation_required.v1',$3,1600)").bind("a".repeat(32)).bind(&b.order_id).bind(serde_json::json!({"schema_version":1,"order_id":b.order_id,"attempt_id":b.id,"dispatch_id":"9".repeat(32)})).execute(&mut **tx).await.expect("recovery custody");
}
async fn counts(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM payment_dispatch_starts),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements)").fetch_one(pool).await.expect("counts")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn immutable_attempt_and_one_dispatch_bind_exact_order_money_bytes_and_recovery_events(
    pool: PgPool,
) {
    let (cfg, f) = candidate(&pool).await;
    let mut tx = pool.begin().await.expect("atomic preparation");
    insert(&mut tx, &cfg, &f)
        .await
        .expect("valid captured intent");
    prepared_event(&mut tx, &f).await;
    tx.commit().await.expect("prepared complete");
    assert_eq!(counts(&pool).await, (1, 0, 2, 2));
    let stored:(i64,String,String,String)=sqlx::query_as("SELECT (snapshot->'binding'->>'amount_minor')::bigint,request_body::jsonb->'amount'->>'value',encode(sha256(convert_to(request_body,'UTF8')),'hex'),encode(sha256(convert_to(configuration_material,'UTF8')),'hex') FROM payment_attempts").fetch_one(&pool).await.expect("independent PostgreSQL digest binding");
    assert_eq!(stored.0, 38245);
    assert_eq!(stored.1, "382.45");
    assert_eq!(stored.2, f.snapshot().request_sha256);
    assert_eq!(stored.3, cfg.identity().sha256);
    let mut tx = pool.begin().await.expect("start custody");
    start(&mut tx, &f, &"9".repeat(32))
        .await
        .expect("one start");
    recovery_event(&mut tx, &f).await;
    tx.commit().await.expect("durable reconcile before any I/O");
    assert_eq!(counts(&pool).await, (1, 1, 3, 2));
    for statement in [
        "UPDATE payment_attempts SET id=id",
        "DELETE FROM payment_attempts",
        "UPDATE payment_dispatch_starts SET id=id",
        "DELETE FROM payment_dispatch_starts",
    ] {
        let e = pool
            .execute(statement)
            .await
            .expect_err("immutable custody");
        assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_preparation_or_recovery_event_rolls_back_and_another_start_cannot_replace_identity(
    pool: PgPool,
) {
    let (cfg, f) = candidate(&pool).await;
    let mut tx = pool.begin().await.expect("missing event");
    insert(&mut tx, &cfg, &f).await.expect("row");
    assert!(tx.commit().await.is_err());
    assert_eq!(counts(&pool).await, (0, 0, 1, 2));
    let mut tx = pool.begin().await.expect("complete preparation");
    insert(&mut tx, &cfg, &f).await.expect("row");
    prepared_event(&mut tx, &f).await;
    tx.commit().await.expect("prepared");
    let mut tx = pool.begin().await.expect("missing recovery");
    start(&mut tx, &f, &"9".repeat(32)).await.expect("row");
    assert!(tx.commit().await.is_err());
    assert_eq!(counts(&pool).await, (1, 0, 2, 2));
    let mut tx = pool.begin().await.expect("complete start");
    start(&mut tx, &f, &"9".repeat(32)).await.expect("row");
    recovery_event(&mut tx, &f).await;
    tx.commit().await.expect("started");
    let mut tx = pool.begin().await.expect("alternate start");
    let e = start(&mut tx, &f, &"b".repeat(32))
        .await
        .expect_err("unique attempt start");
    assert_eq!(
        e.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23505")
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (1, 1, 3, 2));
}
#[sqlx::test(migrations = false)]
async fn upgrade_preserves_accepted_orders_holds_grants_and_old_outbox_without_seeding_attempts(
    pool: PgPool,
) {
    MIGRATOR
        .run_to(22, &pool)
        .await
        .expect("accepted previous schema");
    let (_, f) = candidate(&pool).await;
    let before: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("grants");
    MIGRATOR.run(&pool).await.expect("upgrade");
    MIGRATOR.run(&pool).await.expect("idempotent runner");
    assert_eq!(counts(&pool).await, (0, 0, 1, 2));
    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("grants");
    assert_eq!(after, before);
    let order: String = sqlx::query_scalar("SELECT id FROM checkout_orders")
        .fetch_one(&pool)
        .await
        .expect("old source");
    assert_eq!(order, f.snapshot().binding.order_id);
}
