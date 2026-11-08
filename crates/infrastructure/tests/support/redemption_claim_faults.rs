//! Isolated negative controls for quote identity, policy revision and ledger custody.
use super::*;
use commerce_domain::{guest_baskets::BasketUpdate, quotes::BasketLine};

#[sqlx::test(migrator = "MIGRATOR")]
async fn newer_basket_revision_cannot_redeem_an_older_quote(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("old"),
        "COURTYARD",
    )
    .await
    .expect("old owned quote")
    .snapshot;
    write_basket(
        &store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "new-basket".into(),
            lines: vec![BasketLine {
                sku: "SET-FILLED".into(),
                quantity: 2,
            }],
        },
    )
    .await
    .expect("new basket revision");
    let mut intent = order::intent(&quote.original.binding.quote_id);
    intent.expected_basket_revision = 2;
    let before = effects(&pool).await;
    assert!(matches!(
        create_order(&store, &key, &csrf, "old-order", &intent, &order::seller()).await,
        Err(OrderStoreError::QuoteChanged)
    ));
    assert_eq!(effects(&pool).await, before);
    let mut request = fixture::request("new-quote");
    request.expected_basket_revision = 2;
    let current = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("current quote")
        .snapshot;
    intent.quote_id = current.original.binding.quote_id;
    create_order(&store, &key, &csrf, "new-order", &intent, &order::seller())
        .await
        .expect("matching basket positive control");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn newer_approval_of_same_draft_requires_a_fresh_quote(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("old"),
        "COURTYARD",
    )
    .await
    .expect("first review quote")
    .snapshot;
    promotion::decide(&pool, "new-independent-review", 1, 1, true)
        .await
        .expect("new approval revision");
    let before = effects(&pool).await;
    assert!(matches!(
        create_order(
            &store,
            &key,
            &csrf,
            "old-order",
            &order::intent(&quote.original.binding.quote_id),
            &order::seller()
        )
        .await,
        Err(OrderStoreError::QuoteChanged)
    ));
    assert_eq!(effects(&pool).await, before);
    let current = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("new"),
        "COURTYARD",
    )
    .await
    .expect("fresh review quote")
    .snapshot;
    assert_eq!(current.promotion.review_revision, 2);
    create_order(
        &store,
        &key,
        &csrf,
        "new-order",
        &order::intent(&current.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("current approval positive control");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_lifetime_head_fails_closed_before_new_order_effects(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (first, first_csrf) = fixture::guest(&store).await;
    let first_key = GuestCredential::from_session(&first).expect("first owner");
    let first_quote = issue_promoted_quote(
        &store,
        &first_key,
        &first_csrf,
        Locale::Nl,
        &fixture::request("first"),
        "COURTYARD",
    )
    .await
    .expect("first quote")
    .snapshot;
    create_order(
        &store,
        &first_key,
        &first_csrf,
        "first-order",
        &order::intent(&first_quote.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("first redemption");
    let (second, second_csrf) = fixture::guest(&store).await;
    let second_key = GuestCredential::from_session(&second).expect("second owner");
    let quote = issue_promoted_quote(
        &store,
        &second_key,
        &second_csrf,
        Locale::Nl,
        &fixture::request("second"),
        "COURTYARD",
    )
    .await
    .expect("second quote")
    .snapshot;
    let intent = order::intent(&quote.original.binding.quote_id);
    let baseline = effects(&pool).await;
    // The production constraints and immutable trigger normally prevent this.
    // Bypass only in this disposable database to exercise stored-data rejection.
    pool.execute("ALTER TABLE promotion_redemptions DROP CONSTRAINT promotion_redemptions_redemption_number_check; ALTER TABLE promotion_redemptions DROP CONSTRAINT promotion_redemptions_lifetime_discount_minor_check; ALTER TABLE promotion_redemptions DISABLE TRIGGER guard_promotion_redemption")
        .await.expect("isolated corruption instrument");
    for (number, spent) in [(0_i64, 500_i64), (1, 0)] {
        sqlx::query(
            "UPDATE promotion_redemptions SET redemption_number=$1,lifetime_discount_minor=$2",
        )
        .bind(number)
        .bind(spent)
        .execute(&pool)
        .await
        .expect("damage head");
        assert!(matches!(
            create_order(
                &store,
                &second_key,
                &second_csrf,
                "second-order",
                &intent,
                &order::seller()
            )
            .await,
            Err(OrderStoreError::InvalidStoredData)
        ));
        assert_eq!(
            effects(&pool).await,
            baseline,
            "no partial order or stock effects"
        );
    }
    pool.execute("UPDATE promotion_redemptions SET redemption_number=1,lifetime_discount_minor=500; ALTER TABLE promotion_redemptions ENABLE TRIGGER guard_promotion_redemption")
        .await.expect("restore ledger and trigger");
    create_order(
        &store,
        &second_key,
        &second_csrf,
        "second-order",
        &intent,
        &order::seller(),
    )
    .await
    .expect("valid head positive control");
    assert_eq!(effects(&pool).await.5, 2);
}
