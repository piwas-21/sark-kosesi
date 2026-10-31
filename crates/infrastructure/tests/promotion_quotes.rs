#[allow(dead_code, unused_imports)]
mod fixture {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use commerce_domain::{
    Money,
    promotions::{DiscountKind, EligibleProduct, Promotion, PromotionError},
    quotes::*,
};
use commerce_infrastructure::{market_policies as markets, quotes::QuoteFailure};
use fixture::*;
fn basket() -> QuoteRequest {
    QuoteRequest {
        destination: Destination {
            country: "NL".into(),
            postcode: "1234 AB".into(),
        },
        lines: vec![BasketLine {
            sku: "SET-FILLED".into(),
            quantity: 3,
        }],
    }
}
fn policy() -> MarketPolicy {
    MarketPolicy {
        id: "SYNTHETIC-NL".into(),
        revision: 1,
        destination: basket().destination,
        valid_from: 1000,
        expires_at: 3000,
        products: vec![ProductRule {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
            max_quantity: 10,
            vat_basis_points: 2100,
        }],
        shipping: ShippingRule {
            gross: Money::eur(1210).expect("synthetic"),
            vat_basis_points: 2100,
            max_parcels: 10,
            max_total_weight_g: 30000,
            max_edge_mm: 505,
        },
        evidence_ref: "synthetic market evidence".into(),
    }
}
fn promotion() -> Promotion {
    Promotion {
        id: "SYNTHETIC-OFFER".into(),
        revision: 1,
        valid_from: 1000,
        expires_at: 3000,
        countries: vec!["NL".into()],
        products: vec![EligibleProduct {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
        }],
        minimum_eligible_gross_minor: 0,
        maximum_discount_minor: 1000,
        discount: DiscountKind::Fixed { minor_units: 100 },
        evidence_ref: "synthetic economics; not merchant approval".into(),
    }
}
fn audit(key: &str, expected: i32) -> markets::Audit<'_> {
    markets::Audit {
        expected_revision: expected,
        effect_key: key,
        reason: "synthetic",
        confirmed: true,
    }
}
async fn activate(pool: &PgPool) {
    approve(pool).await;
    record_publication(
        pool,
        &author(),
        &FixedClock(1100),
        PublicationRequest {
            sku: "SET-FILLED",
            locale: Locale::Nl,
            expected_revision: 0,
            effect_key: "activate",
            choice: &choice(),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("publish");
    pool.execute("INSERT INTO stock_balances(component_code,on_hand,quarantine,reserved) VALUES('SEAT-COVER',100,0,0),('SEAT-FOAM',100,0,0); INSERT INTO staff_capabilities VALUES('author','market.manage'),('reviewer','market.review')").await.expect("synthetic stock and grants");
}
async fn ready_market_policy(pool: &PgPool, selected: &MarketPolicy) {
    markets::record_draft(
        pool,
        &author(),
        &FixedClock(1100),
        markets::DraftRequest {
            policy: selected,
            audit: audit("draft", 0),
        },
    )
    .await
    .expect("market draft");
    decision(pool, "approve", 0, true).await;
}
async fn ready_market(pool: &PgPool) {
    ready_market_policy(pool, &policy()).await;
}
async fn decision(pool: &PgPool, key: &str, expected: i32, approved: bool) {
    markets::record_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        markets::ReviewRequest {
            id: "SYNTHETIC-NL",
            draft_revision: 1,
            decision: if approved {
                FactDecision::Approved
            } else {
                FactDecision::Rejected
            },
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            audit: audit(key, expected),
        },
    )
    .await
    .expect("independent decision");
}
async fn quote(
    pool: &PgPool,
    p: &Promotion,
) -> Result<commerce_infrastructure::quotes::PromotedServerQuote, markets::MarketError> {
    markets::reviewed_promotion_quote(pool, &FixedClock(1100), Locale::Nl, &basket(), p).await
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn reviewed_promoted_quote_has_exact_total_snapshots_and_no_stock_or_refund_effect(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    ready_market(&pool).await;
    let r = quote(&pool, &promotion())
        .await
        .expect("promoted server quote");
    assert_eq!(
        (r.policy_id.as_str(), r.policy_revision, r.evaluated_at),
        ("SYNTHETIC-NL", 1, 1100)
    );
    assert_eq!(
        (
            r.totals.allocation_version,
            r.totals.discount.minor_units(),
            r.totals.total.gross.minor_units(),
            r.totals.total.net.minor_units(),
            r.totals.total.vat.minor_units()
        ),
        (1, 100, 38145, 31525, 6620)
    );
    assert_eq!(
        (
            r.totals.lines[0].original.quantity,
            r.totals.lines[0].original.unit_gross.minor_units(),
            r.totals.lines[0].original.amounts.gross.minor_units(),
            r.totals.lines[0].amounts.gross.minor_units(),
            r.totals.shipping.gross.minor_units()
        ),
        (3, 12345, 37035, 36935, 1210)
    );
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements),(SELECT sum(reserved)::bigint FROM stock_balances)").fetch_one(&pool).await.expect("no writes");
    assert_eq!(counts, (0, 0, 0));
    assert_eq!(r, quote(&pool, &promotion()).await.expect("repeat"));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_cannot_grant_market_readiness_and_withdrawal_remains_authoritative(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    assert!(matches!(
        quote(&pool, &promotion()).await,
        Err(markets::MarketError::NotFound)
    ));
    ready_market(&pool).await;
    assert!(quote(&pool, &promotion()).await.is_ok());
    decision(&pool, "withdraw", 1, false).await;
    assert!(matches!(
        quote(&pool, &promotion()).await,
        Err(markets::MarketError::NotFound)
    ));
    decision(&pool, "approve", 0, true).await;
    assert!(matches!(
        quote(&pool, &promotion()).await,
        Err(markets::MarketError::NotFound)
    ));
    let mut invalid = basket();
    invalid.lines.clear();
    assert!(matches!(
        markets::reviewed_promotion_quote(
            &pool,
            &FixedClock(1100),
            Locale::Nl,
            &invalid,
            &promotion()
        )
        .await,
        Err(markets::MarketError::Quote(QuoteFailure::Policy(
            QuoteError::InvalidRequest
        )))
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_malformed_and_ineligible_promotions_fail_without_client_coupon_authority(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    ready_market(&pool).await;
    for case in 0..6 {
        let mut p = promotion();
        match case {
            0 => p.id = "".into(),
            1 => p.products[0].publication_revision = 2,
            2 => p.countries = vec!["DE".into()],
            3 => p.valid_from = 1101,
            4 => p.expires_at = 1100,
            _ => p.minimum_eligible_gross_minor = 37036,
        }
        let expected = if case == 0 {
            PromotionError::InvalidPolicy
        } else {
            PromotionError::Ineligible
        };
        assert!(
            matches!(quote(&pool,&p).await,Err(markets::MarketError::Quote(QuoteFailure::Promotion(e)))if e==expected)
        );
    }
    let raw = r#"{"lines":[{"sku":"SET-FILLED","quantity":3}],"destination":{"country":"NL","postcode":"1234 AB"},"coupon":"SYNTHETIC-OFFER"}"#;
    assert!(serde_json::from_str::<QuoteRequest>(raw).is_err());
    pool.execute("UPDATE stock_balances SET reserved=100 WHERE component_code='SEAT-COVER'")
        .await
        .expect("other basket");
    assert!(matches!(
        quote(&pool, &promotion()).await,
        Err(markets::MarketError::Quote(QuoteFailure::Policy(
            QuoteError::InsufficientStock
        )))
    ));
}
use std::sync::{Arc, atomic::AtomicU64};
struct MutableClock(AtomicU64);
impl Clock for MutableClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
async fn waiting(pool: &PgPool) {
    for _ in 0..150 {
        let yes:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT component_code,on_hand,%')").fetch_one(pool).await.expect("observer");
        if yes {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }
    panic!("quote never waited for stock");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_expiry_after_actual_stock_wait_is_checked_at_completion(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    ready_market(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT component_code FROM stock_balances FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("claim");
    let clock = Arc::new(MutableClock(AtomicU64::new(1100)));
    let c = clock.clone();
    let db = pool.clone();
    let mut p = promotion();
    p.expires_at = 1200;
    let task = tokio::spawn(async move {
        markets::reviewed_promotion_quote(&db, c.as_ref(), Locale::Nl, &basket(), &p).await
    });
    waiting(&pool).await;
    clock.0.store(1200, Ordering::SeqCst);
    blocker.rollback().await.expect("release");
    assert!(matches!(
        task.await.expect("task"),
        Err(markets::MarketError::Quote(QuoteFailure::Promotion(
            PromotionError::Ineligible
        )))
    ));
    assert!(quote(&pool, &promotion()).await.is_ok());
}

#[test]
fn promotion_terms_are_closed_server_data_and_never_unchecked_json() {
    let value = serde_json::to_value(promotion()).expect("trusted terms");
    let decoded: Promotion = serde_json::from_value(value.clone()).expect("closed terms");
    assert_eq!(decoded, promotion());
    assert!(decoded.validate().is_ok());
    for target in ["root", "discount", "product"] {
        let mut v = value.clone();
        match target {
            "root" => v["budget_authorized"] = serde_json::json!(true),
            "discount" => v["discount"]["override_total"] = serde_json::json!(0),
            _ => v["products"][0]["client_price"] = serde_json::json!(1),
        }
        assert!(serde_json::from_value::<Promotion>(v).is_err());
    }
    for bad in [serde_json::json!(-0.5), serde_json::json!("100")] {
        let mut v = value.clone();
        v["discount"]["minor_units"] = bad;
        assert!(serde_json::from_value::<Promotion>(v).is_err());
    }
    let mut v = value;
    v["discount"]["kind"] = serde_json::json!("unknown");
    assert!(serde_json::from_value::<Promotion>(v).is_err());
}
async fn wait_for_expiry(
    pool: &PgPool,
    market_expiry: i64,
    promotion_expiry: i64,
    after: u64,
) -> Result<commerce_infrastructure::quotes::PromotedServerQuote, markets::MarketError> {
    let _case = media_seed(pool).await;
    activate(pool).await;
    let mut market = policy();
    market.expires_at = market_expiry;
    ready_market_policy(pool, &market).await;
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT component_code FROM stock_balances FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("claim");
    let clock = Arc::new(MutableClock(AtomicU64::new(1100)));
    let c = clock.clone();
    let db = pool.clone();
    let mut p = promotion();
    p.expires_at = promotion_expiry;
    let task = tokio::spawn(async move {
        markets::reviewed_promotion_quote(&db, c.as_ref(), Locale::Nl, &basket(), &p).await
    });
    waiting(pool).await;
    clock.0.store(after, Ordering::SeqCst);
    blocker.rollback().await.expect("release");
    task.await.expect("quote task")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_never_extends_market_policy_after_a_stock_wait(pool: PgPool) {
    assert!(matches!(
        wait_for_expiry(&pool, 1200, 3000, 1200).await,
        Err(markets::MarketError::Quote(QuoteFailure::Policy(
            QuoteError::MarketUnavailable
        )))
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_never_keeps_expired_photography_eligible(pool: PgPool) {
    assert!(matches!(
        wait_for_expiry(&pool, 3000, 3000, 2000).await,
        Err(markets::MarketError::Quote(QuoteFailure::Policy(
            QuoteError::ProductUnavailable
        )))
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelled_promoted_quote_releases_policy_and_component_claims(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    ready_market(&pool).await;
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT component_code FROM stock_balances FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("claim");
    let db = pool.clone();
    let task = tokio::spawn(async move { quote(&db, &promotion()).await });
    waiting(&pool).await;
    task.abort();
    assert!(task.await.expect_err("cancelled").is_cancelled());
    blocker.rollback().await.expect("release");
    assert!(quote(&pool, &promotion()).await.is_ok());
    decision(&pool, "after-cancel-withdraw", 1, false).await;
    assert!(matches!(
        quote(&pool, &promotion()).await,
        Err(markets::MarketError::NotFound)
    ));
}
