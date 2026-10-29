#[allow(dead_code, unused_imports)] // Complete isolated workflow setup, shared with adapter cases.
mod fixture {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use commerce_api::{AppState, router};
use commerce_application::request_context::Identifiers;
use fixture::*;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tower::ServiceExt;
struct Ids;
impl Identifiers for Ids {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Ok([1; 16])
    }
}
fn app(pool: PgPool) -> axum::Router {
    router(AppState {
        pool,
        preview_token: Arc::from("isolated-preview-token"),
        capacity: Arc::new(Semaphore::new(96)),
        clock: Arc::new(FixedClock(1100)),
        identifiers: Arc::new(Ids),
    })
}
fn get(path: &str, authenticated: bool) -> Request<Body> {
    let builder = Request::builder().uri(path);
    let builder = if authenticated {
        builder.header("authorization", "Bearer isolated-preview-token")
    } else {
        builder
    };
    builder.body(Body::empty()).expect("request")
}
async fn json(app: &axum::Router, path: &str) -> serde_json::Value {
    let r = app.clone().oneshot(get(path, true)).await.expect("HTTP");
    assert_eq!(r.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(r.into_body(), 131072).await.expect("bytes")).expect("JSON")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn explicit_whitelist_price_copy_stock_and_authenticated_sanitized_bytes(pool: PgPool) {
    let _case = media_seed(&pool).await;
    approve(&pool).await;
    record_publication(
        &pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice()),
    )
    .await
    .expect("activate");
    let api = app(pool.clone());
    for path in [
        "/api/published/nl",
        "/api/published/nl/products/SET-FILLED",
        "/api/published/nl/images/SET-FILLED/1/320",
    ] {
        let r = api.clone().oneshot(get(path, false)).await.expect("HTTP");
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(r.headers()["cache-control"], "private, no-store");
    }
    let page = json(&api, "/api/published/nl").await;
    assert_eq!(page["production_enabled"], false);
    assert_eq!(page["next_after"], serde_json::Value::Null);
    assert_eq!(page["locale"], "nl");
    let p = json(&api, "/api/published/nl/products/SET-FILLED").await;
    assert_eq!(page["items"][0], p);
    assert_eq!(p["minor_units"], 12345);
    assert_eq!(p["price_decimal"], "123.45");
    assert_eq!(p["currency"], "EUR");
    assert_eq!(p["copy"]["title"], "Isolated acceptance sample");
    assert_eq!(
        p["components"][0]["material"]["fibres"][0]["basis_points"],
        6000
    );
    assert_eq!(p["components"][1]["material"]["density_g_per_m3"], 35000);
    assert_eq!(p["availability"], "out_of_stock");
    let keys: std::collections::BTreeSet<_> = p
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "sku",
            "locale",
            "publication_revision",
            "family_code",
            "family_slug",
            "size_code",
            "textile_code",
            "fill",
            "minor_units",
            "currency",
            "price_decimal",
            "copy",
            "footprint",
            "components",
            "parcels",
            "images",
            "availability"
        ]
        .into_iter()
        .collect()
    );
    let text = p.to_string();
    for forbidden in [
        "source_ref",
        "evidence_ref",
        "qualification_ref",
        "traceability_ref",
        "actor",
        "asset_id",
        "sha256",
        "on_hand",
        "quarantine",
        "reserved",
        "photo",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
    let url = p["images"][0]["url"].as_str().expect("URL");
    assert_eq!(url, "/api/published/nl/images/SET-FILLED/1/320");
    let r = api.clone().oneshot(get(url, true)).await.expect("bytes");
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()["content-type"], "image/jpeg");
    assert_eq!(r.headers()["x-content-type-options"], "nosniff");
    assert_eq!(r.headers()["cache-control"], "private, no-store");
    assert_eq!(r.headers()["x-robots-tag"], "noindex, nofollow");
    let bytes = to_bytes(r.into_body(), 10000).await.expect("bytes");
    assert_eq!(
        image::load_from_memory(&bytes).expect("derivative").width(),
        32
    );
    record_price_revision(
        &pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "SET-FILLED",
            expected_revision: 1,
            effect_key: "maximum-price",
            price: commerce_domain::Money::eur(9007199254740991).expect("bounded money"),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("price");
    let mut selection = selected();
    selection.facts.price_revision = 2;
    record_factual_review(
        &pool,
        &reviewer(),
        &FixedClock(1100),
        FactReviewRequest {
            sku: "SET-FILLED",
            target: selection.facts,
            expected_review_revision: 0,
            effect_key: "maximum-price-review",
            decision: FactDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("price review");
    let c = PublicationChoice::Activate { selection };
    let mut r = request("maximum-price-publish", &c);
    r.expected_revision = 1;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("republication");
    sqlx::query(
        "INSERT INTO stock_balances(component_code,on_hand) VALUES('SEAT-COVER',2),('SEAT-FOAM',2)",
    )
    .execute(&pool)
    .await
    .expect("physical receipt");
    let maximum = json(&api, "/api/published/nl/products/SET-FILLED").await;
    assert_eq!(maximum["price_decimal"], "90071992547409.91");
    assert_eq!(maximum["availability"], "in_stock");
    let mut r = request("withdraw", &PublicationChoice::Withdraw {});
    r.expected_revision = 2;
    record_publication(&pool, &author(), &FixedClock(1100), r)
        .await
        .expect("withdraw");
    for path in ["/api/published/nl/products/SET-FILLED", url] {
        assert_eq!(
            api.clone()
                .oneshot(get(path, true))
                .await
                .expect("HTTP")
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        json(&api, "/api/published/nl").await["items"],
        serde_json::json!([])
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn empty_locales_invalid_requests_and_database_failures_are_explicit(pool: PgPool) {
    let api = app(pool.clone());
    for locale in ["nl", "de", "fr", "en", "ar", "tr", "ku"] {
        let p = json(&api, &format!("/api/published/{locale}")).await;
        assert_eq!(p["locale"], locale);
        assert_eq!(p["items"], serde_json::json!([]));
    }
    for path in [
        "/api/published/kmr",
        "/api/published/nl?after=bad",
        "/api/published/nl?unknown=bad",
        "/api/published/nl/products/bad",
        "/api/published/nl/images/SET-FILLED/0/320",
        "/api/published/kmr/products/SET-FILLED",
        "/api/published/kmr/images/SET-FILLED/1/320",
        "/api/published/nl/images/SET-FILLED/1/0",
        "/api/published/nl/images/SET-FILLED/bad/320",
    ] {
        assert_eq!(
            api.clone()
                .oneshot(get(path, true))
                .await
                .expect("HTTP")
                .status(),
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    pool.close().await;
    for path in [
        "/api/published/nl",
        "/api/published/nl/products/SET-FILLED",
        "/api/published/nl/images/SET-FILLED/1/320",
    ] {
        let r = api.clone().oneshot(get(path, true)).await.expect("HTTP");
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            &to_bytes(r.into_body(), 1024).await.expect("error")[..],
            br#"{"code":"published_catalogue_unavailable"}"#
        );
    }
}
