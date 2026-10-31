//! Isolated invented promotion terms, never a live offer or real qualification.
use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::{factual_review::FactDecision, promotion_policies::*, promotions::*};
use commerce_infrastructure::{promotion_policies::*, staff_access::SessionToken};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};
pub struct Time(pub AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
pub fn clock() -> Time {
    Time(AtomicU64::new(1100))
}
pub fn token(n: u8) -> SessionToken {
    SessionToken::from_bytes([n; 32])
}
pub async fn seed(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES('author',true),('reviewer',true),('other',true),('market',true); INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review'),('other','promotion.review'),('market','market.manage'),('market','market.review')").await.expect("isolated grants");
    for (n, id) in [(1, "author"), (2, "reviewer"), (3, "other"), (4, "market")] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(token(n).digest().as_slice()).bind(id).execute(pool).await.expect("isolated session");
    }
}
pub fn policy() -> PromotionPolicy {
    PromotionPolicy {
        promotion: Promotion {
            id: "COURTYARD".into(),
            revision: 1,
            valid_from: 1000,
            expires_at: 2000,
            countries: vec!["DE".into(), "NL".into()],
            products: vec![EligibleProduct {
                sku: "SET-FILLED".into(),
                publication_revision: 1,
            }],
            minimum_eligible_gross_minor: 1000,
            maximum_discount_minor: 500,
            discount: DiscountKind::Fixed { minor_units: 500 },
            evidence_ref: "synthetic proposal".into(),
        },
        limits: RedemptionLimits {
            total_discount_minor: 10000,
            total_redemptions: 20,
            per_customer_redemptions: None,
        },
    }
}
pub fn audit(key: &str, expected: i32) -> Audit<'_> {
    Audit {
        expected_revision: expected,
        effect_key: key,
        reason: "synthetic promotion",
        confirmed: true,
    }
}
pub fn review(key: &str, draft: i32, expected: i32, approved: bool) -> ReviewRequest<'_> {
    ReviewRequest {
        id: "COURTYARD",
        draft_revision: draft,
        decision: if approved {
            FactDecision::Approved
        } else {
            FactDecision::Rejected
        },
        evidence_ref: "synthetic review",
        qualification_ref: "synthetic qualification",
        audit: audit(key, expected),
    }
}
pub async fn draft(pool: &PgPool, p: &PromotionPolicy, key: &str) -> Result<Receipt, PolicyError> {
    record_draft(
        pool,
        &token(1),
        &clock(),
        DraftRequest {
            policy: p,
            audit: audit(key, p.promotion.revision - 1),
        },
    )
    .await
}
pub async fn decide(
    pool: &PgPool,
    key: &str,
    d: i32,
    r: i32,
    yes: bool,
) -> Result<Receipt, PolicyError> {
    record_review(pool, &token(2), &clock(), review(key, d, r, yes)).await
}
pub async fn inspect(pool: &PgPool, r: i32) -> Result<PolicyInspection, PolicyError> {
    inspect_policy(pool, &token(2), &clock(), "COURTYARD", r).await
}
pub async fn approved(pool: &PgPool) -> Result<ApprovedPolicy, PolicyError> {
    approved_policy(pool, &clock(), "COURTYARD", "NL").await
}
pub async fn ready(pool: &PgPool) {
    seed(pool).await;
    draft(pool, &policy(), "initial").await.expect("draft");
    decide(pool, "approve", 1, 0, true).await.expect("review");
}
