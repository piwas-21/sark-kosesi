//! Native checks for the disposable Chromium favourites and restock journeys.
use crate::guarded_pool;
use sqlx::{Executor, PgPool};
use std::sync::atomic::AtomicU64;

#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}

async fn business_effects(pool: &PgPool) -> Vec<u8> {
    sqlx::query_scalar(
        "SELECT pg_catalog.sha256(pg_catalog.convert_to(pg_catalog.jsonb_build_object(
           'orders',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(o) ORDER BY id) FROM checkout_orders o),
           'outbox',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(e) ORDER BY id) FROM commerce_outbox e),
           'balances',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(s) ORDER BY component_code) FROM stock_balances s),
           'movements',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(m) ORDER BY sequence) FROM stock_movements m),
           'holds',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(h) ORDER BY id) FROM stock_holds h),
           'attempts',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(a) ORDER BY id) FROM payment_attempts a),
           'mail',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(m) ORDER BY verifier) FROM email_verification_delivery_starts m),
           'mail_outcomes',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(m) ORDER BY verifier) FROM email_verification_delivery_outcomes m),
           'link_mail',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(l) ORDER BY verifier) FROM guest_link_delivery_starts l),
           'link_outcomes',(SELECT pg_catalog.jsonb_agg(pg_catalog.to_jsonb(l) ORDER BY verifier) FROM guest_link_delivery_outcomes l)
        )::text,'UTF8'))",
    )
    .fetch_one(pool)
    .await
    .expect("fixture order, stock and delivery effects digest")
}

#[tokio::test]
#[ignore = "manual Chromium harness snapshots its own database before list operation effects"]
async fn snapshot_customer_list_browser_effects() {
    let pool = guarded_pool().await;
    pool.execute("CREATE TABLE guest_browser_customer_list_effects(digest bytea NOT NULL CHECK(octet_length(digest)=32))")
        .await.expect("isolated effects witness");
    let digest = business_effects(&pool).await;
    sqlx::query("INSERT INTO guest_browser_customer_list_effects VALUES($1)")
        .bind(digest)
        .execute(&pool)
        .await
        .expect("record initial effects digest");
    pool.close().await;
}

#[tokio::test]
#[ignore = "manual Chromium harness withdraws its synthetic published variant"]
async fn withdraw_customer_list_browser_publication() {
    let pool = guarded_pool().await;
    let membership: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM customer_favourites f JOIN verified_customer_accounts a ON a.id=f.account_id
                    WHERE a.email_identity='buyer@example.test' AND f.sku='SET-FILLED'),
                (SELECT count(*) FROM customer_restock_subscriptions r JOIN verified_customer_accounts a ON a.id=r.account_id
                    WHERE a.email_identity='buyer@example.test' AND r.sku='SET-FILLED' AND r.generation=1)",
    )
    .fetch_one(&pool)
    .await
    .expect("one confirmed preference of each kind before withdrawal");
    assert_eq!(membership, (1, 1));
    let mut request = catalogue::request(
        "browser-lists-withdraw",
        &catalogue::PublicationChoice::Withdraw {},
    );
    request.expected_revision = 1;
    catalogue::record_publication(
        &pool,
        &catalogue::author(),
        &crate::fixture::Time(AtomicU64::new(1100)),
        request,
    )
    .await
    .expect("withdraw this fixture's approved synthetic publication");
    pool.close().await;
}

#[tokio::test]
#[ignore = "manual Chromium harness checks final private list revisions and no unrelated effects"]
async fn verify_customer_list_browser_effects() {
    let pool = guarded_pool().await;
    let revisions: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT f.revision,r.revision FROM verified_customer_accounts a
         JOIN customer_favourite_buckets f ON f.account_id=a.id
         JOIN customer_restock_buckets r ON r.account_id=a.id
         ORDER BY a.email_identity",
    )
    .fetch_all(&pool)
    .await
    .expect("both independent owned buckets");
    assert_eq!(revisions, [(2, 2), (0, 0)]);
    let memberships: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM customer_favourites),(SELECT count(*) FROM customer_restock_subscriptions)",
    )
    .fetch_one(&pool)
    .await
    .expect("final memberships");
    assert_eq!(memberships, (0, 0));
    let initial: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT digest FROM guest_browser_customer_list_effects")
            .fetch_all(&pool)
            .await
            .expect("one initial witness");
    assert_eq!(initial, [business_effects(&pool).await]);
    pool.close().await;
}

#[tokio::test]
#[ignore = "manual Chromium harness checks exactly one owner revocation after re-login"]
async fn verify_customer_logout_browser_sessions() {
    let pool = guarded_pool().await;
    let custody: Vec<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT count(*),
                count(*) FILTER (WHERE s.revoked_at IS NOT NULL AND s.revocation_kind='logout'),
                count(*) FILTER (WHERE s.revoked_at IS NULL),
                count(DISTINCT c.csrf_digest)
         FROM customer_sessions s JOIN verified_customer_accounts a ON a.id=s.account_id
         LEFT JOIN customer_session_csrf c ON c.session_verifier=s.verifier
         GROUP BY a.email_identity ORDER BY a.email_identity",
    )
    .fetch_all(&pool)
    .await
    .expect("isolated session revocation and proof bindings");
    assert_eq!(custody, [(2, 1, 1, 2), (1, 0, 1, 1)]);
    pool.close().await;
}
