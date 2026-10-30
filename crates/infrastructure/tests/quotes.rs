#[allow(dead_code, unused_imports)]
mod fixture {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use commerce_domain::{Money, quotes::*};
use commerce_infrastructure::quotes::*;
use fixture::*;
include!("support/published_candidate.rs");
fn basket(quantity: u32) -> QuoteRequest {
    QuoteRequest {
        destination: Destination {
            country: "NL".into(),
            postcode: "1234 AB".into(),
        },
        lines: vec![BasketLine {
            sku: "SET-FILLED".into(),
            quantity,
        }],
    }
}
fn policy() -> MarketPolicy {
    MarketPolicy {
        id: "SYNTHETIC-NL".into(),
        revision: 1,
        destination: basket(1).destination,
        valid_from: 1000,
        expires_at: 3000,
        products: vec![ProductRule {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
            max_quantity: 10,
            vat_basis_points: 2100,
        }],
        shipping: ShippingRule {
            gross: Money::eur(1210).expect("fixture"),
            vat_basis_points: 2100,
            max_parcels: 10,
            max_total_weight_g: 30000,
            max_edge_mm: 505,
        },
        evidence_ref: "isolated synthetic policy; not carrier or tax approval".into(),
    }
}
async fn activate(pool: &PgPool) {
    approve(pool).await;
    record_publication(
        pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice()),
    )
    .await
    .expect("activate");
    stock(pool, 100, 100, 0, 0).await;
}
async fn stock(pool: &PgPool, cover: i64, foam: i64, quarantine: i64, reserved: i64) {
    sqlx::query("INSERT INTO stock_balances(component_code,on_hand,quarantine,reserved) VALUES('SEAT-COVER',$1,$3,$4),('SEAT-FOAM',$2,0,0) ON CONFLICT(component_code) DO UPDATE SET on_hand=EXCLUDED.on_hand,quarantine=EXCLUDED.quarantine,reserved=EXCLUDED.reserved").bind(cover).bind(foam).bind(quarantine).bind(reserved).execute(pool).await.expect("isolated stock snapshot");
}
async fn quote(
    pool: &PgPool,
    r: &QuoteRequest,
    p: &[MarketPolicy],
) -> Result<ServerQuote, QuoteFailure> {
    server_quote(pool, &FixedClock(1100), Locale::Nl, r, p).await
}
fn rejected(result: Result<ServerQuote, QuoteFailure>, error: QuoteError) {
    assert!(matches!(result,Err(QuoteFailure::Policy(e)) if e==error));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quote_uses_approved_server_price_and_leaves_stock_and_financial_state_untouched(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    let q = quote(&pool, &basket(3), &[policy()])
        .await
        .expect("server quote");
    assert_eq!(
        (&*q.policy_id, q.policy_revision, q.evaluated_at),
        ("SYNTHETIC-NL", 1, 1100)
    );
    assert_eq!(
        (
            q.totals.total.gross.minor_units(),
            q.totals.total.net.minor_units(),
            q.totals.total.vat.minor_units()
        ),
        (38245, 31607, 6638)
    );
    assert_eq!(
        (
            q.totals.lines[0].quantity,
            q.totals.lines[0].unit_gross.minor_units(),
            q.totals.lines[0].publication_revision
        ),
        (3, 12345, 1)
    );
    assert_eq!(
        q,
        quote(&pool, &basket(3), &[policy()]).await.expect("repeat")
    );
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements),(SELECT sum(reserved)::bigint FROM stock_balances)").fetch_one(&pool).await.expect("no writes");
    assert_eq!(counts, (0, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_request_contract_has_no_client_money_policy_discount_or_ownership_fields(
    pool: PgPool,
) {
    let valid = r#"{"lines":[{"sku":"SET-FILLED","quantity":1}],"destination":{"country":"NL","postcode":"1234 AB"}}"#;
    let r: QuoteRequest = serde_json::from_str(valid).expect("closed request");
    assert_eq!(r, basket(1));
    for extra in [
        "total",
        "price",
        "vat_basis_points",
        "shipping",
        "policy",
        "coupon",
        "owner_id",
    ] {
        let hostile = valid.replacen("{", &format!("{{\"{extra}\":0,"), 1);
        assert!(serde_json::from_str::<QuoteRequest>(&hostile).is_err());
    }
    for hostile in [
        valid.replace("\"quantity\":1", "\"quantity\":1,\"unit_gross\":1"),
        valid.replace("\"country\":\"NL\"", "\"country\":\"NL\",\"ready\":true"),
        valid.replace("\"quantity\":1", "\"quantity\":-1"),
        valid.replace("\"quantity\":1", "\"quantity\":1.5"),
    ] {
        assert!(serde_json::from_str::<QuoteRequest>(&hostile).is_err());
    }
    rejected(
        quote(&pool, &basket(0), &[]).await,
        QuoteError::InvalidRequest,
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn unconfigured_ambiguous_expired_and_wrong_destination_policies_fail_closed(pool: PgPool) {
    rejected(
        quote(&pool, &basket(1), &[]).await,
        QuoteError::MarketUnavailable,
    );
    rejected(
        quote(&pool, &basket(1), &vec![policy(); 65]).await,
        QuoteError::InvalidPolicy,
    );
    rejected(
        quote(&pool, &basket(1), &[policy(), policy()]).await,
        QuoteError::InvalidPolicy,
    );
    for (country, postcode, start, end) in [
        ("DE", "1234 AB", 1000, 3000),
        ("NL", "1234 AC", 1000, 3000),
        ("NL", "1234 AB", 1101, 3000),
        ("NL", "1234 AB", 1000, 1100),
    ] {
        let mut p = policy();
        p.destination.country = country.into();
        p.destination.postcode = postcode.into();
        p.valid_from = start;
        p.expires_at = end;
        rejected(
            quote(&pool, &basket(1), &[p]).await,
            QuoteError::MarketUnavailable,
        );
    }
    let mut p = policy();
    p.products.clear();
    rejected(
        quote(&pool, &basket(1), &[p]).await,
        QuoteError::InvalidPolicy,
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn unpublished_fixture_missing_locale_and_unapproved_source_changes_never_get_quoted(
    pool: PgPool,
) {
    let _case = media_seed(&pool).await;
    rejected(
        quote(&pool, &basket(1), &[policy()]).await,
        QuoteError::ProductUnavailable,
    );
    activate(&pool).await;
    rejected(
        server_quote(
            &pool,
            &FixedClock(1100),
            Locale::De,
            &basket(1),
            &[policy()],
        )
        .await,
        QuoteError::ProductUnavailable,
    );
    for sku in ["MISSING", "DEMO-COURTYARD-COVERS"] {
        let mut r = basket(1);
        r.lines[0].sku = sku.into();
        rejected(
            quote(&pool, &r, &[policy()]).await,
            if sku.starts_with("DEMO") {
                QuoteError::InvalidRequest
            } else {
                QuoteError::ProductUnavailable
            },
        );
    }
    let mut p = policy();
    p.products[0].publication_revision = 2;
    rejected(
        quote(&pool, &basket(1), &[p]).await,
        QuoteError::ProductUnavailable,
    );
    let mut p = policy();
    p.products[0].sku = "OTHER".into();
    rejected(
        quote(&pool, &basket(1), &[p]).await,
        QuoteError::ProductUnavailable,
    );
    rejected(
        quote(&pool, &basket(11), &[policy()]).await,
        QuoteError::ProductUnavailable,
    );
    price(&pool, 1, "new-price").await;
    rejected(
        quote(&pool, &basket(1), &[policy()]).await,
        QuoteError::ProductUnavailable,
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn withdrawal_rights_revocation_and_no_sibling_fallback_invalidate_quotes(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    let withdraw = PublicationChoice::Withdraw {};
    let mut r = request("withdraw", &withdraw);
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("withdraw");
    rejected(
        quote(&pool, &basket(1), &[policy()]).await,
        QuoteError::ProductUnavailable,
    );
    let activation = choice();
    let mut r = request("republish", &activation);
    r.expected_revision = 2;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("republish");
    rejected(
        quote(&pool, &basket(1), &[policy()]).await,
        QuoteError::ProductUnavailable,
    );
    let mut p = policy();
    p.products[0].publication_revision = 3;
    assert!(quote(&pool, &basket(1), &[p.clone()]).await.is_ok());
    rights(&pool, 1, &MediaRightsChange::Revoke).await;
    rejected(
        quote(&pool, &basket(1), &[p]).await,
        QuoteError::ProductUnavailable,
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quantities_and_shared_components_use_one_physical_stock_budget(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    for (cover, foam, quarantine, reserved) in [
        (5, 100, 0, 0),
        (100, 5, 0, 0),
        (8, 8, 3, 0),
        (8, 8, 1, 2),
        (0, 0, 0, 0),
    ] {
        stock(&pool, cover, foam, quarantine, reserved).await;
        rejected(
            quote(&pool, &basket(3), &[policy()]).await,
            QuoteError::InsufficientStock,
        );
    }
    stock(&pool, 8, 8, 1, 1).await;
    assert!(quote(&pool, &basket(3), &[policy()]).await.is_ok());
    candidate(&pool, "SECOND-SET").await;
    stock(&pool, 6, 6, 0, 0).await;
    let mut p = policy();
    p.products.push(ProductRule {
        sku: "SECOND-SET".into(),
        ..p.products[0].clone()
    });
    let mut r = basket(2);
    r.lines.push(BasketLine {
        sku: "SECOND-SET".into(),
        quantity: 2,
    });
    rejected(
        quote(&pool, &r, &[p.clone()]).await,
        QuoteError::InsufficientStock,
    );
    stock(&pool, 8, 8, 0, 0).await;
    let q = quote(&pool, &r, &[p.clone()]).await.expect("shared fits");
    assert_eq!(q.totals.total.gross.minor_units(), 50590);
    r.lines.reverse();
    assert_eq!(
        q,
        quote(&pool, &r, &[p]).await.expect("same canonical quote")
    );
    pool.execute("DELETE FROM stock_balances WHERE component_code='SEAT-FOAM'")
        .await
        .expect("unreceived");
    rejected(
        quote(&pool, &basket(1), &[policy()]).await,
        QuoteError::InsufficientStock,
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn measured_shipping_limits_apply_to_whole_basket_and_include_tolerances(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    for (count, weight, edge) in [(2, 30000, 505), (10, 8999, 505), (10, 30000, 504)] {
        let mut p = policy();
        p.shipping.max_parcels = count;
        p.shipping.max_total_weight_g = weight;
        p.shipping.max_edge_mm = edge;
        rejected(
            quote(&pool, &basket(3), &[p]).await,
            QuoteError::ShippingUnavailable,
        );
    }
    let mut p = policy();
    p.shipping.max_parcels = 3;
    p.shipping.max_total_weight_g = 9000;
    assert!(quote(&pool, &basket(3), &[p]).await.is_ok());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_clock_rechecks_policy_and_image_rights_and_rejects_clock_faults(pool: PgPool) {
    let _case = media_seed(&pool).await;
    activate(&pool).await;
    for after in [0, 1, 2, 3] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            after,
            value: Err(ContextError::ClockUnavailable),
        };
        let result = server_quote(&pool, &clock, Locale::Nl, &basket(1), &[policy()]).await;
        assert!(matches!(
            result,
            Err(QuoteFailure::ClockUnavailable)
                | Err(QuoteFailure::Catalogue(
                    commerce_infrastructure::published_catalogue::PublishedError::ClockUnavailable
                ))
        ));
    }
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 3,
        value: Ok(2000),
    };
    rejected(
        server_quote(&pool, &clock, Locale::Nl, &basket(1), &[policy()]).await,
        QuoteError::ProductUnavailable,
    );
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        after: 3,
        value: Ok(1200),
    };
    let mut p = policy();
    p.expires_at = 1200;
    rejected(
        server_quote(&pool, &clock, Locale::Nl, &basket(1), &[p]).await,
        QuoteError::MarketUnavailable,
    );
    for value in [1099, u64::MAX] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            after: 3,
            value: Ok(value),
        };
        assert!(matches!(
            server_quote(&pool, &clock, Locale::Nl, &basket(1), &[policy()]).await,
            Err(QuoteFailure::ClockUnavailable)
        ));
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn dependency_failure_never_returns_partial_quote(pool: PgPool) {
    pool.close().await;
    assert!(matches!(
        quote(&pool, &basket(1), &[policy()]).await,
        Err(QuoteFailure::Catalogue(
            commerce_infrastructure::published_catalogue::PublishedError::Database(_)
        ))
    ));
}
include!("support/quote_waits.rs");
