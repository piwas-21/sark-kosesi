//! Isolated real guest-order custody and the test-only account history boundary.
use super::*;
use commerce_application::Locale;
use commerce_infrastructure::{
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    guest_baskets::{GuestCredential, GuestSessionStore},
    guest_order_links,
};

#[allow(dead_code)]
#[path = "../../../infrastructure/tests/support/checkout_order_fixture.rs"]
mod order_fixture;
#[allow(dead_code)]
#[path = "../../../infrastructure/tests/support/checkout_quote_fixture.rs"]
mod quote_fixture;

const URL: &str = "/api/test/customer/linked-orders";
fn history_app(pool: &PgPool, clock: Arc<Time>) -> axum::Router {
    let mut config = enabled();
    config.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    config.insert("TEST_LINKED_ORDERS_TRANSPORT".into(), "1".into());
    router_with_test_login(
        state(pool, clock),
        None,
        None,
        None,
        Config::parse(&config).expect("test config").test_login,
    )
    .expect("router")
}
fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("synthetic request")
}
async fn response(app: &axum::Router, req: Request<Body>, status: StatusCode) -> Vec<u8> {
    let result = app.clone().oneshot(req).await.expect("response");
    assert_eq!(result.status(), status);
    assert_private_headers(&result);
    assert!(!result.headers().contains_key("access-control-allow-origin"));
    to_bytes(result.into_body(), 262144)
        .await
        .expect("body")
        .to_vec()
}
async fn link_order(
    store: &GuestSessionStore,
    pool: &PgPool,
    key: &GuestCredential,
    csrf: &str,
    token: &CustomerSessionToken,
    time: &quote_fixture::Time,
    suffix: &str,
) -> String {
    let quote = issue_quote(
        store,
        key,
        csrf,
        Locale::Nl,
        &quote_fixture::request(&format!("quote-{suffix}")),
    )
    .await
    .expect("quote")
    .snapshot;
    let mut intent = order_fixture::intent(&quote.binding.quote_id);
    intent.buyer.email = EMAIL.into();
    let order = create_order(
        store,
        key,
        csrf,
        &format!("place-{suffix}"),
        &intent,
        &order_fixture::seller(),
    )
    .await
    .expect("order")
    .snapshot
    .identity
    .id;
    let proof = guest_order_links::issue(store, key, csrf, &order)
        .await
        .expect("proof");
    guest_order_links::link(pool, token, &order, &proof, time)
        .await
        .expect("link");
    order
}
async fn rows(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM guest_order_link_proofs),(SELECT count(*) FROM customer_order_links),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM commerce_outbox)")
        .fetch_one(pool).await.expect("custody counts")
}

#[test]
fn history_requires_separate_explicit_test_configuration() {
    let mut config = enabled();
    config.insert("TEST_LINKED_ORDERS_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&config).is_ok());
    for mode in ["local", "staging", "production"] {
        let mut value = config.clone();
        value.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&value).is_err());
    }
    for bad in ["", "0", "true", "2"] {
        let mut value = config.clone();
        value.insert("TEST_LINKED_ORDERS_TRANSPORT".into(), bad.into());
        assert!(Config::parse(&value).is_err());
    }
    for key in ["TEST_LOGIN_ORIGIN", "TEST_LOGIN_ADMISSION_KEY"] {
        let mut value = config.clone();
        value.remove(key);
        assert!(Config::parse(&value).is_err());
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_pages_linked_orders_with_same_time_ties_and_no_private_data(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    quote_fixture::seed(&pool).await;
    let api_time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &api_time).await;
    let app = history_app(&pool, api_time.clone());
    let (cookie, csrf, token) = bound_issue(&app).await;
    let other_email = "other@example.test";
    let other_proof = email_verification::issue(&pool, other_email, api_time.as_ref())
        .await
        .expect("other proof");
    verified_customer_accounts::establish(&pool, other_email, &other_proof, api_time.as_ref())
        .await
        .expect("other account");
    let other_proof = customer_passwords::issue(&pool, other_email, api_time.as_ref())
        .await
        .expect("other password proof");
    customer_passwords::replace(
        &pool,
        other_email,
        &other_proof,
        PASSWORD,
        api_time.as_ref(),
    )
    .await
    .expect("other password");
    let login = app
        .clone()
        .oneshot(request(super::body(other_email, PASSWORD)))
        .await
        .expect("other login");
    assert_eq!(login.status(), StatusCode::NO_CONTENT);
    let other_cookie = login
        .headers()
        .get(SET_COOKIE)
        .expect("cookie")
        .to_str()
        .expect("cookie header")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();
    api_time.0.store(1100, Ordering::SeqCst);
    let (store, time) = quote_fixture::store(&pool, 1100);
    let (guest, guest_csrf) = quote_fixture::guest(&store).await;
    let key = GuestCredential::from_session(&guest).expect("guest");
    let mut expected = Vec::new();
    for index in 0..9 {
        expected.push(
            link_order(
                &store,
                &pool,
                &key,
                &guest_csrf,
                &token,
                &time,
                &format!("owner-{index}"),
            )
            .await,
        );
    }
    expected.sort_by(|a, b| b.cmp(a));
    let before = rows(&pool).await;
    let body = response(&app, get(URL, &cookie), StatusCode::OK).await;
    let text = std::str::from_utf8(&body).expect("UTF-8");
    for secret in [
        EMAIL,
        "Synthetic Buyer",
        "1234 AB",
        cookie.as_str(),
        csrf.as_str(),
    ] {
        assert!(!text.contains(secret));
    }
    let first: serde_json::Value = serde_json::from_slice(&body).expect("page");
    assert_eq!(first["has_more"], true);
    let orders = first["orders"].as_array().expect("orders");
    assert_eq!(orders.len(), 8);
    let ids: Vec<&str> = orders
        .iter()
        .map(|v| v["order_id"].as_str().expect("ID"))
        .collect();
    assert_eq!(
        ids,
        expected[..8].iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(orders[0]["created_at"], "1100");
    assert_eq!(orders[0]["linked_at"], "1100");
    assert_eq!(orders[0]["currency"], "EUR");
    assert_eq!(
        orders[0]["lines"],
        serde_json::json!([{"sku":"SET-FILLED","title":"Isolated acceptance sample","quantity":3,"amounts":{"gross_minor":"37035","net_minor":"30607","vat_minor":"6428"}}])
    );
    assert_eq!(
        orders[0]["shipping"],
        serde_json::json!({"gross_minor":"1210","net_minor":"1000","vat_minor":"210"})
    );
    assert_eq!(
        orders[0]["total"],
        serde_json::json!({"gross_minor":"38245","net_minor":"31607","vat_minor":"6638"})
    );
    let path = format!("{URL}?after_linked_at=1100&after_order_id={}", expected[7]);
    let last: serde_json::Value =
        serde_json::from_slice(&response(&app, get(&path, &cookie), StatusCode::OK).await)
            .expect("last page");
    assert_eq!(last["has_more"], false);
    assert_eq!(last["orders"].as_array().expect("orders").len(), 1);
    assert_eq!(last["orders"][0]["order_id"], expected[8]);
    let end = format!("{URL}?after_order_id={}&after_linked_at=1100", expected[8]);
    let empty: serde_json::Value =
        serde_json::from_slice(&response(&app, get(&end, &cookie), StatusCode::OK).await)
            .expect("empty page");
    assert_eq!(empty, serde_json::json!({"orders":[],"has_more":false}));
    let other: serde_json::Value =
        serde_json::from_slice(&response(&app, get(&path, &other_cookie), StatusCode::OK).await)
            .expect("other page");
    assert_eq!(other, serde_json::json!({"orders":[],"has_more":false}));
    assert_eq!(rows(&pool).await, before);
    // Isolated synthetic corruption of one reciprocal proof must fail the page closed.
    let mut tx = pool.begin().await.expect("corruption transaction");
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable guards");
    sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=consumed_at+1 WHERE order_id=$1")
        .bind(&expected[0])
        .execute(&mut *tx)
        .await
        .expect("damage proof");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore guards");
    tx.commit().await.expect("commit fixture fault");
    let denied = response(&app, get(URL, &cookie), StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(denied, br#"{"code":"linked_orders_unavailable"}"#);
    let mut tx = pool.begin().await.expect("repair transaction");
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable guards");
    sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=consumed_at-1 WHERE order_id=$1")
        .bind(&expected[0])
        .execute(&mut *tx)
        .await
        .expect("repair proof");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore guards");
    tx.commit().await.expect("commit repair");
    response(&app, get(URL, &cookie), StatusCode::OK).await;
    response(&app, bound_logout(&cookie, &csrf), StatusCode::NO_CONTENT).await;
    response(&app, get(&path, &cookie), StatusCode::UNAUTHORIZED).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn endpoint_rejects_bad_authority_and_canonical_query_without_leaking(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let app = history_app(&pool, time.clone());
    let (cookie, _, _) = bound_issue(&app).await;
    let disabled = bound_app(&pool, time);
    response(&disabled, get(URL, &cookie), StatusCode::NOT_FOUND).await;
    for query in [
        "?",
        "?after_linked_at=1",
        "?after_order_id=00000000000000000000000000000000",
        "?after_linked_at=01&after_order_id=00000000000000000000000000000000",
        "?after_linked_at=9007199254740992&after_order_id=00000000000000000000000000000000",
        "?after_linked_at=1&after_order_id=00000000000000000000000000000000&extra=1",
        "?after_linked_at=1&after_linked_at=1&after_order_id=00000000000000000000000000000000",
        "?after_linked_at=%31&after_order_id=00000000000000000000000000000000",
    ] {
        response(
            &app,
            get(&format!("{URL}{query}"), &cookie),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let mut req = get(URL, &cookie);
    req.headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    response(&app, req, StatusCode::FORBIDDEN).await;
    let mut req = get(URL, &cookie);
    req.headers_mut().remove("authorization");
    response(&app, req, StatusCode::UNAUTHORIZED).await;
    let mut req = get(URL, &cookie);
    req.headers_mut().remove("cookie");
    response(&app, req, StatusCode::UNAUTHORIZED).await;
    let mut req = get(URL, &cookie);
    *req.method_mut() = axum::http::Method::HEAD;
    response(&app, req, StatusCode::METHOD_NOT_ALLOWED).await;
    let mut req = get(URL, &cookie);
    *req.body_mut() = Body::from("ignored");
    response(&app, req, StatusCode::BAD_REQUEST).await;
}
