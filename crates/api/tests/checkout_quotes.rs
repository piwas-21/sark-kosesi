//! HTTP checks use invented 1970 clocks and manually replayed cookies, not browser acceptance.
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/checkout_quote_fixture.rs"]
mod fixture;
mod guest_support;
use axum::{Router, http::StatusCode};
use commerce_infrastructure::MIGRATOR;
use guest_support::*;
use sqlx::PgPool;
use std::sync::atomic::Ordering;
use tower::ServiceExt;
const INPUT: &str =
    r#"{"expected_basket_revision":1,"effect_key":"first","country":"NL","postcode":"1234 AB"}"#;
async fn ready(pool: &PgPool) -> (Router, std::sync::Arc<Time>, String, String) {
    fixture::seed(pool).await;
    let (app, time) = app(pool);
    time.0.store(1100, Ordering::SeqCst);
    let (cookie, csrf) = issue(&app, None).await;
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/guest/basket",
            Some(&cookie),
            Some(&csrf),
            &update("basket", 0, 3).replace("SET-A", "SET-FILLED"),
        ))
        .await
        .expect("intent");
    assert_eq!(response.status(), StatusCode::OK);
    (app, time, cookie, csrf)
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quote_contract_hides_internal_bindings_and_replays_exact_owner_view(pool: PgPool) {
    let (app, time, cookie, csrf) = ready(&pool).await;
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&csrf),
            INPUT,
        ))
        .await
        .expect("quote");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let first = json(response).await;
    assert_eq!(first["replayed"], false);
    assert_eq!(
        first["quote"]["total"],
        serde_json::json!({"gross_minor":38245,"net_minor":31607,"vat_minor":6638})
    );
    assert_eq!(first["quote"]["indicative"], true);
    assert_eq!(first["quote"]["locale"], "nl");
    let serialized = first.to_string();
    for hidden in [
        "basket_id",
        "policy_id",
        "policy_revision",
        "evidence",
        "1234 AB",
        "author",
        "csrf",
    ] {
        assert!(!serialized.contains(hidden), "{hidden}");
    }
    let id = first["quote"]["id"].as_str().expect("ID");
    let path = format!("/api/guest/quotes/{id}");
    let response = app
        .clone()
        .oneshot(request("GET", &path, Some(&cookie), None, ""))
        .await
        .expect("read");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await, first["quote"]);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&csrf),
            INPUT,
        ))
        .await
        .expect("retry");
    let retry = json(response).await;
    assert_eq!(retry["replayed"], true);
    assert_eq!(retry["quote"], first["quote"]);
    let (other, _) = issue(&app, None).await;
    for (owner, status) in [
        (Some(other.as_str()), StatusCode::NOT_FOUND),
        (None, StatusCode::UNAUTHORIZED),
    ] {
        let response = app
            .clone()
            .oneshot(request("GET", &path, owner, None, ""))
            .await
            .expect("owner");
        assert_eq!(response.status(), status);
    }
    time.0.store(1400, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(request("GET", &path, Some(&cookie), None, ""))
        .await
        .expect("expired");
    assert_eq!(response.status(), StatusCode::GONE);
    assert_eq!(fixture::counts(&pool).await, (1, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quote_http_rejects_forged_authority_unready_market_and_internal_locale(pool: PgPool) {
    let (app, _, cookie, csrf) = ready(&pool).await;
    for (language, body, token, status) in [
        ("nl", INPUT, None, StatusCode::FORBIDDEN),
        ("kmr", INPUT, Some(csrf.as_str()), StatusCode::BAD_REQUEST),
        ("ku", INPUT, Some(csrf.as_str()), StatusCode::CONFLICT),
        (
            "nl",
            r#"{"expected_basket_revision":1,"effect_key":"unready","country":"DE","postcode":"1234 AB"}"#,
            Some(csrf.as_str()),
            StatusCode::CONFLICT,
        ),
        ("nl", "[]", Some(csrf.as_str()), StatusCode::BAD_REQUEST),
        (
            "nl",
            r#"{"expected_basket_revision":1,"effect_key":"bad","country":"NL","postcode":"1234 AB","total_minor":1}"#,
            Some(csrf.as_str()),
            StatusCode::BAD_REQUEST,
        ),
        (
            "nl",
            r#"{"expected_basket_revision":1,"effect_key":"bad","country":"NL","postcode":"1234 AB","basket_id":"victim"}"#,
            Some(csrf.as_str()),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/api/guest/{language}/quotes"),
                Some(&cookie),
                token,
                body,
            ))
            .await
            .expect("rejection");
        assert_eq!(response.status(), status, "{language} {body}");
    }
    let mut foreign = request(
        "POST",
        "/api/guest/nl/quotes",
        Some(&cookie),
        Some(&csrf),
        INPUT,
    );
    foreign
        .headers_mut()
        .insert("origin", "https://evil.example".parse().expect("origin"));
    let response = app.clone().oneshot(foreign).await.expect("origin");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&csrf),
            &" ".repeat(4097),
        ))
        .await
        .expect("bound");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture::counts(&pool).await, (0, 0, 0));
    for n in 0..11 {
        let input = INPUT.replace("first", &format!("quota-{n}"));
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/api/guest/nl/quotes",
                Some(&cookie),
                Some(&csrf),
                &input,
            ))
            .await
            .expect("quota");
        if n < 10 {
            assert_eq!(response.status(), StatusCode::OK);
        } else {
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()["retry-after"], "60");
        }
    }
    assert_eq!(fixture::counts(&pool).await, (10, 0, 0));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quote_http_service_faults_and_invalid_identifiers_use_static_private_errors(pool: PgPool) {
    use sqlx::Executor;
    let (app, _, cookie, csrf) = ready(&pool).await;
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/guest/quotes/bad",
            Some(&cookie),
            None,
            "",
        ))
        .await
        .expect("invalid");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            None,
            Some(&csrf),
            INPUT,
        ))
        .await
        .expect("unauthorized");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&"0".repeat(64)),
            INPUT,
        ))
        .await
        .expect("CSRF");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let mut plain = request(
        "POST",
        "/api/guest/nl/quotes",
        Some(&cookie),
        Some(&csrf),
        INPUT,
    );
    plain
        .headers_mut()
        .insert("content-type", "text/plain".parse().expect("type"));
    let response = app.clone().oneshot(plain).await.expect("type");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&csrf),
            &INPUT.replace("revision\":1", "revision\":0"),
        ))
        .await
        .expect("revision");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&csrf),
            INPUT,
        ))
        .await
        .expect("positive");
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/de/quotes",
            Some(&cookie),
            Some(&csrf),
            INPUT,
        ))
        .await
        .expect("intent conflict");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json(response).await["code"], "quote_intent_conflict");
    pool.execute("DROP TABLE guest_quote_snapshots CASCADE")
        .await
        .expect("outage fixture");
    for (method, path, body) in [
        ("POST", "/api/guest/nl/quotes", INPUT),
        (
            "GET",
            "/api/guest/quotes/11111111111111111111111111111111",
            "",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(method, path, Some(&cookie), Some(&csrf), body))
            .await
            .expect("dependency failure");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        let body = json(response).await.to_string();
        assert!(body.contains("quote_service_unavailable"));
        assert!(!body.contains("guest_quote_snapshots"));
        assert!(!body.contains("1234 AB"));
    }
}
