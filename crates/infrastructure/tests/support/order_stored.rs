use super::*;
use crate::MIGRATOR;
use crate::payment_test_fixture::order::snapshot_fixture as fixture;
#[sqlx::test(migrator = "MIGRATOR")]
async fn serialization_envelopes_reject_oversized_trusted_arguments_before_any_storage(
    pool: sqlx::PgPool,
) {
    let snapshot = fixture::snapshot();
    let mut intent = OrderIntent {
        quote_id: snapshot.identity.quote_id.clone(),
        expected_basket_revision: 1,
        buyer: snapshot.buyer.clone(),
        shipping_address: snapshot.shipping_address.clone(),
        billing_address: snapshot.billing_address.clone(),
        accepted_terms_version: snapshot.seller.terms_version.clone(),
        confirmed: true,
    };
    let mut oversized = snapshot.clone();
    oversized.products[0].title = "x".repeat(65537);
    let mut tx = pool.begin().await.expect("isolated transaction");
    assert!(matches!(
        insert(&mut tx, &oversized, &intent, "place", &"5".repeat(32)).await,
        Err(OrderStoreError::InvalidRequest)
    ));
    tx.rollback().await.expect("rollback");
    let valid_intent = intent.clone();
    intent.buyer.name = "x".repeat(16385);
    let mut tx = pool.begin().await.expect("isolated transaction");
    assert!(matches!(
        insert(&mut tx, &snapshot, &intent, "place", &"5".repeat(32)).await,
        Err(OrderStoreError::InvalidRequest)
    ));
    tx.rollback().await.expect("rollback");
    // Positive envelope control reaches the relational boundary; this pure fixture
    // deliberately has no owned quote/guest rows and must fail there instead.
    let mut tx = pool.begin().await.expect("isolated transaction");
    assert!(matches!(
        insert(&mut tx, &snapshot, &valid_intent, "place", &"5".repeat(32)).await,
        Err(OrderStoreError::Database)
    ));
    tx.rollback().await.expect("rollback");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_orders")
        .fetch_one(&pool)
        .await
        .expect("no storage");
    assert_eq!(count, 0);
}

fn stored_row() -> Row {
    let snapshot = fixture::snapshot();
    let intent = OrderIntent {
        quote_id: snapshot.identity.quote_id.clone(),
        expected_basket_revision: 1,
        buyer: snapshot.buyer.clone(),
        shipping_address: snapshot.shipping_address.clone(),
        billing_address: snapshot.billing_address.clone(),
        accepted_terms_version: snapshot.seller.terms_version.clone(),
        confirmed: true,
    };
    Row {
        id: snapshot.identity.id.clone(),
        basket_id: snapshot.identity.basket_id.clone(),
        basket_revision: 1,
        quote_id: snapshot.identity.quote_id.clone(),
        effect_key: "place".into(),
        stock_group_id: snapshot.identity.stock_group_id.clone(),
        created_at: snapshot.identity.created_at,
        hold_expires_at: snapshot.identity.hold_expires_at,
        intent: Some(serde_json::to_value(intent).expect("intent")),
        snapshot: Some(serde_json::to_value(&snapshot).expect("snapshot")),
        original_quote: Some(serde_json::to_value(&snapshot.quote).expect("quote")),
        original_market: Some(serde_json::to_value(&snapshot.market).expect("market")),
    }
}

#[test]
fn stored_order_header_and_binding_must_match_snapshot() {
    assert!(stored_row().decode().is_ok(), "valid independent fixture");
    for mode in ["basket", "quote", "revision"] {
        let mut row = stored_row();
        match mode {
            "basket" => row.basket_id = "9".repeat(32),
            "quote" => row.quote_id = "9".repeat(32),
            _ => row.basket_revision = 2,
        }
        assert!(
            matches!(row.decode(), Err(OrderStoreError::InvalidStoredData)),
            "mismatched {mode} must fail closed"
        );
    }
}

#[test]
fn promoted_order_decode_rejects_unbounded_or_malformed_line_arrays() {
    for lines in [
        serde_json::json!(null),
        serde_json::json!([null]),
        serde_json::json!([{"amounts": []}]),
        serde_json::Value::Array(vec![serde_json::json!({"amounts": {}}); 21]),
    ] {
        let mut row = stored_row();
        let value = row.snapshot.as_mut().expect("snapshot");
        value["schema_version"] = 2.into();
        value["quote"] = serde_json::json!({"original": {}, "promotion": {}, "shipping": {}, "total": {}, "lines": lines});
        assert!(
            matches!(row.decode(), Err(OrderStoreError::InvalidStoredData)),
            "unsafe line shape must fail closed"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn verified_order_rejects_a_different_owned_quote_read_in_the_same_transaction(
    pool: sqlx::PgPool,
) {
    use commerce_domain::{
        Currency,
        checkout_quotes::PromotedQuoteSnapshot,
        promotion_policies::{PromotionPolicy, RedemptionLimits},
        promotions::{DiscountKind, EligibleProduct, Promotion},
    };
    let (_, _, _, _, order) = crate::payment_test_fixture::accepted(&pool).await;
    let mut tx = pool.begin().await.expect("reader transaction");
    async fn load(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: &str) -> Row {
        sqlx::query_as::<_, Row>("SELECT o.id,o.basket_id,o.basket_revision,o.quote_id,o.effect_key,o.stock_group_id,o.created_at,o.hold_expires_at,o.intent,o.snapshot,q.snapshot AS original_quote,p.policy AS original_market FROM checkout_orders o JOIN guest_quote_snapshots q ON q.id=o.quote_id JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.id=$1")
            .bind(id).fetch_one(&mut **tx).await.expect("actual order row")
    }
    // Exercise a real returned legacy row first, then simulate a stale first
    // read whose coherent promoted envelope differs from the next quote read.
    let row = load(&mut tx, &order.identity.id).await;
    assert!(verified(&mut tx, row, None).await.is_ok());
    let legacy = load(&mut tx, &order.identity.id).await;
    assert!(
        matches!(
            verified(&mut tx, legacy, Some(1)).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "account association cannot authorize a legacy snapshot"
    );
    assert!(
        matches!(
            by_effect(&mut tx, &order.identity.basket_id, "place", Some(1)).await,
            Err(OrderStoreError::NotFound)
        ),
        "another account cannot replay the guest effect"
    );
    assert!(
        matches!(
            by_quote(
                &mut tx,
                &order.identity.basket_id,
                &order.identity.quote_id,
                Some(1)
            )
            .await,
            Err(OrderStoreError::NotFound)
        ),
        "another account cannot recover the guest quote effect"
    );
    let mut stale = load(&mut tx, &order.identity.id).await;
    let policy = PromotionPolicy {
        currency: Currency::EUR,
        promotion: Promotion {
            id: "SYNTHETIC-OFFER".into(),
            revision: 1,
            valid_from: 1000,
            expires_at: 2000,
            countries: vec!["NL".into()],
            products: vec![EligibleProduct {
                sku: "SET-FILLED".into(),
                publication_revision: 1,
            }],
            minimum_eligible_gross_minor: 1,
            maximum_discount_minor: 500,
            discount: DiscountKind::Fixed { minor_units: 500 },
            evidence_ref: "synthetic".into(),
        },
        limits: RedemptionLimits {
            total_discount_minor: 10000,
            total_redemptions: 20,
            per_customer_redemptions: None,
        },
    };
    let promoted = PromotedQuoteSnapshot::capture(order.quote.clone(), &policy, 1)
        .expect("coherent synthetic envelope");
    let value = serde_json::to_value(promoted).expect("promoted JSON");
    stale.original_quote = Some(value.clone());
    let snapshot = stale.snapshot.as_mut().expect("order snapshot");
    snapshot["schema_version"] = 2.into();
    snapshot["quote"] = value;
    assert!(
        matches!(
            verified(&mut tx, stale, None).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "the second owned quote read cannot differ from the first"
    );
    tx.rollback().await.expect("no effects");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn verified_promoted_order_requires_the_exact_second_promoted_read(pool: sqlx::PgPool) {
    use crate::payment_test_fixture::{promotion as promotion_fixture, quote as quote_fixture};
    use crate::{
        checkout_orders::create_order, checkout_quotes::issue_promoted_quote,
        guest_baskets::GuestCredential,
    };
    use commerce_application::Locale;
    use sqlx::Executor;
    quote_fixture::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent promotion grants");
    promotion_fixture::draft(&pool, &promotion_fixture::policy(), "promotion-draft")
        .await
        .expect("draft");
    promotion_fixture::decide(&pool, "promotion-review", 1, 0, true)
        .await
        .expect("review");
    let (store, _) = quote_fixture::store(&pool, 1100);
    let (session, csrf) = quote_fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &quote_fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("approved quote")
    .snapshot;
    let order = create_order(
        &store,
        &key,
        &csrf,
        "place",
        &crate::payment_test_fixture::order::intent(&quote.original.binding.quote_id),
        &crate::payment_test_fixture::order::seller(),
    )
    .await
    .expect("promoted order")
    .snapshot;
    let mut tx = pool.begin().await.expect("reader transaction");
    async fn load(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: &str) -> Row {
        sqlx::query_as::<_, Row>("SELECT o.id,o.basket_id,o.basket_revision,o.quote_id,o.effect_key,o.stock_group_id,o.created_at,o.hold_expires_at,o.intent,o.snapshot,q.snapshot AS original_quote,p.policy AS original_market FROM checkout_orders o JOIN guest_quote_snapshots q ON q.id=o.quote_id JOIN market_policy_drafts p ON p.id=q.policy_id AND p.revision=q.policy_revision WHERE o.id=$1")
            .bind(id).fetch_one(&mut **tx).await.expect("stored order row")
    }
    let exact = load(&mut tx, &order.identity.id).await;
    assert!(verified(&mut tx, exact, None).await.is_ok());
    let mut stale = load(&mut tx, &order.identity.id).await;
    let mut alternate = serde_json::to_value(&quote).expect("coherent promoted envelope");
    alternate["promotion"]["review_revision"] = 2.into();
    stale.original_quote = Some(alternate.clone());
    stale.snapshot.as_mut().expect("order snapshot")["quote"] = alternate;
    assert!(
        matches!(
            verified(&mut tx, stale, None).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "second promoted quote must be the same complete envelope"
    );
    (&mut *tx).execute("ALTER TABLE promotion_redemptions DISABLE TRIGGER guard_promotion_redemption; ALTER TABLE promotion_redemptions DROP CONSTRAINT promotion_redemptions_redemption_number_check, DROP CONSTRAINT promotion_redemptions_lifetime_discount_minor_check; ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order").await.expect("isolated damaged history");
    for (number, total) in [
        (0_i64, 500_i64),
        (1_000_001, 500),
        (1, 499),
        (1, 9_007_199_254_740_992),
    ] {
        sqlx::query("UPDATE promotion_redemptions SET redemption_number=$1,lifetime_discount_minor=$2 WHERE order_id=$3")
            .bind(number).bind(total).bind(&order.identity.id).execute(&mut *tx).await.expect("corrupt receipt");
        let row = load(&mut tx, &order.identity.id).await;
        assert!(
            matches!(
                verified(&mut tx, row, None).await,
                Err(OrderStoreError::InvalidStoredData)
            ),
            "damaged receipt cannot authorize history"
        );
    }
    sqlx::query("UPDATE promotion_redemptions SET redemption_number=1,lifetime_discount_minor=500 WHERE order_id=$1").bind(&order.identity.id).execute(&mut *tx).await.expect("restore receipt");
    (&mut *tx).execute("ALTER TABLE promotion_redemptions DROP CONSTRAINT promotion_redemption_account_sequence").await.expect("isolate inconsistent account sequence");
    sqlx::query("UPDATE promotion_redemptions SET account_redemption_number=1 WHERE order_id=$1")
        .bind(&order.identity.id)
        .execute(&mut *tx)
        .await
        .expect("detached account sequence");
    let row = load(&mut tx, &order.identity.id).await;
    assert!(
        matches!(
            verified(&mut tx, row, None).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "guest debit with an account sequence is damaged"
    );
    sqlx::query(
        "UPDATE promotion_redemptions SET account_redemption_number=NULL WHERE order_id=$1",
    )
    .bind(&order.identity.id)
    .execute(&mut *tx)
    .await
    .expect("restore guest debit");
    let row = load(&mut tx, &order.identity.id).await;
    assert!(
        verified(&mut tx, row, None).await.is_ok(),
        "restored receipt remains readable"
    );
    sqlx::query("UPDATE checkout_orders SET promotion_redemption_transaction_id=NULL WHERE id=$1")
        .bind(&order.identity.id)
        .execute(&mut *tx)
        .await
        .expect("detach marker");
    let row = load(&mut tx, &order.identity.id).await;
    assert!(
        matches!(
            verified(&mut tx, row, None).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "missing receipt marker cannot authorize history"
    );
    tx.rollback().await.expect("no effects");
}

#[allow(dead_code)]
#[path = "admitted_proofs.rs"]
mod admitted_proofs;

#[sqlx::test(migrations = false)]
async fn populated_promoted_order_survives_schema_upgrade_and_rejects_partial_account_custody(
    pool: sqlx::PgPool,
) {
    use crate::payment_test_fixture::{
        order as order_fixture, promotion as promotion_fixture, quote,
    };
    use crate::{
        checkout_orders, checkout_quotes::issue_promoted_quote, guest_baskets::GuestCredential,
    };
    use commerce_application::Locale;
    use sqlx::Executor;

    MIGRATOR.run_to(52, &pool).await.expect("schema 52");
    quote::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent grants");
    promotion_fixture::draft(&pool, &promotion_fixture::policy(), "upgrade-draft")
        .await
        .expect("draft");
    promotion_fixture::decide(&pool, "upgrade-review", 1, 0, true)
        .await
        .expect("approval");
    let (store, time) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let quoted = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &quote::request("upgrade"),
        "COURTYARD",
    )
    .await
    .expect("schema 52 promoted quote")
    .snapshot;
    let intent = order_fixture::intent(&quoted.original.binding.quote_id);
    let order = checkout_orders::create_order(
        &store,
        &key,
        &csrf,
        "upgrade-order",
        &intent,
        &order_fixture::seller(),
    )
    .await
    .expect("schema 52 promoted order")
    .snapshot;
    let mut tx = pool.begin().await.expect("schema 52 reader");
    assert!(
        by_id_for(&mut tx, &order.identity.basket_id, &order.identity.id, None)
            .await
            .is_ok(),
        "legacy receipt is readable without account columns"
    );
    tx.rollback().await.expect("finish read");
    MIGRATOR.run_to(53, &pool).await.expect("schema 53");
    let proof =
        admitted_proofs::email_verification::issue(&pool, "upgrade@example.test", time.as_ref())
            .await
            .expect("verification proof");
    let id = admitted_proofs::verified_customer_accounts::establish(
        &pool,
        "upgrade@example.test",
        &proof,
        time.as_ref(),
    )
    .await
    .expect("verified account")
    .value();
    let mut tx = pool.begin().await.expect("isolated damaged import");
    (&mut *tx)
        .execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
        .await
        .expect("allow isolated source damage");
    sqlx::query("UPDATE guest_quote_snapshots SET promotion_account_id=$1 WHERE id=$2")
        .bind(id)
        .bind(&order.identity.quote_id)
        .execute(&mut *tx)
        .await
        .expect("simulate forbidden account association");
    assert!(
        matches!(
            by_id_for(
                &mut tx,
                &order.identity.basket_id,
                &order.identity.id,
                Some(id)
            )
            .await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "schema 53 has no account debit and must fail closed on an account-bound order"
    );
    tx.rollback()
        .await
        .expect("restore original quote and guard");
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade to current schema");
    assert!(
        checkout_orders::owned_order(&store, &key, &order.identity.id)
            .await
            .expect("upgraded guest history")
            == order
    );
    assert!(
        checkout_orders::create_order(
            &store,
            &key,
            &csrf,
            "upgrade-order",
            &intent,
            &order_fixture::seller()
        )
        .await
        .expect("upgraded replay")
        .replayed
    );
}
