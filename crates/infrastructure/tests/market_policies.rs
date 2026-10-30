use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::{Money, factual_review::FactDecision, quotes::*};
use commerce_infrastructure::{
    MIGRATOR,
    market_policies::*,
    staff_access::{SessionToken, StaffError},
};
use sqlx::{Executor, PgPool};
struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn token(number: u8) -> SessionToken {
    SessionToken::from_bytes([number; 32])
}
async fn seed(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES('author',true),('reviewer',true),('other',true),('catalogue',true); INSERT INTO staff_capabilities VALUES('author','market.manage'),('reviewer','market.review'),('other','market.review'),('catalogue','catalogue.manage')").await.expect("isolated grants");
    for (n, id) in [
        (1, "author"),
        (2, "reviewer"),
        (3, "other"),
        (4, "catalogue"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)")
            .bind(token(n).digest().as_slice()).bind(id).execute(pool).await.expect("isolated session");
    }
}
fn policy() -> MarketPolicy {
    MarketPolicy {
        id: "SYNTHETIC-NL".into(),
        revision: 1,
        destination: Destination {
            country: "NL".into(),
            postcode: "1234 AB".into(),
        },
        valid_from: 1000,
        expires_at: 2000,
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
        evidence_ref: "synthetic tax/carrier/territory/returns evidence; not actual readiness"
            .into(),
    }
}
fn audit(key: &str, revision: i32) -> Audit<'_> {
    Audit {
        effect_key: key,
        expected_revision: revision,
        reason: "isolated policy test",
        confirmed: true,
    }
}
fn review(key: &str, draft: i32, expected: i32, approved: bool) -> ReviewRequest<'_> {
    ReviewRequest {
        id: "SYNTHETIC-NL",
        draft_revision: draft,
        decision: if approved {
            FactDecision::Approved
        } else {
            FactDecision::Rejected
        },
        evidence_ref: "synthetic independent tax/carrier/territory/returns review",
        qualification_ref: "synthetic qualification",
        audit: audit(key, expected),
    }
}
async fn draft(pool: &PgPool, p: &MarketPolicy, key: &str) -> Result<Receipt, MarketError> {
    record_draft(
        pool,
        &token(1),
        &FixedClock(1100),
        DraftRequest {
            policy: p,
            audit: audit(key, p.revision - 1),
        },
    )
    .await
}
async fn decide(
    pool: &PgPool,
    key: &str,
    d: i32,
    r: i32,
    yes: bool,
) -> Result<Receipt, MarketError> {
    record_review(pool, &token(2), &FixedClock(1100), review(key, d, r, yes)).await
}
async fn inspect(pool: &PgPool, revision: i32) -> Result<PolicyInspection, MarketError> {
    inspect_policy(pool, &token(2), &FixedClock(1100), "SYNTHETIC-NL", revision).await
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn independent_history_retries_withdrawal_and_new_draft_never_reactivate_old_approval(
    pool: PgPool,
) {
    seed(&pool).await;
    assert!(matches!(
        inspect(&pool, 1).await,
        Err(MarketError::NotFound)
    ));
    assert_eq!(
        draft(&pool, &policy(), "first").await.expect("draft"),
        Receipt {
            revision: 1,
            replayed: false
        }
    );
    let pending = inspect(&pool, 1).await.expect("review-only inspection");
    assert_eq!(pending.policy, policy());
    assert_eq!(
        (&*pending.author, &*pending.reason, pending.review.is_none()),
        ("author", "isolated policy test", true)
    );
    assert_eq!(
        decide(&pool, "approved", 1, 0, true)
            .await
            .expect("independent"),
        Receipt {
            revision: 1,
            replayed: false
        }
    );
    let approved = inspect(&pool, 1)
        .await
        .expect("approved")
        .review
        .expect("decision");
    assert!(approved.approved);
    assert_eq!(
        (
            approved.revision,
            approved.reviewer.as_str(),
            approved.reason.as_str()
        ),
        (1, "reviewer", "isolated policy test")
    );
    assert_eq!(approved.evidence_ref, review("x", 1, 0, true).evidence_ref);
    assert_eq!(approved.qualification_ref, "synthetic qualification");
    decide(&pool, "withdraw", 1, 1, false)
        .await
        .expect("withdraw");
    assert!(
        decide(&pool, "approved", 1, 0, true)
            .await
            .expect("historical receipt")
            .replayed
    );
    assert!(
        !inspect(&pool, 1)
            .await
            .expect("latest")
            .review
            .expect("withdrawal")
            .approved
    );
    let mut p = policy();
    p.revision = 2;
    p.shipping.gross = Money::eur(999).expect("fixture");
    draft(&pool, &p, "second").await.expect("new source");
    assert!(
        draft(&pool, &policy(), "first")
            .await
            .expect("historical draft")
            .replayed
    );
    assert!(
        inspect(&pool, 2)
            .await
            .expect("new draft pending")
            .review
            .is_none()
    );
    assert!(matches!(
        decide(&pool, "stale", 1, 2, true).await,
        Err(MarketError::Conflict)
    ));
    decide(&pool, "historical-reject", 1, 2, false)
        .await
        .expect("old withdrawal allowed");
    decide(&pool, "second-approve", 2, 0, true)
        .await
        .expect("exact new draft");
    assert!(
        inspect(&pool, 2)
            .await
            .expect("current")
            .review
            .expect("review")
            .approved
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn least_privilege_confirmation_and_all_command_fields_are_enforced(pool: PgPool) {
    seed(&pool).await;
    for n in [2, 3, 4, 99] {
        assert!(matches!(
            record_draft(
                &pool,
                &token(n),
                &FixedClock(1100),
                DraftRequest {
                    policy: &policy(),
                    audit: audit("deny", 0)
                }
            )
            .await,
            Err(MarketError::Staff(StaffError::Denied))
        ));
    }
    draft(&pool, &policy(), "first").await.expect("draft");
    for n in [1, 4, 99] {
        assert!(matches!(
            record_review(
                &pool,
                &token(n),
                &FixedClock(1100),
                review("deny", 1, 0, true)
            )
            .await,
            Err(MarketError::Staff(StaffError::Denied))
        ));
        assert!(matches!(
            inspect_policy(&pool, &token(n), &FixedClock(1100), "SYNTHETIC-NL", 1).await,
            Err(MarketError::Staff(StaffError::Denied))
        ));
    }
    pool.execute("INSERT INTO staff_capabilities VALUES('author','market.review')")
        .await
        .expect("dual grant");
    assert!(matches!(
        record_review(
            &pool,
            &token(1),
            &FixedClock(1100),
            review("self", 1, 0, true)
        )
        .await,
        Err(MarketError::SelfReview)
    ));
    for case in 0..10 {
        let long = "a".repeat(501);
        let mut a = audit("bad", 0);
        match case {
            0 => a.confirmed = false,
            1 => a.expected_revision = -1,
            2 => a.expected_revision = i32::MAX,
            3 => a.effect_key = "",
            4 => a.effect_key = "bad_key",
            5 => a.reason = "",
            6 => a.reason = "  ",
            7 => a.reason = &long,
            8 => a.reason = "line\nbreak",
            _ => a.effect_key = &long,
        }
        assert!(
            matches!(
                record_draft(
                    &pool,
                    &token(1),
                    &FixedClock(1100),
                    DraftRequest {
                        policy: &policy(),
                        audit: a
                    }
                )
                .await,
                Err(MarketError::InvalidRequest)
            ),
            "draft case {case}"
        );
        let mut r = review("bad", 1, 0, true);
        r.audit = a;
        assert!(
            matches!(
                record_review(&pool, &token(2), &FixedClock(1100), r).await,
                Err(MarketError::InvalidRequest)
            ),
            "review case {case}"
        );
    }
    for case in 0..5 {
        let mut r = review("bad", 1, 0, true);
        match case {
            0 => r.id = "DEMO-INVALID",
            1 => r.draft_revision = 0,
            2 => r.evidence_ref = "",
            3 => r.qualification_ref = "",
            _ => r.audit.confirmed = false,
        }
        assert!(matches!(
            record_review(&pool, &token(2), &FixedClock(1100), r).await,
            Err(MarketError::InvalidRequest)
        ));
    }
    for (id, rev) in [("", 1), ("SYNTHETIC-NL", 0)] {
        assert!(matches!(
            inspect_policy(&pool, &token(2), &FixedClock(1100), id, rev).await,
            Err(MarketError::InvalidRequest)
        ));
    }
    assert!(matches!(
        inspect(&pool, 2).await,
        Err(MarketError::NotFound)
    ));
    let mut p = policy();
    p.revision = 3;
    assert!(matches!(
        record_draft(
            &pool,
            &token(1),
            &FixedClock(1100),
            DraftRequest {
                policy: &p,
                audit: audit("mismatch", 1)
            }
        )
        .await,
        Err(MarketError::InvalidRequest)
    ));
    p = policy();
    p.products.clear();
    assert!(matches!(
        draft(&pool, &p, "invalid").await,
        Err(MarketError::InvalidRequest)
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn effect_keys_bind_full_draft_review_intent_and_session_actor(pool: PgPool) {
    seed(&pool).await;
    draft(&pool, &policy(), "draft").await.expect("draft");
    for case in 0..7 {
        let mut p = policy();
        let mut a = audit("draft", 0);
        let mut n = 1;
        match case {
            0 => p.evidence_ref = "changed".into(),
            1 => p.shipping.gross = Money::eur(1).expect("fixture"),
            2 => p.revision = 2,
            3 => p.id = "OTHER".into(),
            4 => a.reason = "changed",
            5 => {
                n = 3;
                pool.execute("INSERT INTO staff_capabilities VALUES('other','market.manage')")
                    .await
                    .expect("grant");
            }
            _ => p.destination.postcode = "1234 AC".into(),
        }
        if case == 2 {
            a.expected_revision = 1;
        }
        assert!(
            matches!(
                record_draft(
                    &pool,
                    &token(n),
                    &FixedClock(1100),
                    DraftRequest {
                        policy: &p,
                        audit: a
                    }
                )
                .await,
                Err(MarketError::Conflict)
            ),
            "draft {case}"
        );
    }
    decide(&pool, "review", 1, 0, true).await.expect("review");
    let mut other = policy();
    other.id = "OTHER".into();
    other.destination.postcode = "9999 ZZ".into();
    draft(&pool, &other, "other-destination")
        .await
        .expect("independent existing destination");
    let mut p = policy();
    p.revision = 2;
    draft(&pool, &p, "new").await.expect("second");
    for case in 0..8 {
        let mut r = review("review", 1, 0, true);
        let mut n = 2;
        match case {
            0 => r.id = "OTHER",
            1 => r.draft_revision = 2,
            2 => r.audit.expected_revision = 1,
            3 => r.decision = FactDecision::Rejected,
            4 => r.evidence_ref = "changed",
            5 => r.qualification_ref = "changed",
            6 => r.audit.reason = "changed",
            _ => n = 3,
        }
        let result = record_review(&pool, &token(n), &FixedClock(1100), r).await;
        assert!(
            matches!(
                result,
                Err(MarketError::Conflict) | Err(MarketError::NotFound)
            ),
            "review {case}: {result:?}"
        );
    }
    assert!(matches!(
        decide(&pool, "stale-review", 1, 5, false).await,
        Err(MarketError::Conflict)
    ));
    let mut reordered = policy();
    reordered.revision = 3;
    reordered.products.push(ProductRule {
        sku: "AAA".into(),
        ..reordered.products[0].clone()
    });
    draft(&pool, &reordered, "multi").await.expect("multi");
    reordered.products.reverse();
    assert!(
        draft(&pool, &reordered, "multi")
            .await
            .expect("canonical replay")
            .replayed
    );
}
include!("support/market_waits.rs");
include!("support/market_schema.rs");
include!("support/market_quotes.rs");

#[test]
fn persisted_policy_contract_round_trips_and_rejects_unknown_and_unsafe_money() {
    let p = policy();
    let value = serde_json::to_value(&p).expect("serialize trusted policy");
    assert_eq!(
        serde_json::from_value::<MarketPolicy>(value.clone()).expect("closed policy"),
        p
    );
    for path in ["root", "destination", "product", "shipping", "money"] {
        let mut v = value.clone();
        let target = match path {
            "root" => &mut v,
            "destination" => &mut v["destination"],
            "product" => &mut v["products"][0],
            "shipping" => &mut v["shipping"],
            _ => &mut v["shipping"]["gross"],
        };
        target["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MarketPolicy>(v).is_err());
    }
    for bad in [
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!("100"),
    ] {
        let mut v = value.clone();
        v["shipping"]["gross"]["minor_units"] = bad;
        assert!(serde_json::from_value::<MarketPolicy>(v).is_err());
    }
    let mut v = value;
    v["shipping"]["gross"]["currency"] = serde_json::json!("USD");
    assert!(serde_json::from_value::<MarketPolicy>(v).is_err());
}
