//! Synthetic private quote ownership and promotion history; no payment authority.
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_quotes::*,
    guest_baskets::{GuestCredential, GuestError},
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_order_fixture.rs"]
mod order;
#[allow(dead_code)]
#[path = "support/promotion_policy_fixture.rs"]
mod promotion;

async fn seed(pool: &PgPool) {
    fixture::seed(pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent promotion grants");
    promotion::draft(pool, &promotion::policy(), "promotion-draft")
        .await
        .expect("synthetic draft");
    promotion::decide(pool, "promotion-review", 1, 0, true)
        .await
        .expect("synthetic independent approval");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn owned_promoted_quote_exact_money_replay_and_v1_boundaries(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest owner");
    let request = fixture::request("promo-one");
    let before = fixture::counts(&pool).await;
    let receipt = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("reviewed indicative quote");
    assert!(!receipt.replayed);
    let p = receipt.snapshot;
    assert_eq!(
        (
            p.schema_version,
            p.original.schema_version,
            p.promotion.id.as_str(),
            p.promotion.revision,
            p.promotion.review_revision
        ),
        (2, 1, "COURTYARD", 1, 1)
    );
    assert_eq!(
        (
            p.original.total.gross_minor,
            p.discount_minor,
            p.total.gross_minor,
            p.total.net_minor,
            p.total.vat_minor
        ),
        (38245, 500, 37745, 31194, 6551)
    );
    assert_eq!(
        (
            p.lines[0].discount_minor,
            p.lines[0].amounts.gross_minor,
            p.lines[0].amounts.net_minor,
            p.lines[0].amounts.vat_minor
        ),
        (500, 36535, 30194, 6341)
    );
    assert_eq!(
        (
            p.shipping.gross_minor,
            p.shipping.net_minor,
            p.shipping.vat_minor
        ),
        (1210, 1000, 210)
    );
    let replay = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("historical retry");
    assert!(replay.replayed && replay.snapshot == p);
    assert!(
        owned_promoted_quote(&store, &key, &p.original.binding.quote_id)
            .await
            .expect("owned promoted read")
            == p
    );
    assert!(matches!(
        owned_quote(&store, &key, &p.original.binding.quote_id).await,
        Err(QuoteStoreError::NotFound)
    ));
    assert!(matches!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request).await,
        Err(QuoteStoreError::Conflict)
    ));
    assert!(matches!(
        issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "OTHER").await,
        Err(QuoteStoreError::Conflict)
    ));
    let after = fixture::counts(&pool).await;
    assert_eq!(
        (after.0, after.1, after.2),
        (before.0 + 1, before.1, before.2)
    );
    let effects:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT issued::bigint FROM guest_quote_quota LIMIT 1)").fetch_one(&pool).await.expect("no order or event and one quota");
    assert_eq!(effects, (0, 0, 1));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_replay_survives_withdrawal_but_new_quotes_fail(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let request = fixture::request("historical");
    let original = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("approved quote")
        .snapshot;
    promotion::decide(&pool, "withdraw", 1, 1, false)
        .await
        .expect("withdraw current approval");
    let replay = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("historical receipt");
    assert!(replay.replayed && replay.snapshot == original);
    assert!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("new"),
            "COURTYARD"
        )
        .await
        .is_err()
    );
    time.0.store(1400, Ordering::SeqCst);
    assert!(matches!(
        owned_promoted_quote(&store, &key, &original.original.binding.quote_id).await,
        Err(QuoteStoreError::Expired)
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quota_csrf_ownership_and_cross_version_effects_are_shared(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (a, csrf) = fixture::guest(&store).await;
    let (b, other_csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&a).expect("owner");
    let other = GuestCredential::from_session(&b).expect("other");
    assert!(matches!(
        issue_promoted_quote(
            &store,
            &key,
            &other_csrf,
            Locale::Nl,
            &fixture::request("bad-csrf"),
            "COURTYARD"
        )
        .await,
        Err(QuoteStoreError::Guest(GuestError::Csrf))
    ));
    let legacy = issue_quote(&store, &key, &csrf, Locale::Nl, &fixture::request("legacy"))
        .await
        .expect("legacy")
        .snapshot;
    assert!(matches!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("legacy"),
            "COURTYARD"
        )
        .await,
        Err(QuoteStoreError::Conflict)
    ));
    assert!(matches!(
        owned_promoted_quote(&store, &key, &legacy.binding.quote_id).await,
        Err(QuoteStoreError::NotFound)
    ));
    let p = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("promoted"),
        "COURTYARD",
    )
    .await
    .expect("promoted")
    .snapshot;
    assert!(matches!(
        owned_promoted_quote(&store, &other, &p.original.binding.quote_id).await,
        Err(QuoteStoreError::NotFound)
    ));
    for n in 0..8 {
        issue_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request(&format!("extra-{n}")),
        )
        .await
        .expect("shared quota");
    }
    assert!(matches!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("over"),
            "COURTYARD"
        )
        .await,
        Err(QuoteStoreError::RateLimited)
    ));
    assert!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("promoted"),
            "COURTYARD"
        )
        .await
        .expect("replay without quota")
        .replayed
    );
}

#[path = "support/promoted_checkout_intent.rs"]
mod promoted_checkout_intent;
#[path = "support/promoted_checkout_native.rs"]
mod promoted_checkout_native;
#[path = "support/promoted_checkout_shapes.rs"]
mod promoted_checkout_shapes;

#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_read_fails_closed_for_corrupt_allocation_and_review(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let request = fixture::request("damaged");
    let receipt = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("valid source and quote");
    let id = &receipt.snapshot.original.binding.quote_id;
    let original: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM guest_quote_snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("original snapshot");
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
        .await
        .expect("simulate isolated damaged snapshot");
    sqlx::query("UPDATE guest_quote_snapshots SET snapshot=jsonb_set(snapshot,'{total,gross_minor}','1') WHERE id=$1").bind(id).execute(&pool).await.expect("damage quote total");
    pool.execute("ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote")
        .await
        .expect("restore quote guard");
    assert!(matches!(
        owned_promoted_quote(&store, &key, id).await,
        Err(QuoteStoreError::InvalidStoredData)
    ));
    assert!(
        matches!(
            issue_quote(&store, &key, &csrf, Locale::Nl, &request).await,
            Err(QuoteStoreError::InvalidStoredData)
        ),
        "opposite-version effect validates before conflict"
    );
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
        .await
        .expect("restore isolated fixture");
    sqlx::query("UPDATE guest_quote_snapshots SET snapshot=$2 WHERE id=$1")
        .bind(id)
        .bind(original)
        .execute(&pool)
        .await
        .expect("restore exact quote");
    pool.execute("ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote; ALTER TABLE promotion_policy_reviews DISABLE TRIGGER guard_promotion_review; UPDATE promotion_policy_reviews SET actor='author' WHERE id='COURTYARD' AND draft_revision=1 AND revision=1; ALTER TABLE promotion_policy_reviews ENABLE TRIGGER guard_promotion_review").await.expect("simulate damaged historical review");
    assert!(matches!(
        owned_promoted_quote(&store, &key, id).await,
        Err(QuoteStoreError::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE promotion_policy_reviews DISABLE TRIGGER guard_promotion_review; UPDATE promotion_policy_reviews SET actor='reviewer' WHERE id='COURTYARD' AND draft_revision=1 AND revision=1; ALTER TABLE promotion_policy_reviews ENABLE TRIGGER guard_promotion_review").await.expect("restore historical approval");
    assert!(owned_promoted_quote(&store, &key, id).await.is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_restriction_publication_mismatch_and_unavailable_components_roll_back(pool: PgPool) {
    fixture::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent promotion grants");
    let mut policy = promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    promotion::draft(&pool, &policy, "limited-draft")
        .await
        .expect("valid customer-limited policy");
    promotion::decide(&pool, "limited-review", 1, 0, true)
        .await
        .expect("independent review");
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let baseline: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s",
    )
    .fetch_one(&pool)
    .await
    .expect("initial physical stock");
    assert!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("limited"),
            "COURTYARD"
        )
        .await
        .is_err(),
        "guest identity cannot satisfy customer limit"
    );
    policy.promotion.revision = 2;
    policy.limits.per_customer_redemptions = None;
    policy.promotion.products[0].publication_revision = 2;
    promotion::draft(&pool, &policy, "mismatch-draft")
        .await
        .expect("new eligible revision");
    promotion::decide(&pool, "mismatch-review", 2, 0, true)
        .await
        .expect("approved mismatch");
    assert!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("mismatched-publication"),
            "COURTYARD"
        )
        .await
        .is_err(),
        "exact published revision must match"
    );
    policy.promotion.revision = 3;
    policy.promotion.products[0].publication_revision = 1;
    promotion::draft(&pool, &policy, "stock-draft")
        .await
        .expect("eligible revision");
    promotion::decide(&pool, "stock-review", 3, 0, true)
        .await
        .expect("approved source");
    pool.execute("UPDATE stock_balances SET on_hand=0 WHERE component_code='SEAT-COVER'")
        .await
        .expect("isolated unavailable component");
    let unavailable: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s",
    )
    .fetch_one(&pool)
    .await
    .expect("unavailable stock");
    assert!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request("insufficient-stock"),
            "COURTYARD"
        )
        .await
        .is_err(),
        "shared component shortage blocks issuance"
    );
    let unchanged: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s",
    )
    .fetch_one(&pool)
    .await
    .expect("stock unaffected by failure");
    assert_eq!(unchanged, unavailable);
    assert_eq!(fixture::counts(&pool).await, (0, 0, 0));
    let quota: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_quote_quota")
        .fetch_one(&pool)
        .await
        .expect("failed quota effects rolled back");
    assert_eq!(quota, 0);
    pool.execute("UPDATE stock_balances SET on_hand=100 WHERE component_code='SEAT-COVER'")
        .await
        .expect("restore component");
    let restored: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s",
    )
    .fetch_one(&pool)
    .await
    .expect("physical stock restored");
    assert_eq!(restored, baseline);
    issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("healthy"),
        "COURTYARD",
    )
    .await
    .expect("healthy reviewed policy and physical stock");
    assert_eq!(fixture::counts(&pool).await, (1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn imported_v2_cannot_be_relabelled_as_legacy_by_replacing_the_envelope(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let legacy_request = fixture::request("valid-v1");
    let legacy = issue_quote(&store, &key, &csrf, Locale::Nl, &legacy_request)
        .await
        .expect("genuine v1 control")
        .snapshot;
    let promoted_request = fixture::request("corrupt-import");
    let promoted = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &promoted_request,
        "COURTYARD",
    )
    .await
    .expect("genuine v2")
    .snapshot;
    let id = &promoted.original.binding.quote_id;
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote")
        .await
        .expect("isolated import corruption");
    sqlx::query("UPDATE guest_quote_snapshots SET snapshot=snapshot->'original' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("replace envelope but retain promotion metadata");
    pool.execute("ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote")
        .await
        .expect("restore native guard");
    assert!(
        matches!(
            owned_quote(&store, &key, id).await,
            Err(QuoteStoreError::InvalidStoredData)
        ),
        "legacy read cannot adopt promoted row"
    );
    assert!(
        matches!(
            issue_quote(&store, &key, &csrf, Locale::Nl, &promoted_request).await,
            Err(QuoteStoreError::InvalidStoredData)
        ),
        "legacy effect replay cannot adopt promoted row"
    );
    assert!(matches!(
        owned_promoted_quote(&store, &key, id).await,
        Err(QuoteStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &promoted_request,
            "COURTYARD"
        )
        .await,
        Err(QuoteStoreError::InvalidStoredData)
    ));
    assert!(
        owned_quote(&store, &key, &legacy.binding.quote_id)
            .await
            .expect("intact legacy read")
            == legacy
    );
    assert!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &legacy_request)
            .await
            .expect("intact legacy replay")
            .replayed
    );
}
