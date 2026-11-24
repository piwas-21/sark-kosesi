//! Isolated synthetic order HTTP custody, never production or legal evidence.
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/checkout_quote_fixture.rs"]
mod fixture;
#[allow(dead_code)]
mod guest_support;
#[allow(dead_code)]
#[path = "../../../tests/fixtures/checkout_order_snapshot.rs"]
mod order_fixture;
use axum::{Router, http::StatusCode};
use commerce_api::{
    TestGuestOperations, config::Config, guest::test_orders::TestOrderConfig,
    router_with_test_guest_operations,
};
use commerce_infrastructure::MIGRATOR;
use guest_support::*;
use sqlx::{Executor, PgPool};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
};
use tower::ServiceExt;

const URL: &str = "/api/test/guest/orders";
const QUOTE: &str =
    r#"{"expected_basket_revision":1,"effect_key":"quote","country":"NL","postcode":"1234 AB"}"#;
fn settings() -> HashMap<String, String> {
    HashMap::from([
        ("APP_ENV".into(), "test".into()),
        (
            "DATABASE_URL".into(),
            "postgresql://localhost/unused".into(),
        ),
        ("PREVIEW_TOKEN".into(), TOKEN.into()),
        ("API_BIND".into(), "127.0.0.1:3000".into()),
        ("GUEST_ORIGIN".into(), ORIGIN.into()),
        ("TEST_GUEST_ORDER_TRANSPORT".into(), "1".into()),
        (
            "TEST_GUEST_ORDER_SELLER".into(),
            serde_json::to_string(&order_fixture::seller()).expect("synthetic policy"),
        ),
    ])
}
fn configured_app(pool: &PgPool, values: &HashMap<String, String>) -> (Router, Arc<Time>) {
    let (state, clock) = state(pool);
    let config = Config::parse(values).expect("test config");
    (
        router_with_test_guest_operations(
            state,
            config.guest_origin,
            None,
            None,
            None,
            TestGuestOperations {
                order: config.test_guest_order,
                cancellation: config.test_guest_cancellation,
                pending_cancellation: config.test_guest_pending_cancellation,
            },
        )
        .expect("router"),
        clock,
    )
}
fn payload(quote: &str) -> String {
    serde_json::json!({
        "effect_key":"place", "quote_id":quote, "expected_basket_revision":1,
        "buyer":{"name":"Synthetic Buyer", "email":"buyer@example.test"},
        "shipping_address":order_fixture::address(), "billing_address":order_fixture::address(),
        "accepted_terms_version":"synthetic-terms-v1", "confirmed":true
    })
    .to_string()
}
async fn send(
    app: &Router,
    cookie: Option<&str>,
    csrf: Option<&str>,
    input: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(request("POST", URL, cookie, csrf, input))
        .await
        .expect("response")
}
async fn ready(pool: &PgPool) -> (Router, Arc<Time>, String, String, String) {
    ready_with(pool, &settings()).await
}
async fn ready_with(
    pool: &PgPool,
    values: &HashMap<String, String>,
) -> (Router, Arc<Time>, String, String, String) {
    fixture::seed(pool).await;
    let (app, time) = configured_app(pool, values);
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
        .expect("basket");
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/guest/nl/quotes",
            Some(&cookie),
            Some(&csrf),
            QUOTE,
        ))
        .await
        .expect("quote");
    assert_eq!(response.status(), StatusCode::OK);
    let quote = json(response).await["quote"]["id"]
        .as_str()
        .expect("quote id")
        .to_owned();
    (app, time, cookie, csrf, quote)
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn test_routes_accept_then_cancel_one_order_under_the_same_guest_authority(pool: PgPool) {
    let mut values = settings();
    values.insert("TEST_GUEST_CANCELLATION_TRANSPORT".into(), "1".into());
    let config = Config::parse(&values).expect("both operations explicitly enabled");
    let (unconfigured, _) = state(&pool);
    for (order, cancellation) in [
        (config.test_guest_order, None),
        (None, config.test_guest_cancellation),
    ] {
        assert!(
            router_with_test_guest_operations(
                unconfigured.clone(),
                None,
                None,
                None,
                None,
                TestGuestOperations {
                    order,
                    cancellation,
                    ..TestGuestOperations::default()
                },
            )
            .is_err(),
            "each test operation requires the guest origin"
        );
    }
    let (app, _, cookie, csrf, quote) = ready_with(&pool, &values).await;
    let body = payload(&quote);
    let alias = cookie.replacen("__Host-piwas-guest=", "__Host-piwas-%67uest=", 1);
    assert_ne!(alias, cookie);
    let ambiguous = format!("{cookie}; {alias}");
    assert_eq!(
        send(&app, Some(&ambiguous), Some(&csrf), &body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));

    let accepted = send(&app, Some(&cookie), Some(&csrf), &body).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted = json(accepted).await;
    let order_id = accepted["id"].as_str().expect("test order ID");
    let before = counts(&pool).await;
    let cancel_url = format!("{URL}/{order_id}/cancellation");
    for replayed in [false, true] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &cancel_url,
                Some(&cookie),
                Some(&csrf),
                r#"{"confirmed":true,"effect_key":"cancel"}"#,
            ))
            .await
            .expect("cancellation response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json(response).await,
            serde_json::json!({"replayed":replayed})
        );
    }
    let effects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_canceled.v1' AND aggregate_id=$1",
    )
    .bind(order_id)
    .fetch_one(&pool)
    .await
    .expect("one committed cancellation");
    assert_eq!(effects, 1);
    assert_eq!(counts(&pool).await, before);
}
async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_created.v1'),(SELECT count(*) FROM payment_attempts)").fetch_one(pool).await.expect("custody counts")
}
#[test]
fn configuration_requires_explicit_test_mode_policy_and_guest_origin() {
    let original = settings();
    assert!(Config::parse(&original).is_ok());
    for mode in ["local", "staging", "production", ""] {
        let mut v = original.clone();
        v.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&v).is_err());
    }
    for key in [
        "GUEST_ORIGIN",
        "TEST_GUEST_ORDER_SELLER",
        "TEST_GUEST_ORDER_TRANSPORT",
    ] {
        let mut v = original.clone();
        v.remove(key);
        assert!(Config::parse(&v).is_err());
    }
    for value in ["", "true", "0", "2"] {
        let mut v = original.clone();
        v.insert("TEST_GUEST_ORDER_TRANSPORT".into(), value.into());
        assert!(Config::parse(&v).is_err());
    }
    let mut invalid_tax: serde_json::Value =
        serde_json::from_str(&original["TEST_GUEST_ORDER_SELLER"]).expect("seller");
    invalid_tax["tax_registration"] = serde_json::json!([]);
    for value in [
        invalid_tax.to_string(),
        "not json".into(),
        "{}".to_owned(),
        "[]".to_owned(),
        " ".repeat(4097),
        original["TEST_GUEST_ORDER_SELLER"].replace("SYNTHETIC-SELLER", "invalid seller"),
        original["TEST_GUEST_ORDER_SELLER"].replacen("{", "{\"id\":\"DUPLICATE\",", 1),
    ] {
        let mut v = original.clone();
        v.insert("TEST_GUEST_ORDER_SELLER".into(), value);
        assert!(Config::parse(&v).is_err());
    }
    let mut disabled = original;
    disabled.remove("TEST_GUEST_ORDER_TRANSPORT");
    disabled.remove("TEST_GUEST_ORDER_SELLER");
    assert!(
        Config::parse(&disabled)
            .expect("default")
            .test_guest_order
            .is_none()
    );
    assert!(TestOrderConfig::from_env(Some("staging"), &settings()).is_err());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_can_accept_and_retry_exact_order_without_payment(pool: PgPool) {
    let (app, time, cookie, csrf, quote) = ready(&pool).await;
    let before = counts(&pool).await;
    let body = payload(&quote);
    let (a, b) = tokio::join!(
        send(&app, Some(&cookie), Some(&csrf), &body),
        send(&app, Some(&cookie), Some(&csrf), &body)
    );
    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(b.status(), StatusCode::OK);
    assert_eq!(a.headers()["cache-control"], "private, no-store");
    let a = json(a).await;
    let b = json(b).await;
    assert_eq!(a.as_object().expect("receipt").len(), 2);
    assert_eq!(a["id"], b["id"]);
    assert_ne!(a["replayed"], b["replayed"]);
    assert_eq!(
        counts(&pool).await,
        (
            before.0 + 1,
            before.1 + 1,
            before.2 + 1,
            before.3 + 1,
            before.4
        )
    );
    let row: (String, String) = sqlx::query_as(
        "SELECT snapshot->'seller'->>'legal_name',snapshot->'buyer'->>'email' FROM checkout_orders",
    )
    .fetch_one(&pool)
    .await
    .expect("snapshot");
    assert_eq!(
        row,
        (
            "Piwas synthetic test seller".into(),
            "buyer@example.test".into()
        )
    );
    for secret in [
        "buyer",
        "example.test",
        "seller",
        "shipping",
        "1234 AB",
        "gross",
        "minor",
    ] {
        assert!(!a.to_string().contains(secret));
    }
    // Replay binds the original intent, even when server policy terms change.
    let (new_state, new_clock) = state(&pool);
    new_clock.0.store(1100, Ordering::SeqCst);
    let mut changed_config = settings();
    let mut seller: serde_json::Value =
        serde_json::from_str(&changed_config["TEST_GUEST_ORDER_SELLER"]).expect("policy");
    seller["terms_version"] = "synthetic-terms-v2".into();
    changed_config.insert("TEST_GUEST_ORDER_SELLER".into(), seller.to_string());
    let cfg = Config::parse(&changed_config).expect("changed test policy");
    let restarted = router_with_test_guest_operations(
        new_state,
        cfg.guest_origin,
        None,
        None,
        None,
        TestGuestOperations {
            order: cfg.test_guest_order,
            cancellation: cfg.test_guest_cancellation,
            pending_cancellation: cfg.test_guest_pending_cancellation,
        },
    )
    .expect("restarted router");
    let prior = send(&restarted, Some(&cookie), Some(&csrf), &body).await;
    assert_eq!(prior.status(), StatusCode::OK);
    assert_eq!(json(prior).await["replayed"], true);
    let another_key = body.replace("\"place\"", "\"different-effect\"");
    assert_eq!(
        send(&app, Some(&cookie), Some(&csrf), &another_key)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let changed = body.replace("Synthetic Buyer", "Different Buyer");
    assert_eq!(
        send(&app, Some(&cookie), Some(&csrf), &changed)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    time.0.store(2000, Ordering::SeqCst);
    let retry = send(&app, Some(&cookie), Some(&csrf), &body).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(json(retry).await["replayed"], true);
    assert_eq!(
        counts(&pool).await,
        (
            before.0 + 1,
            before.1 + 1,
            before.2 + 1,
            before.3 + 1,
            before.4
        )
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn requires_owner_origin_csrf_and_closed_bounded_objects(pool: PgPool) {
    let (app, _, cookie, csrf, quote) = ready(&pool).await;
    let before = counts(&pool).await;
    let body = payload(&quote);
    let (other, other_csrf) = issue(&app, None).await;
    let other_update = update("other-basket", 0, 3).replace("SET-A", "SET-FILLED");
    assert_eq!(
        app.clone()
            .oneshot(request(
                "PUT",
                "/api/guest/basket",
                Some(&other),
                Some(&other_csrf),
                &other_update
            ))
            .await
            .expect("other basket")
            .status(),
        StatusCode::OK
    );
    let (disabled, _) = guest_support::app(&pool);
    assert_eq!(
        send(&disabled, Some(&cookie), Some(&csrf), &body)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for (owner, proof, status) in [
        (None, Some(csrf.as_str()), StatusCode::UNAUTHORIZED),
        (Some(cookie.as_str()), None, StatusCode::FORBIDDEN),
        (
            Some(cookie.as_str()),
            Some(other_csrf.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(other.as_str()),
            Some(other_csrf.as_str()),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(send(&app, owner, proof, &body).await.status(), status);
    }
    let mut wrong = request("POST", URL, Some(&cookie), Some(&csrf), &body);
    wrong
        .headers_mut()
        .insert("origin", "https://bad.example".parse().expect("origin"));
    assert_eq!(
        app.clone().oneshot(wrong).await.expect("origin").status(),
        StatusCode::FORBIDDEN
    );
    let mut array_buyer: serde_json::Value = serde_json::from_str(&body).expect("body");
    array_buyer["buyer"] =
        serde_json::json!([{"name":"Synthetic Buyer","email":"buyer@example.test"}]);
    let mut array_address: serde_json::Value = serde_json::from_str(&body).expect("body");
    array_address["shipping_address"] = serde_json::json!([order_fixture::address()]);
    for bad in [
        "[]".to_owned(),
        array_buyer.to_string(),
        array_address.to_string(),
        body.replacen("{", "{\"effect_key\":\"second\",", 1),
        body.replace("Synthetic Buyer", "Synthetic Buyer\",\"name\":\"duplicate"),
        body.replace("\"buyer\":{", "\"buyer\":[{").replace(
            "\"email\":\"buyer@example.test\"}",
            "\"email\":\"buyer@example.test\"}]",
        ),
        body.replacen("{", "{\"seller\":{},", 1),
        body.replacen("{", "{\"total\":1,", 1),
    ] {
        assert_eq!(
            send(&app, Some(&cookie), Some(&csrf), &bad).await.status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(
        send(&app, Some(&cookie), Some(&csrf), &" ".repeat(4097))
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut duplicate = request("POST", URL, Some(&cookie), Some(&csrf), &body);
    duplicate
        .headers_mut()
        .append("x-csrf-token", csrf.parse().expect("header"));
    assert_eq!(
        app.clone()
            .oneshot(duplicate)
            .await
            .expect("duplicate")
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut plain = request("POST", URL, Some(&cookie), Some(&csrf), &body);
    plain
        .headers_mut()
        .insert("content-type", "text/plain".parse().expect("header"));
    assert_eq!(
        app.clone()
            .oneshot(plain)
            .await
            .expect("content type")
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let query = request(
        "POST",
        &format!("{URL}?x=1"),
        Some(&cookie),
        Some(&csrf),
        &body,
    );
    assert_eq!(
        app.clone().oneshot(query).await.expect("query").status(),
        StatusCode::BAD_REQUEST
    );
    let mut unauthenticated = request("POST", URL, Some(&cookie), Some(&csrf), &body);
    unauthenticated.headers_mut().remove("authorization");
    assert_eq!(
        app.clone()
            .oneshot(unauthenticated)
            .await
            .expect("preview")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(counts(&pool).await, before);
    let revoked = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/api/guest/session",
            Some(&cookie),
            Some(&csrf),
            "",
        ))
        .await
        .expect("revoke");
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        send(&app, Some(&cookie), Some(&csrf), &body).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(counts(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn terms_quote_expiry_and_storage_failure_fail_closed(pool: PgPool) {
    let (app, time, cookie, csrf, quote) = ready(&pool).await;
    let before = counts(&pool).await;
    let body = payload(&quote);
    assert_eq!(
        send(
            &app,
            Some(&cookie),
            Some(&csrf),
            &body.replace("synthetic-terms-v1", "wrong-terms")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            Some(&cookie),
            Some(&csrf),
            &body.replace("\"confirmed\":true", "\"confirmed\":false")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    time.0.store(1400, Ordering::SeqCst);
    assert_eq!(
        send(&app, Some(&cookie), Some(&csrf), &body).await.status(),
        StatusCode::GONE
    );
    time.0.store(1100, Ordering::SeqCst);
    pool.execute("CREATE FUNCTION test_order_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'private synthetic detail'; END $$; CREATE TRIGGER test_order_fault BEFORE INSERT ON checkout_orders FOR EACH ROW EXECUTE FUNCTION test_order_fault()").await.expect("fault");
    let result = send(&app, Some(&cookie), Some(&csrf), &body).await;
    assert_eq!(result.status(), StatusCode::SERVICE_UNAVAILABLE);
    let text = json(result).await.to_string();
    assert!(!text.contains("private synthetic detail"));
    assert!(!text.contains("buyer"));
    assert_eq!(counts(&pool).await, before);
    pool.execute("DROP FUNCTION test_order_fault() CASCADE")
        .await
        .expect("remove fault");
    assert_eq!(
        send(&app, Some(&cookie), Some(&csrf), &body).await.status(),
        StatusCode::OK
    );
}

#[derive(Clone)]
struct LogSink(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("lock"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn trace_and_errors_do_not_log_order_credentials_or_private_body(pool: PgPool) {
    use tracing::instrument::WithSubscriber;
    let sink = LogSink(Arc::new(std::sync::Mutex::new(vec![])));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(sink.clone())
        .finish();
    let (cookie, csrf) = async {
        tracing::warn!(code = "test_order_log_positive_control");
        let (app, _, cookie, csrf, quote) = ready(&pool).await;
        let body = payload(&quote)
            .replace("Synthetic Buyer", "private-name-marker")
            .replace("buyer@example.test", "private-email-marker@example.test");
        assert_eq!(
            send(&app, Some(&cookie), Some(&csrf), &body).await.status(),
            StatusCode::OK
        );
        pool.close().await;
        assert_eq!(
            send(&app, Some(&cookie), Some(&csrf), &body).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        (cookie, csrf)
    }
    .with_subscriber(subscriber)
    .await;
    let log = String::from_utf8(sink.0.lock().expect("logs").clone()).expect("utf8");
    assert!(log.contains("test_order_log_positive_control"));
    for value in [
        cookie.split('=').nth(1).expect("cookie"),
        &csrf,
        TOKEN,
        "private-name-marker",
        "private-email-marker",
        "1234 AB",
        "synthetic-tax-id-not-real",
    ] {
        assert!(!log.contains(value), "sensitive test marker leaked");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn truncated_body_does_not_create_a_partial_order(pool: PgPool) {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpStream},
        time::Duration,
    };
    let (app, _, cookie, csrf, quote) = ready(&pool).await;
    let before = counts(&pool).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("read deadline");
        stream.set_write_timeout(Some(Duration::from_secs(3))).expect("write deadline");
        let body = payload(&quote);
        write!(stream, "POST {URL} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nOrigin: {ORIGIN}\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()+1).expect("partial body");
        stream.shutdown(Shutdown::Write).expect("early EOF");
        let mut result = String::new(); stream.take(4096).read_to_string(&mut result).expect("response");
        assert!(!result.contains(&cookie)); assert!(!result.contains(&csrf)); assert!(!result.contains("buyer@example.test"));
        result
    }).await.expect("transport");
    server.abort();
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(response.contains("invalid_order_request"));
    assert_eq!(counts(&pool).await, before);
}
