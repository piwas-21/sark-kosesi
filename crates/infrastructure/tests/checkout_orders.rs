//! Private atomic checkout against invented isolated catalogue/customer evidence.
use commerce_application::Locale;
use commerce_domain::{guest_baskets::BasketUpdate, quotes::BasketLine};
use commerce_infrastructure::{MIGRATOR, checkout_orders::*, checkout_quotes::*, guest_baskets::*};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quote_fixture;
use quote_fixture::{guest, request, seed, store};
async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)").fetch_one(pool).await.expect("isolated counts")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn accepted_quote_atomically_freezes_order_whole_basket_hold_and_minimal_event(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (s, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let quote = issue_quote(&store, &key, &csrf, Locale::Nl, &request("first"))
        .await
        .expect("quote")
        .snapshot;
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
    let intent = fixture::intent(&quote.binding.quote_id);
    let receipt = create_order(
        &store,
        &key,
        &csrf,
        "place-first",
        &intent,
        &fixture::seller(),
    )
    .await
    .expect("accepted whole order");
    assert!(!receipt.replayed);
    let snapshot = receipt.snapshot;
    assert_eq!(
        (
            snapshot.quote.total.gross_minor,
            snapshot.quote.total.net_minor,
            snapshot.quote.total.vat_minor
        ),
        (38245, 31607, 6638)
    );
    assert!(snapshot.quote == quote);
    assert_eq!(snapshot.products[0].bom_revision, 1);
    assert_eq!(snapshot.products[0].publication_revision, 1);
    assert_eq!(
        (
            snapshot.identity.created_at,
            snapshot.identity.hold_expires_at
        ),
        (1100, 2000)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 2));
    let reserved: Vec<(String, i64)> = sqlx::query_as(
        "SELECT component_code,reserved FROM stock_balances ORDER BY component_code",
    )
    .fetch_all(&pool)
    .await
    .expect("balances");
    assert_eq!(
        reserved,
        vec![("SEAT-COVER".into(), 6), ("SEAT-FOAM".into(), 6)]
    );
    let event: serde_json::Value = sqlx::query_scalar("SELECT payload FROM commerce_outbox")
        .fetch_one(&pool)
        .await
        .expect("event");
    assert_eq!(
        event,
        serde_json::json!({"schema_version":1,"order_id":snapshot.identity.id})
    );
    time.0.store(2500, Ordering::SeqCst);
    let read = owned_order(&store, &key, &snapshot.identity.id)
        .await
        .expect("order survives indicative and seller expiry");
    assert!(read == snapshot);
    let retry = create_order(
        &store,
        &key,
        &csrf,
        "place-first",
        &intent,
        &fixture::seller(),
    )
    .await
    .expect("exact historical retry");
    assert!(retry.replayed && retry.snapshot == snapshot);
    let retry = create_order(
        &store,
        &key,
        &csrf,
        "alternate-key",
        &intent,
        &fixture::seller(),
    )
    .await
    .expect("one quote one order");
    assert!(retry.replayed && retry.snapshot == snapshot);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 2));
    for operation in [
        "UPDATE checkout_orders SET id=id",
        "DELETE FROM checkout_orders",
        "UPDATE commerce_outbox SET id=id",
        "DELETE FROM commerce_outbox",
    ] {
        let e = pool
            .execute(operation)
            .await
            .expect_err("immutable history");
        assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn ownership_csrf_changed_intent_and_revocation_protect_order_history(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (a, ac) = guest(&store).await;
    let (b, bc) = guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("A");
    let bk = GuestCredential::from_session(&b).expect("B");
    let q = issue_quote(&store, &ak, &ac, Locale::Nl, &request("a"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    assert!(matches!(
        create_order(&store, &ak, &bc, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::Csrf)
    ));
    assert!(matches!(
        create_order(&store, &bk, &bc, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::QuoteNotFound)
    ));
    let order = create_order(&store, &ak, &ac, "place", &intent, &fixture::seller())
        .await
        .expect("owned")
        .snapshot;
    assert!(matches!(
        owned_order(&store, &bk, &order.identity.id).await,
        Err(OrderStoreError::NotFound)
    ));
    for effect in ["place", "other"] {
        let mut changed = intent.clone();
        changed.buyer.email = "different@example.test".into();
        assert!(matches!(
            create_order(&store, &ak, &ac, effect, &changed, &fixture::seller()).await,
            Err(OrderStoreError::Conflict)
        ));
    }
    write_basket(
        &store,
        &ak,
        &ac,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "new-empty".into(),
            lines: vec![],
        },
    )
    .await
    .expect("later cart");
    assert!(
        create_order(&store, &ak, &ac, "place", &intent, &fixture::seller())
            .await
            .expect("receipt doesn't mutate current cart")
            .replayed
    );
    assert!(
        read_basket(&store, &ak)
            .await
            .expect("cart")
            .lines
            .is_empty()
    );
    revoke_session(&store, &a, &ac).await.expect("revoked");
    assert!(matches!(
        owned_order(&store, &ak, &order.identity.id).await,
        Err(OrderStoreError::Denied)
    ));
    assert!(matches!(
        create_order(&store, &ak, &ac, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_cart_expired_quote_and_unaccepted_seller_leave_no_partial_checkout(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (s, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &key, &csrf, Locale::Nl, &request("q"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    let mut seller = fixture::seller();
    seller.terms_version = "different-terms".into();
    assert!(matches!(
        create_order(&store, &key, &csrf, "place", &intent, &seller).await,
        Err(OrderStoreError::Unavailable)
    ));
    time.0.store(1400, Ordering::SeqCst);
    assert!(matches!(
        create_order(&store, &key, &csrf, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::QuoteExpired)
    ));
    time.0.store(1100, Ordering::SeqCst);
    write_basket(
        &store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "change".into(),
            lines: vec![BasketLine {
                sku: "SET-FILLED".into(),
                quantity: 2,
            }],
        },
    )
    .await
    .expect("different basket");
    assert!(matches!(
        create_order(&store, &key, &csrf, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::Conflict)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn two_shoppers_race_for_last_shared_components_with_one_complete_winner(pool: PgPool) {
    seed(&pool).await;
    pool.execute("UPDATE stock_balances SET on_hand=6")
        .await
        .expect("six physical components each");
    let (store, _) = store(&pool, 1100);
    let (a, ac) = guest(&store).await;
    let (b, bc) = guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("A");
    let bk = GuestCredential::from_session(&b).expect("B");
    let aq = issue_quote(&store, &ak, &ac, Locale::Nl, &request("a"))
        .await
        .expect("A indicative")
        .snapshot;
    let bq = issue_quote(&store, &bk, &bc, Locale::Nl, &request("b"))
        .await
        .expect("B indicative")
        .snapshot;
    let ai = fixture::intent(&aq.binding.quote_id);
    let bi = fixture::intent(&bq.binding.quote_id);
    let seller = fixture::seller();
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            create_order(&store, &ak, &ac, "place", &ai, &seller),
            create_order(&store, &bk, &bc, "place", &bi, &seller)
        )
    })
    .await
    .expect("globally ordered UPDATE claims finish without SHARE upgrade deadlock");
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(OrderStoreError::Unavailable))
            || matches!(b, Err(OrderStoreError::Unavailable))
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 2));
    let balances: Vec<(i64, i64)> =
        sqlx::query_as("SELECT on_hand,reserved FROM stock_balances ORDER BY component_code")
            .fetch_all(&pool)
            .await
            .expect("physical");
    assert_eq!(balances, vec![(6, 6), (6, 6)]);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_identical_intents_replay_one_order_without_extending_holds(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (s, c) = guest(&store).await;
    let k = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &k, &c, Locale::Nl, &request("q"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    let seller = fixture::seller();
    let (a, b) = tokio::join!(
        create_order(&store, &k, &c, "place", &intent, &seller),
        create_order(&store, &k, &c, "place", &intent, &seller)
    );
    let a = a.expect("A");
    let b = b.expect("B");
    assert!(a.snapshot == b.snapshot);
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn reviewed_market_revision_and_withdrawal_require_fresh_acceptance(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = store(&pool, 1100);
    let (s, c) = guest(&store).await;
    let k = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &k, &c, Locale::Nl, &request("q"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    let mut policy = quote_fixture::policy();
    policy.revision = 2;
    quote_fixture::draft(&pool, &policy).await;
    assert!(matches!(
        create_order(&store, &k, &c, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::Unavailable)
    ));
    quote_fixture::decide(&pool, 2, 0, true, "new-review").await;
    assert!(matches!(
        create_order(&store, &k, &c, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::QuoteChanged)
    ));
    let q = issue_quote(&store, &k, &c, Locale::Nl, &request("fresh"))
        .await
        .expect("fresh acceptance")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    quote_fixture::decide(&pool, 2, 1, false, "withdraw").await;
    assert!(matches!(
        create_order(&store, &k, &c, "place", &intent, &fixture::seller()).await,
        Err(OrderStoreError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
}
async fn waiting(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(2),async{loop{let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)").bind(pattern).fetch_one(pool).await.expect("actual lock observer");if waiting{return;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}).await.expect("reached actual write wait");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_quote_seller_clock_and_cancellation_checks_follow_actual_outbox_insert(
    pool: PgPool,
) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION pause_order_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(851236); RETURN NEW; END; $$;CREATE TRIGGER pause_order_event AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION pause_order_event()").await.expect("post-write instrument");
    let (store, time) = store(&pool, 1100);
    let (s, c) = guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &key, &c, Locale::Nl, &request("q"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    for mode in [
        "cancel",
        "quote-expired",
        "seller-expired",
        "rewind",
        "positive",
    ] {
        time.0.store(1100, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("gate");
        sqlx::query("SELECT pg_advisory_xact_lock(851236)")
            .execute(&mut *blocker)
            .await
            .expect("lock");
        let (ws, k, c, i) = (
            store.clone(),
            GuestCredential::from_session(&s).expect("owner"),
            c.clone(),
            intent.clone(),
        );
        let mut seller = fixture::seller();
        if mode == "seller-expired" {
            seller.expires_at = 1200;
        }
        let work =
            tokio::spawn(async move { create_order(&ws, &k, &c, "place", &i, &seller).await });
        waiting(&pool, "INSERT INTO commerce_outbox%").await;
        if mode == "cancel" {
            work.abort();
            assert!(work.await.err().expect("cancelled").is_cancelled());
            blocker.rollback().await.expect("unlock");
        } else {
            time.0.store(
                match mode {
                    "quote-expired" => 1400,
                    "seller-expired" => 1200,
                    "rewind" => 1099,
                    _ => 1100,
                },
                Ordering::SeqCst,
            );
            blocker.rollback().await.expect("resume");
            let result = work.await.expect("task");
            match mode {
                "quote-expired" => assert!(matches!(result, Err(OrderStoreError::QuoteExpired))),
                "seller-expired" => assert!(matches!(result, Err(OrderStoreError::Unavailable))),
                "rewind" => assert!(matches!(result, Err(OrderStoreError::ClockUnavailable))),
                _ => assert!(result.is_ok()),
            }
        }
        let n = i64::from(mode == "positive");
        assert_eq!(counts(&pool).await, (n, n, n, n, 2 * n));
        let reserved: i64 = sqlx::query_scalar("SELECT sum(reserved)::bigint FROM stock_balances")
            .fetch_one(&pool)
            .await
            .expect("atomic reservation");
        assert_eq!(reserved, 12 * n);
    }
}
