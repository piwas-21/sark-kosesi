//! Invented isolated acceptance data, never supplier/native/legal/provider evidence.
#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}
use commerce_application::{
    Locale,
    guest_access::GuestLimits,
    request_context::{Clock, ContextError, Identifiers},
};
use commerce_domain::{
    Money,
    factual_review::FactDecision,
    guest_baskets::BasketUpdate,
    quotes::{BasketLine, Destination, MarketPolicy, ProductRule, ShippingRule},
};
use commerce_infrastructure::{
    checkout_quotes::GuestQuoteRequest,
    guest_baskets::*,
    market_policies::{Audit, DraftRequest, ReviewRequest, record_draft, record_review},
};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tower_sessions::Session;
pub struct Time(pub AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
struct Ids(AtomicU64);
impl Identifiers for Ids {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Ok(u128::from(self.0.fetch_add(1, Ordering::SeqCst) + 1).to_be_bytes())
    }
}
pub fn store(pool: &PgPool, at: u64) -> (GuestSessionStore, Arc<Time>) {
    let clock = Arc::new(Time(AtomicU64::new(at)));
    (
        GuestSessionStore::new(
            pool.clone(),
            clock.clone(),
            Arc::new(Ids(AtomicU64::new(100))),
            GuestLimits::default(),
        )
        .expect("fixture store"),
        clock,
    )
}
pub fn policy() -> MarketPolicy {
    MarketPolicy {
        id: "SNAPSHOT-NL".into(),
        revision: 1,
        destination: Destination {
            country: "NL".into(),
            postcode: "1234 AB".into(),
        },
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
        evidence_ref: "synthetic policy, never tax/carrier readiness".into(),
    }
}
pub async fn seed(pool: &PgPool) {
    let _guard = catalogue::media_seed(pool).await;
    catalogue::approve(pool).await;
    commerce_infrastructure::publication::record_publication(
        pool,
        &catalogue::author(),
        &Time(AtomicU64::new(1100)),
        commerce_infrastructure::publication::PublicationRequest {
            locale: Locale::Nl,
            sku: "SET-FILLED",
            expected_revision: 0,
            effect_key: "snapshot-activate",
            choice: &catalogue::choice(),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("synthetic publication");
    pool.execute("INSERT INTO stock_balances(component_code,on_hand,quarantine,reserved) VALUES('SEAT-COVER',100,0,0),('SEAT-FOAM',100,0,0);INSERT INTO staff_capabilities VALUES('author','market.manage'),('reviewer','market.review')").await.expect("isolated ready fixture");
    draft(pool, &policy()).await;
    decide(pool, 1, 0, true, "snapshot-review").await;
}
pub async fn draft(pool: &PgPool, p: &MarketPolicy) {
    record_draft(
        pool,
        &catalogue::author(),
        &Time(AtomicU64::new(1100)),
        DraftRequest {
            policy: p,
            audit: Audit {
                expected_revision: p.revision - 1,
                effect_key: &format!("snapshot-policy-{}", p.revision),
                reason: "synthetic",
                confirmed: true,
            },
        },
    )
    .await
    .expect("synthetic policy draft");
}
pub async fn decide(pool: &PgPool, draft: i32, expected: i32, approved: bool, key: &str) {
    record_review(
        pool,
        &catalogue::reviewer(),
        &Time(AtomicU64::new(1100)),
        ReviewRequest {
            id: "SNAPSHOT-NL",
            draft_revision: draft,
            decision: if approved {
                FactDecision::Approved
            } else {
                FactDecision::Rejected
            },
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            audit: Audit {
                expected_revision: expected,
                effect_key: key,
                reason: "synthetic",
                confirmed: true,
            },
        },
    )
    .await
    .expect("synthetic independent decision");
}
pub async fn guest(store: &GuestSessionStore) -> (Session, String) {
    let session = Session::new(None, Arc::new(store.clone()), None);
    let v = ensure_session(store, &session).await.expect("guest");
    let csrf = v.csrf_token().to_owned();
    let key = GuestCredential::from_session(&session).expect("credential");
    write_basket(
        store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 0,
            effect_key: "basket-first".into(),
            lines: vec![BasketLine {
                sku: "SET-FILLED".into(),
                quantity: 3,
            }],
        },
    )
    .await
    .expect("basket intent");
    (session, csrf)
}
pub fn request(effect: &str) -> GuestQuoteRequest {
    GuestQuoteRequest {
        expected_basket_revision: 1,
        effect_key: effect.into(),
        destination: policy().destination,
    }
}
pub async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)").fetch_one(pool).await.expect("quote never reserves or finances")
}
