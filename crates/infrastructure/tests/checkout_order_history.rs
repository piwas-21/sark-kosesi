//! Damaged-import and atomic persistence controls through the owning order adapter.
use commerce_application::Locale;
use commerce_domain::checkout_orders::{OrderIntent, OrderSnapshot};
use commerce_infrastructure::{MIGRATOR, checkout_orders::*, checkout_quotes::*, guest_baskets::*};
use sqlx::{Executor, PgPool};
#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quotes;
#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_bounded_history_rejects_damaged_imports_and_restores_exact_receipt(pool: PgPool) {
    quotes::seed(&pool).await;
    let (store, _) = quotes::store(&pool, 1100);
    let (s, c) = quotes::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &key, &c, Locale::Nl, &quotes::request("q"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    let snapshot = create_order(&store, &key, &c, "place", &intent, &fixture::seller())
        .await
        .expect("order")
        .snapshot;
    let original = serde_json::to_value(&snapshot).expect("snapshot");
    let original_intent = serde_json::to_value(&intent).expect("intent");
    pool.execute("ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order;ALTER TABLE checkout_orders DROP CONSTRAINT checkout_orders_snapshot_check,DROP CONSTRAINT checkout_orders_intent_check").await.expect("isolated damaged import fixture");
    let mut changes = vec![
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!({"oversized":"x".repeat(65537)}),
    ];
    for pointer in [
        "/identity",
        "/quote",
        "/quote/binding",
        "/quote/lines/0",
        "/quote/lines/0/amounts",
        "/quote/shipping",
        "/quote/total",
        "/buyer",
        "/shipping_address",
        "/billing_address",
        "/seller",
        "/seller/address",
        "/seller/tax_registration",
        "/market",
        "/market/destination",
        "/market/shipping",
        "/market/shipping/gross",
        "/market/products/0",
        "/products/0",
        "/products/0/footprint",
    ] {
        let mut v = original.clone();
        let target = v.pointer_mut(pointer).expect("promised object");
        assert!(target.is_object());
        *target = serde_json::json!([]);
        changes.push(v);
    }
    for (pointer, value) in [
        ("/schema_version", serde_json::json!(2)),
        ("/identity/id", serde_json::json!("9".repeat(32))),
        (
            "/identity/stock_group_id",
            serde_json::json!("9".repeat(32)),
        ),
        ("/identity/created_at", serde_json::json!(1101)),
        ("/identity/hold_expires_at", serde_json::json!(1999)),
        ("/quote/total/gross_minor", serde_json::json!(38244)),
        ("/buyer/email", serde_json::json!("changed@example.test")),
        (
            "/shipping_address/recipient",
            serde_json::json!("Different Recipient"),
        ),
        (
            "/billing_address/recipient",
            serde_json::json!("Different Recipient"),
        ),
        (
            "/seller/terms_version",
            serde_json::json!("different-terms"),
        ),
        (
            "/market/evidence_ref",
            serde_json::json!("different source"),
        ),
    ] {
        let mut v = original.clone();
        *v.pointer_mut(pointer).expect("field") = value;
        changes.push(v);
    }
    let mut unknown = original.clone();
    unknown["role"] = "admin".into();
    changes.push(unknown);
    for value in changes {
        sqlx::query("UPDATE checkout_orders SET snapshot=$1")
            .bind(value)
            .execute(&pool)
            .await
            .expect("damaged row");
        assert!(matches!(
            owned_order(&store, &key, &snapshot.identity.id).await,
            Err(OrderStoreError::InvalidStoredData)
        ));
    }
    sqlx::query("UPDATE checkout_orders SET snapshot=$1")
        .bind(&original)
        .execute(&pool)
        .await
        .expect("restore");
    let mut changes = vec![
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!({"oversized":"x".repeat(16385)}),
    ];
    for pointer in ["/buyer", "/shipping_address", "/billing_address"] {
        let mut v = original_intent.clone();
        *v.pointer_mut(pointer).expect("object") = serde_json::json!([]);
        changes.push(v);
    }
    for (pointer, value) in [
        ("/quote_id", serde_json::json!("9".repeat(32))),
        ("/expected_basket_revision", serde_json::json!(2)),
        ("/confirmed", serde_json::json!(false)),
        (
            "/accepted_terms_version",
            serde_json::json!("different-terms"),
        ),
        ("/buyer/email", serde_json::json!("changed@example.test")),
    ] {
        let mut v = original_intent.clone();
        *v.pointer_mut(pointer).expect("field") = value;
        changes.push(v);
    }
    for value in changes {
        sqlx::query("UPDATE checkout_orders SET intent=$1")
            .bind(value)
            .execute(&pool)
            .await
            .expect("damaged intent");
        assert!(matches!(
            create_order(&store, &key, &c, "place", &intent, &fixture::seller()).await,
            Err(OrderStoreError::InvalidStoredData)
        ));
    }
    sqlx::query("UPDATE checkout_orders SET intent=$1")
        .bind(original_intent)
        .execute(&pool)
        .await
        .expect("restore");
    assert!(
        owned_order(&store, &key, &snapshot.identity.id)
            .await
            .expect("positive restored source")
            == snapshot
    );
    // Serde alone permits positional structs; the storage contract requires objects.
    let mut positional = original.clone();
    positional["buyer"] = serde_json::json!(["Synthetic Buyer", "buyer@example.test"]);
    assert!(serde_json::from_value::<OrderSnapshot>(positional).is_ok());
    let mut positional = serde_json::to_value(intent).expect("intent");
    positional["buyer"] = serde_json::json!(["Synthetic Buyer", "buyer@example.test"]);
    assert!(serde_json::from_value::<OrderIntent>(positional).is_ok());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn event_write_failure_rolls_back_order_holds_and_all_movements_without_private_error_detail(
    pool: PgPool,
) {
    quotes::seed(&pool).await;
    let (store, _) = quotes::store(&pool, 1100);
    let (s, c) = quotes::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_quote(&store, &key, &c, Locale::Nl, &quotes::request("q"))
        .await
        .expect("quote")
        .snapshot;
    let intent = fixture::intent(&q.binding.quote_id);
    pool.execute("CREATE FUNCTION reject_order_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic buyer@example.test private database detail'; END; $$; CREATE TRIGGER reject_order_event BEFORE INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION reject_order_event()").await.expect("failure instrument");
    let result = create_order(&store, &key, &c, "place", &intent, &fixture::seller()).await;
    let error = result.err().expect("write fails");
    assert_eq!(error, OrderStoreError::Database);
    assert_eq!(format!("{error:?}"), "Database");
    let counts:(i64,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)").fetch_one(&pool).await.expect("rollback");
    assert_eq!(counts, (0, 0, 0, 0, 0));
    pool.execute("DROP TRIGGER reject_order_event ON commerce_outbox")
        .await
        .expect("restore");
    assert!(
        create_order(&store, &key, &c, "place", &intent, &fixture::seller())
            .await
            .is_ok()
    );
}
