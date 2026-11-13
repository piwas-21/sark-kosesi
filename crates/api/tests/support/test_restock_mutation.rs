//! Test-only bound-session subscription changes through the HTTP boundary.
use super::*;

#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}
const URL: &str = "/api/test/customer/restock/nl/SET-FILLED";

fn mutation_app(pool: &PgPool, time: Arc<Time>) -> axum::Router {
    let mut config = enabled();
    config.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    config.insert("TEST_RESTOCK_MUTATION_TRANSPORT".into(), "1".into());
    router_with_test_login(
        state(pool, time),
        None,
        None,
        None,
        Config::parse(&config)
            .expect("test configuration")
            .test_login,
    )
    .expect("test router")
}
fn mutate(path: &str, cookie: &str, proof: &str, value: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(path)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("cookie", cookie)
        .header("x-csrf-token", proof)
        .header("content-type", "application/json")
        .body(Body::from(value.to_owned()))
        .expect("synthetic request")
}
async fn response(app: &axum::Router, request: Request<Body>, status: StatusCode) -> Vec<u8> {
    let response = app.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), status);
    assert_private_headers(&response);
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    to_bytes(response.into_body(), 4096)
        .await
        .expect("body")
        .to_vec()
}
fn payload(desired: bool, revision: &str) -> String {
    format!(r#"{{"subscribed":{desired},"expected_revision":"{revision}"}}"#)
}
async fn setup(pool: &PgPool) -> (axum::Router, String, String, String) {
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(pool, &time).await;
    catalogue::approve(pool).await;
    catalogue::record_publication(
        pool,
        &catalogue::author(),
        &Time(AtomicU64::new(1100)),
        catalogue::request("activate", &catalogue::choice()),
    )
    .await
    .expect("publication");
    let app = mutation_app(pool, time.clone());
    let (cookie, proof, _) = bound_issue(&app).await;
    let (_, other_proof, _) = bound_issue(&app).await;
    time.0.store(1100, Ordering::SeqCst);
    (app, cookie, proof, other_proof)
}
async fn effects(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM stock_movements),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM email_verification_delivery_envelopes)")
        .fetch_one(pool).await.expect("side effects")
}

async fn second_customer(pool: &PgPool, app: &axum::Router) -> (String, String) {
    let time = Time(AtomicU64::new(1100));
    let email = "other@example.test";
    let proof = email_verification::issue(pool, email, &time)
        .await
        .expect("proof");
    verified_customer_accounts::establish(pool, email, &proof, &time)
        .await
        .expect("account");
    let proof = customer_passwords::issue(pool, email, &time)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, email, &proof, PASSWORD, &time)
        .await
        .expect("password");
    let result = app
        .clone()
        .oneshot(bound_login(super::body(email, PASSWORD)))
        .await
        .expect("login");
    assert_eq!(result.status(), StatusCode::OK);
    let cookie = result
        .headers()
        .get(SET_COOKIE)
        .expect("cookie")
        .to_str()
        .expect("header")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned();
    let body = to_bytes(result.into_body(), 4096)
        .await
        .expect("proof body");
    let proof: serde_json::Value = serde_json::from_slice(&body).expect("proof JSON");
    (
        cookie,
        proof["csrf_token"].as_str().expect("proof").to_owned(),
    )
}

#[test]
fn mutation_requires_independent_explicit_test_configuration() {
    let mut config = enabled();
    config.insert("TEST_RESTOCK_MUTATION_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&config).is_ok());
    for mode in ["local", "staging", "production"] {
        let mut changed = config.clone();
        changed.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&changed).is_err());
    }
    for value in ["", "0", "true", "2"] {
        let mut changed = config.clone();
        changed.insert("TEST_RESTOCK_MUTATION_TRANSPORT".into(), value.into());
        assert!(Config::parse(&changed).is_err());
    }
    for key in ["TEST_LOGIN_ORIGIN", "TEST_LOGIN_ADMISSION_KEY"] {
        let mut changed = config.clone();
        changed.remove(key);
        assert!(Config::parse(&changed).is_err());
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn subscription_retry_hidden_cancel_and_stale_replay_are_owned(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let _fixture = catalogue::media_seed(&pool).await;
    let (app, cookie, proof, _) = setup(&pool).await;
    let before = effects(&pool).await;
    let value = payload(true, "0");
    let (first, retry) = tokio::join!(
        response(&app, mutate(URL, &cookie, &proof, &value), StatusCode::OK),
        response(&app, mutate(URL, &cookie, &proof, &value), StatusCode::OK)
    );
    assert_eq!(first, br#"{"revision":"1"}"#);
    assert_eq!(retry, first);
    let row: (i64, i64, i64) = sqlx::query_as("SELECT count(*),COALESCE(min(generation),0),(SELECT max(revision) FROM customer_restock_buckets) FROM customer_restock_subscriptions WHERE sku='SET-FILLED'")
        .fetch_one(&pool).await.expect("single opt in");
    assert_eq!(row, (1, 1, 1));
    assert_eq!(effects(&pool).await, before);
    let mut withdrawn = catalogue::request("withdraw", &catalogue::PublicationChoice::Withdraw {});
    withdrawn.expected_revision = 1;
    catalogue::record_publication(
        &pool,
        &catalogue::author(),
        &Time(AtomicU64::new(1100)),
        withdrawn,
    )
    .await
    .expect("withdraw");
    let before = effects(&pool).await;
    let result = response(
        &app,
        mutate(URL, &cookie, &proof, &payload(false, "1")),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result, br#"{"revision":"2"}"#);
    assert_eq!(effects(&pool).await, before);
    let choice = catalogue::choice();
    let mut republished = catalogue::request("republish", &choice);
    republished.expected_revision = 2;
    catalogue::record_publication(
        &pool,
        &catalogue::author(),
        &Time(AtomicU64::new(1100)),
        republished,
    )
    .await
    .expect("republish");
    let before = effects(&pool).await;
    let result = response(
        &app,
        mutate(URL, &cookie, &proof, &payload(true, "2")),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result, br#"{"revision":"3"}"#);
    response(
        &app,
        mutate(URL, &cookie, &proof, &payload(false, "1")),
        StatusCode::CONFLICT,
    )
    .await;
    let generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM customer_restock_subscriptions WHERE sku='SET-FILLED'",
    )
    .fetch_one(&pool)
    .await
    .expect("new generation retained");
    assert_eq!(generation, 3);
    assert_eq!(effects(&pool).await, before);
    response(&app, bound_logout(&cookie, &proof), StatusCode::NO_CONTENT).await;
    response(
        &app,
        mutate(URL, &cookie, &proof, &payload(false, "3")),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rejects_bad_authority_aliases_and_noncanonical_bodies(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let _fixture = catalogue::media_seed(&pool).await;
    let (app, cookie, proof, other_proof) = setup(&pool).await;
    let body = payload(true, "0");
    let disabled = bound_app(&pool, Arc::new(Time(AtomicU64::new(1100))));
    response(
        &disabled,
        mutate(URL, &cookie, &proof, &body),
        StatusCode::NOT_FOUND,
    )
    .await;
    response(
        &app,
        mutate(URL, &cookie, &other_proof, &body),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let login = app
        .clone()
        .oneshot(request(super::body(EMAIL, PASSWORD)))
        .await
        .expect("bearer-only login");
    assert_eq!(login.status(), StatusCode::NO_CONTENT);
    let bearer_cookie = login
        .headers()
        .get(SET_COOKIE)
        .expect("cookie")
        .to_str()
        .expect("cookie header")
        .split(';')
        .next()
        .expect("cookie pair");
    response(
        &app,
        mutate(URL, bearer_cookie, &proof, &body),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    for path in [
        "/api/test/customer/restock/nl/%53ET-FILLED",
        "/api/test/customer/restock/NL/SET-FILLED",
        "/api/test/customer/restock/nl/SET-FILLED?x=1",
    ] {
        response(
            &app,
            mutate(path, &cookie, &proof, &body),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    for bad in [
        r#"{"subscribed":true,"expected_revision":"00"}"#,
        r#"{"subscribed":true,"expected_revision":"-1"}"#,
        r#"{"subscribed":true,"expected_revision":"9223372036854775808"}"#,
        r#"{"subscribed":true,"expected_revision":"0","unknown":1}"#,
        r#"[true,"0"]"#,
    ] {
        response(
            &app,
            mutate(URL, &cookie, &proof, bad),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let mut req = mutate(URL, &cookie, &proof, &body);
    req.headers_mut()
        .append("x-csrf-token", proof.parse().expect("header"));
    response(&app, req, StatusCode::UNAUTHORIZED).await;
    let mut req = mutate(URL, &cookie, &proof, &body);
    req.headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    response(&app, req, StatusCode::FORBIDDEN).await;
    let mut req = mutate(URL, &cookie, &proof, &body);
    req.headers_mut().remove("authorization");
    response(&app, req, StatusCode::UNAUTHORIZED).await;
    let mut req = mutate(URL, &cookie, &proof, &body);
    req.headers_mut().remove("x-csrf-token");
    response(&app, req, StatusCode::UNAUTHORIZED).await;
    let mut req = mutate(URL, &cookie, &proof, &body);
    req.headers_mut()
        .insert("content-type", "text/plain".parse().expect("header"));
    response(&app, req, StatusCode::UNSUPPORTED_MEDIA_TYPE).await;
    response(
        &app,
        mutate(URL, &cookie, &proof, &" ".repeat(257)),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    let mut req = mutate(URL, &cookie, &proof, &body);
    *req.method_mut() = axum::http::Method::HEAD;
    response(&app, req, StatusCode::METHOD_NOT_ALLOWED).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_restock_subscriptions")
        .fetch_one(&pool)
        .await
        .expect("no mutation");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn another_customer_cannot_remove_the_owners_subscription(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let _fixture = catalogue::media_seed(&pool).await;
    let (app, cookie, proof, _) = setup(&pool).await;
    let (other_cookie, other_proof) = second_customer(&pool, &app).await;
    response(
        &app,
        mutate(URL, &cookie, &proof, &payload(true, "0")),
        StatusCode::OK,
    )
    .await;
    let result = response(
        &app,
        mutate(URL, &other_cookie, &other_proof, &payload(false, "0")),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result, br#"{"revision":"0"}"#);
    let rows: (i64, i64) = sqlx::query_as("SELECT count(*),COALESCE(min(generation),0) FROM customer_restock_subscriptions WHERE sku='SET-FILLED'")
        .fetch_one(&pool).await.expect("owner row");
    assert_eq!(rows, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn storage_failure_is_redacted_and_has_no_partial_mutation(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let _fixture = catalogue::media_seed(&pool).await;
    let (app, cookie, proof, _) = setup(&pool).await;
    let before = effects(&pool).await;
    pool.execute("CREATE FUNCTION test_restock_mutation_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic storage detail'; END $$; CREATE TRIGGER test_restock_mutation_fault BEFORE INSERT ON customer_restock_buckets FOR EACH ROW EXECUTE FUNCTION test_restock_mutation_fault()")
        .await.expect("install isolated fault");
    let result = response(
        &app,
        mutate(URL, &cookie, &proof, &payload(true, "0")),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(result, br#"{"code":"restock_unavailable"}"#);
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_restock_buckets),(SELECT count(*) FROM customer_restock_subscriptions)")
        .fetch_one(&pool).await.expect("no partial state");
    assert_eq!(counts, (0, 0));
    assert_eq!(effects(&pool).await, before);
    pool.execute("DROP FUNCTION test_restock_mutation_fault() CASCADE")
        .await
        .expect("remove isolated fault");
    let result = response(
        &app,
        mutate(URL, &cookie, &proof, &payload(true, "0")),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result, br#"{"revision":"1"}"#);
    assert_eq!(effects(&pool).await, before);
}
