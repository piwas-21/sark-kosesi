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
