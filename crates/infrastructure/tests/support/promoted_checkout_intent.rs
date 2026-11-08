//! Exact retry intent and invalid private locator controls for promoted receipts.
use super::*;

async fn effects(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots), (SELECT count(*) FROM stock_holds), (SELECT count(*) FROM stock_movements), (SELECT count(*) FROM checkout_orders), (SELECT count(*) FROM commerce_outbox), (SELECT COALESCE(sum(issued),0)::bigint FROM guest_quote_quota)")
        .fetch_one(pool).await.expect("quote, inventory, order, outbox and quota effects")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replay_requires_full_exact_intent_and_invalid_locators_leave_no_effects(pool: PgPool) {
    seed(&pool).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let request = fixture::request("exact-intent");
    let original = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("original owned quote")
        .snapshot;
    let before = effects(&pool).await;
    assert_eq!(before, (1, 0, 0, 0, 0, 1));

    // Every change remains syntactically valid, so the outcome tests receipt
    // binding rather than merely rejecting a malformed request.
    for (name, revision, locale, country, postcode, promotion_id) in [
        (
            "basket revision",
            2,
            Locale::Nl,
            "NL",
            "1234 AB",
            "COURTYARD",
        ),
        ("locale", 1, Locale::De, "NL", "1234 AB", "COURTYARD"),
        ("country", 1, Locale::Nl, "DE", "1234 AB", "COURTYARD"),
        ("postcode", 1, Locale::Nl, "NL", "5678 CD", "COURTYARD"),
        ("promotion", 1, Locale::Nl, "NL", "1234 AB", "OTHER"),
    ] {
        let mut changed = fixture::request("exact-intent");
        changed.expected_basket_revision = revision;
        changed.destination.country = country.into();
        changed.destination.postcode = postcode.into();
        assert!(
            matches!(
                issue_promoted_quote(&store, &key, &csrf, locale, &changed, promotion_id).await,
                Err(QuoteStoreError::Conflict)
            ),
            "changed {name} must conflict with original receipt"
        );
    }
    for invalid in ["", "DEMO", "invalid id"] {
        assert!(matches!(
            issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, invalid).await,
            Err(QuoteStoreError::InvalidRequest)
        ));
    }
    for invalid in ["", "wrong", "g0000000000000000000000000000000"] {
        assert!(matches!(
            owned_promoted_quote(&store, &key, invalid).await,
            Err(QuoteStoreError::InvalidRequest)
        ));
    }
    assert!(matches!(
        owned_promoted_quote(&store, &key, "00000000000000000000000000000000").await,
        Err(QuoteStoreError::NotFound)
    ));
    let mut stale = fixture::request("new-stale-basket");
    stale.expected_basket_revision = 2;
    assert!(matches!(
        issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &stale, "COURTYARD").await,
        Err(QuoteStoreError::Conflict)
    ));
    assert_eq!(
        effects(&pool).await,
        before,
        "failed requests roll back quote and quota effects"
    );

    let replay = issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
        .await
        .expect("exact replay still succeeds");
    assert!(replay.replayed && replay.snapshot == original);
    assert_eq!(
        effects(&pool).await,
        before,
        "replay must not duplicate effects"
    );
}
