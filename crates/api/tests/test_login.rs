//! Synthetic, isolated PostgreSQL tests for the opt-in login boundary.
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/admitted_proofs.rs"]
mod admitted_proofs;
use admitted_proofs::{customer_passwords, email_verification, verified_customer_accounts};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::SET_COOKIE},
};
use commerce_api::{AppState, config::Config, router, router_with_test_login};
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use commerce_infrastructure::{
    MIGRATOR,
    customer_sessions::{self, CustomerCsrfToken, CustomerSessionToken, SessionError},
};
use sqlx::{Executor, PgPool};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
use tokio::sync::{Mutex, Semaphore};
use tower::ServiceExt;
use tower_sessions::cookie::{Cookie, SameSite};

const PREVIEW: &str = "fixture_preview_key_12345678901234567890";
const ORIGIN: &str = "https://test.example.invalid";
const PATH: &str = "/api/test/customer/login";
const EMAIL: &str = "owner@example.test";
const PASSWORD: &str = "correct horse battery staple";
static LOGIN_CASE: Mutex<()> = Mutex::const_new(());
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
// The request, quota custody and issuance each obtain independent clock samples.
// This fixture fails the first sample after the private session has committed.
struct FailAfterIssuance(AtomicUsize);
impl Clock for FailAfterIssuance {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.0.fetch_add(1, Ordering::SeqCst) < 7 {
            Ok(1000)
        } else {
            Err(ContextError::ClockUnavailable)
        }
    }
}
struct Ids;
impl Identifiers for Ids {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Ok([1; 16])
    }
}
fn values() -> HashMap<String, String> {
    [
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgresql://localhost/isolated"),
        ("PREVIEW_TOKEN", PREVIEW),
        ("API_BIND", "127.0.0.1:4567"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn enabled() -> HashMap<String, String> {
    let mut values = values();
    values.insert("TEST_LOGIN_ORIGIN".into(), ORIGIN.into());
    values.insert("TEST_LOGIN_ADMISSION_KEY".into(), "08".repeat(32));
    values
}
fn state(pool: &PgPool, clock: Arc<Time>) -> AppState {
    AppState {
        pool: pool.clone(),
        preview_token: Arc::from(PREVIEW),
        capacity: Arc::new(Semaphore::new(32)),
        clock,
        identifiers: Arc::new(Ids),
    }
}
fn app(pool: &PgPool, clock: Arc<Time>) -> axum::Router {
    router_with_test_login(
        state(pool, clock),
        None,
        None,
        None,
        Config::parse(&enabled()).expect("config").test_login,
    )
    .expect("router")
}
fn request(body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(PATH)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(body.into())
        .expect("request")
}
fn body(email: &str, password: &str) -> String {
    serde_json::json!({"email":email,"password":password}).to_string()
}
async fn account(pool: &PgPool, time: &Time) {
    let proof = email_verification::issue(pool, EMAIL, time)
        .await
        .expect("verification");
    verified_customer_accounts::establish(pool, EMAIL, &proof, time)
        .await
        .expect("account");
    let proof = customer_passwords::issue(pool, EMAIL, time)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, EMAIL, &proof, PASSWORD, time)
        .await
        .expect("password");
}
async fn counts(pool: &PgPool) -> (i32, i64) {
    sqlx::query_as("SELECT (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=1),(SELECT count(*) FROM customer_sessions)").fetch_one(pool).await.expect("admission and sessions")
}
fn assert_private_headers(response: &axum::response::Response) {
    assert!(
        response
            .headers()
            .get("cache-control")
            .is_some_and(|v| v == "private, no-store"),
        "private cache policy required"
    );
    assert!(
        response
            .headers()
            .get("referrer-policy")
            .is_some_and(|v| v == "no-referrer"),
        "referrer policy required"
    );
}
fn assert_distinct_cookies(first: &str, second: &str) {
    assert!(
        first != second,
        "separate logins must issue distinct cookies"
    );
}
fn assert_same_denial(
    actual: &(StatusCode, Vec<u8>),
    expected: &(StatusCode, Vec<u8>),
    message: &str,
) {
    assert!(actual == expected, "{message}");
}
fn assert_body(actual: &[u8], expected: &[u8], message: &str) {
    assert!(actual == expected, "{message}");
}
async fn response_parts(response: axum::response::Response) -> (StatusCode, Vec<u8>) {
    assert_private_headers(&response);
    assert!(response.headers().get(SET_COOKIE).is_none());
    (
        response.status(),
        to_bytes(response.into_body(), 4096)
            .await
            .unwrap_or_else(|_| panic!("response body unavailable"))
            .to_vec(),
    )
}
fn bearer(header: &str) -> CustomerSessionToken {
    let cookie =
        Cookie::parse(header.to_owned()).unwrap_or_else(|_| panic!("invalid response cookie"));
    assert!(
        cookie.name() == "__Host-piwas-customer",
        "customer cookie name required"
    );
    assert_eq!(cookie.secure(), Some(true));
    assert_eq!(cookie.http_only(), Some(true));
    assert_eq!(cookie.same_site(), Some(SameSite::Lax));
    assert!(cookie.path() == Some("/"), "customer cookie path required");
    assert!(
        cookie.domain().is_none(),
        "customer cookie must be host-only"
    );
    assert_eq!(cookie.max_age(), None);
    assert_eq!(
        cookie
            .expires_datetime()
            .expect("fixed expiry")
            .unix_timestamp(),
        44_200
    );
    let text = cookie.value();
    assert_eq!(text.len(), 64);
    assert!(
        text.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let mut bytes = [0; 32];
    for (index, pair) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        bytes[index] = digit(pair[0]) * 16 + digit(pair[1]);
    }
    CustomerSessionToken::from_bytes(bytes)
}

#[test]
fn secret_adjacent_failure_diagnostics_do_not_render_synthetic_values() {
    let marker = "synthetic-login-secret-control-712093";
    let denial = (StatusCode::UNAUTHORIZED, b"denied".to_vec());
    let exposed = (StatusCode::UNAUTHORIZED, marker.as_bytes().to_vec());
    let response = |cache: &str, referrer: &str| {
        axum::response::Response::builder()
            .header("cache-control", cache)
            .header("referrer-policy", referrer)
            .body(Body::empty())
            .expect("synthetic response")
    };
    assert_private_headers(&response("private, no-store", "no-referrer"));
    assert_distinct_cookies(marker, "different");
    assert_same_denial(&denial, &denial, "same denial required");
    assert_body(b"denied", b"denied", "generic body required");
    let byte_rendering = format!("{:?}", marker.as_bytes());
    let failures = [
        std::panic::catch_unwind(|| assert_distinct_cookies(marker, marker)),
        std::panic::catch_unwind(|| {
            bearer(&format!("{marker}=value"));
        }),
        std::panic::catch_unwind(|| assert_private_headers(&response(marker, "no-referrer"))),
        std::panic::catch_unwind(|| assert_private_headers(&response("private, no-store", marker))),
        std::panic::catch_unwind(|| assert_same_denial(&exposed, &denial, "same denial required")),
        std::panic::catch_unwind(|| {
            assert_body(marker.as_bytes(), b"denied", "generic body required")
        }),
    ];
    for failure in failures {
        let payload = failure.expect_err("synthetic mismatch must fail");
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .expect("failure diagnostic");
        assert!(
            !message.contains(marker),
            "synthetic secret reached diagnostics"
        );
        assert!(
            !message.contains(&byte_rendering),
            "synthetic bytes reached diagnostics"
        );
    }
    // Positive controls prove the diagnostic checks detect both representations.
    let literal = std::panic::catch_unwind(|| assert_eq!(marker, "different"))
        .expect_err("literal control must fail");
    let bytes = std::panic::catch_unwind(|| assert_eq!(marker.as_bytes(), b"different"))
        .expect_err("byte control must fail");
    assert!(
        literal
            .downcast_ref::<String>()
            .is_some_and(|m| m.contains(marker))
    );
    assert!(
        bytes
            .downcast_ref::<String>()
            .is_some_and(|m| m.contains(&byte_rendering))
    );
}

#[test]
fn configuration_is_paired_test_only_and_uses_separate_keys() {
    assert!(Config::parse(&values()).expect("base").test_login.is_none());
    assert!(
        Config::parse(&enabled())
            .expect("enabled")
            .test_login
            .is_some()
    );
    for mode in ["local", "staging", "production"] {
        let mut v = enabled();
        v.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&v).is_err());
    }
    for missing in ["TEST_LOGIN_ORIGIN", "TEST_LOGIN_ADMISSION_KEY"] {
        let mut v = enabled();
        v.remove(missing);
        assert!(Config::parse(&v).is_err());
    }
    for bad in ["00".repeat(32), "AB".repeat(32), "aa".repeat(31)] {
        let mut v = enabled();
        v.insert("TEST_LOGIN_ADMISSION_KEY".into(), bad);
        assert!(Config::parse(&v).is_err());
    }
    let mut v = enabled();
    v.insert(
        "TEST_LOGIN_ORIGIN".into(),
        "http://test.example.invalid".into(),
    );
    assert!(Config::parse(&v).is_err());
    let mut v = enabled();
    v.insert("TEST_VERIFICATION_ORIGIN".into(), ORIGIN.into());
    v.insert("TEST_VERIFICATION_ADMISSION_KEY".into(), "08".repeat(32));
    assert!(Config::parse(&v).is_err());
    v.insert("TEST_VERIFICATION_ADMISSION_KEY".into(), "07".repeat(32));
    assert!(Config::parse(&v).is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn valid_login_sets_only_a_bound_expiring_cookie_and_replay_issues_a_distinct_session(
    pool: PgPool,
) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, time.as_ref()).await;
    let app = app(&pool, time.clone());
    let mut tokens = Vec::new();
    let mut cookies = Vec::new();
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request(body(EMAIL, PASSWORD)))
            .await
            .expect("login");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_private_headers(&response);
        let header = response.headers().get(SET_COOKIE).expect("one cookie");
        assert!(header.is_sensitive());
        assert_eq!(response.headers().get_all(SET_COOKIE).iter().count(), 1);
        let cookie = header
            .to_str()
            .unwrap_or_else(|_| panic!("invalid cookie encoding"))
            .to_owned();
        tokens.push(bearer(&cookie));
        cookies.push(cookie);
        assert!(
            to_bytes(response.into_body(), 4096)
                .await
                .unwrap_or_else(|_| panic!("response body unavailable"))
                .is_empty()
        );
    }
    assert_distinct_cookies(&cookies[0], &cookies[1]);
    assert_eq!(counts(&pool).await, (2, 2));
    let bindings: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_session_csrf")
        .fetch_one(&pool)
        .await
        .expect("count private synchronizers");
    assert_eq!(bindings, 0);
    let unrelated = CustomerCsrfToken::from_bytes([17; 32]);
    for token in &tokens {
        assert_eq!(
            customer_sessions::logout_with_csrf(&pool, token, &unrelated, time.as_ref()).await,
            Err(SessionError::Denied)
        );
        assert_eq!(
            customer_sessions::authenticate(&pool, token, time.as_ref())
                .await
                .expect("live session")
                .expires_at,
            44_200
        );
    }
    time.0.store(44_200, Ordering::SeqCst);
    for token in &tokens {
        assert!(matches!(
            customer_sessions::authenticate(&pool, token, time.as_ref()).await,
            Err(SessionError::Denied)
        ));
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_unknown_and_throttled_credentials_share_denial_without_cookie(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, time.as_ref()).await;
    let app = app(&pool, time);
    let wrong = response_parts(
        app.clone()
            .oneshot(request(body(EMAIL, "incorrect password long")))
            .await
            .expect("wrong"),
    )
    .await;
    let unknown = response_parts(
        app.clone()
            .oneshot(request(body("unknown@example.test", PASSWORD)))
            .await
            .expect("unknown"),
    )
    .await;
    assert_same_denial(
        &wrong,
        &unknown,
        "wrong and unknown credentials must receive the same denial",
    );
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert_body(
        &wrong.1,
        br#"{"code":"login_denied"}"#,
        "credential denial body must be generic",
    );
    for _ in 0..4 {
        assert_same_denial(
            &response_parts(
                app.clone()
                    .oneshot(request(body(EMAIL, "incorrect password long")))
                    .await
                    .expect("denied"),
            )
            .await,
            &wrong,
            "repeated invalid credentials must receive the same denial",
        );
    }
    assert_same_denial(
        &response_parts(
            app.oneshot(request(body(EMAIL, PASSWORD)))
                .await
                .expect("limited"),
        )
        .await,
        &wrong,
        "throttled credentials must receive the same denial",
    );
    assert_eq!(counts(&pool).await, (6, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn origin_auth_closed_body_and_storage_fail_without_authority(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, time.as_ref()).await;
    let app = app(&pool, time.clone());
    let disabled = router(state(&pool, time));
    assert_eq!(
        disabled
            .oneshot(request(body(EMAIL, PASSWORD)))
            .await
            .expect("disabled")
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut no_auth = request(body(EMAIL, PASSWORD));
    no_auth.headers_mut().remove("authorization");
    assert_eq!(
        response_parts(app.clone().oneshot(no_auth).await.expect("auth"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let mut no_origin = request(body(EMAIL, PASSWORD));
    no_origin.headers_mut().remove("origin");
    assert_eq!(
        response_parts(app.clone().oneshot(no_origin).await.expect("origin"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut duplicate = request(body(EMAIL, PASSWORD));
    duplicate
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    assert_eq!(
        response_parts(
            app.clone()
                .oneshot(duplicate)
                .await
                .expect("duplicate origin")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut query = request(body(EMAIL, PASSWORD));
    *query.uri_mut() = format!("{PATH}?password=secret").parse().expect("query");
    assert_eq!(
        response_parts(app.clone().oneshot(query).await.expect("query"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut media = request(body(EMAIL, PASSWORD));
    media
        .headers_mut()
        .insert("content-type", "text/plain".parse().expect("media"));
    assert_eq!(
        response_parts(app.clone().oneshot(media).await.expect("media"))
            .await
            .0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    for invalid in [
        "[]".to_string(),
        format!("{{\"email\":\"{EMAIL}\",\"password\":\"{PASSWORD}\",\"extra\":1}}"),
        format!("{{\"email\":\"{EMAIL}\",\"email\":\"x\",\"password\":\"{PASSWORD}\"}}"),
    ] {
        let (status, _) = response_parts(
            app.clone()
                .oneshot(request(invalid))
                .await
                .expect("invalid"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        response_parts(
            app.clone()
                .oneshot(request("x".repeat(5000)))
                .await
                .expect("oversize")
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(counts(&pool).await, (0, 0));
    pool.execute(
        "ALTER TABLE customer_credential_admission_scopes RENAME TO unavailable_admission_scopes",
    )
    .await
    .expect("isolate unavailable admission");
    assert_eq!(
        response_parts(
            app.oneshot(request(body(EMAIL, PASSWORD)))
                .await
                .expect("unavailable")
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    pool.execute(
        "ALTER TABLE unavailable_admission_scopes RENAME TO customer_credential_admission_scopes",
    )
    .await
    .expect("restore");
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn password_change_invalidates_the_issued_customer_cookie(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, time.as_ref()).await;
    let response = app(&pool, time.clone())
        .oneshot(request(body(EMAIL, PASSWORD)))
        .await
        .expect("login");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let token = bearer(
        response.headers()[SET_COOKIE]
            .to_str()
            .unwrap_or_else(|_| panic!("invalid cookie encoding")),
    );
    assert!(
        customer_sessions::authenticate(&pool, &token, time.as_ref())
            .await
            .is_ok()
    );
    let proof = customer_passwords::issue(&pool, EMAIL, time.as_ref())
        .await
        .expect("change proof");
    customer_passwords::replace(
        &pool,
        EMAIL,
        &proof,
        "a different password phrase",
        time.as_ref(),
    )
    .await
    .expect("replacement");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &token, time.as_ref()).await,
        Err(SessionError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admission_wait_hits_shared_deadline_without_issuing_a_cookie(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, time.as_ref()).await;
    let app = app(&pool, time);
    let mut blocker = pool.begin().await.expect("quota blocker");
    sqlx::query(
        "SELECT purpose FROM customer_credential_admission_scopes WHERE purpose=1 FOR UPDATE",
    )
    .execute(&mut *blocker)
    .await
    .expect("hold login scope");
    let worker = tokio::spawn(app.clone().oneshot(request(body(EMAIL, PASSWORD))));
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%customer_credential_admission_scopes%FOR UPDATE%')").fetch_one(&pool).await.expect("inspect wait");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("actual admission lock wait");
    let response = worker
        .await
        .unwrap_or_else(|_| panic!("login worker failed"))
        .expect("response");
    let (status, payload) = response_parts(response).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_body(
        &payload,
        br#"{"code":"dependency_timeout"}"#,
        "deadline body must be generic",
    );
    blocker.rollback().await.expect("release admission");
    assert_eq!(counts(&pool).await, (0, 0));
    let response = app
        .oneshot(request(body(EMAIL, PASSWORD)))
        .await
        .expect("healthy retry");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.headers().contains_key(SET_COOKIE));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn truncated_transport_body_is_redacted_and_debits_nothing(pool: PgPool) {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpStream},
        time::Duration,
    };
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let router = app(&pool, time);
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("deadline");
        write!(stream, "POST {PATH} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {PREVIEW}\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n{}", body(EMAIL, PASSWORD)).expect("partial body");
        stream.shutdown(Shutdown::Write).expect("premature EOF");
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap_or_else(|_| panic!("bounded response unavailable"));
        response
    }).await.unwrap_or_else(|_| panic!("transport task failed"));
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(response.contains("invalid_login_request"));
    assert!(!response.contains(EMAIL));
    assert!(!response.contains(PASSWORD));
    assert!(!response.contains(PREVIEW));
    assert!(!response.to_ascii_lowercase().contains("set-cookie:"));
    assert_eq!(counts(&pool).await, (0, 0));
    server.abort();
}

const BOUND_LOGIN: &str = "/api/test/customer/login-with-csrf";
const BOUND_LOGOUT: &str = "/api/test/customer/session";

fn bound_app(pool: &PgPool, clock: Arc<Time>) -> axum::Router {
    let mut values = enabled();
    values.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    router_with_test_login(
        state(pool, clock),
        None,
        None,
        None,
        Config::parse(&values).expect("test config").test_login,
    )
    .expect("router")
}
fn bound_login(body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(BOUND_LOGIN)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(body.into())
        .expect("request")
}
fn bound_logout(cookie: &str, csrf: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(BOUND_LOGOUT)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("cookie", cookie)
        .header("x-csrf-token", csrf)
        .body(Body::empty())
        .expect("request")
}
async fn bound_issue(app: &axum::Router) -> (String, String, CustomerSessionToken) {
    let response = app
        .clone()
        .oneshot(bound_login(body(EMAIL, PASSWORD)))
        .await
        .expect("test login response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_private_headers(&response);
    let header = response
        .headers()
        .get(SET_COOKIE)
        .expect("one session cookie");
    assert!(header.is_sensitive());
    assert_eq!(response.headers().get_all(SET_COOKIE).iter().count(), 1);
    let header = header
        .to_str()
        .unwrap_or_else(|_| panic!("invalid cookie encoding"));
    let token = bearer(header);
    let cookie = header
        .split(';')
        .next()
        .unwrap_or_else(|| panic!("missing cookie pair"))
        .to_owned();
    let bytes = to_bytes(response.into_body(), 4096)
        .await
        .unwrap_or_else(|_| panic!("missing proof body"));
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| panic!("invalid proof body"));
    let map = value
        .as_object()
        .unwrap_or_else(|| panic!("proof must be an object"));
    assert_eq!(map.len(), 1);
    let proof = map
        .get("csrf_token")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("missing synchronizer"));
    assert!(
        proof.len() == 64
            && proof
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid synchronizer encoding"
    );
    (cookie, proof.to_owned(), token)
}

#[test]
fn bound_transport_requires_explicit_test_mode_and_complete_login_configuration() {
    let mut v = enabled();
    v.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&v).is_ok());
    for mode in ["local", "staging", "production"] {
        let mut v = v.clone();
        v.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&v).is_err());
    }
    for value in ["", "0", "true", "yes", "2"] {
        let mut v = v.clone();
        v.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), value.into());
        assert!(Config::parse(&v).is_err());
    }
    for key in ["TEST_LOGIN_ORIGIN", "TEST_LOGIN_ADMISSION_KEY"] {
        let mut v = v.clone();
        v.remove(key);
        assert!(Config::parse(&v).is_err());
    }
    let mut v = values();
    v.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&v).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_transport_requires_all_authority_and_replay_fails_closed(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let disabled = app(&pool, time.clone());
    let response = disabled
        .clone()
        .oneshot(bound_login(body(EMAIL, PASSWORD)))
        .await
        .expect("disabled route");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = disabled
        .oneshot(bound_logout("missing=1", "missing"))
        .await
        .expect("disabled logout");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let app = bound_app(&pool, time.clone());
    let before = counts(&pool).await;
    for invalid in ["[]", "{}", r#"{"email":"x","password":"y","extra":true}"#] {
        let response = app
            .clone()
            .oneshot(bound_login(invalid))
            .await
            .expect("closed request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let _ = response_parts(response).await;
    }
    let mut missing_login_origin = bound_login(body(EMAIL, PASSWORD));
    missing_login_origin.headers_mut().remove("origin");
    let mut duplicate_login_origin = bound_login(body(EMAIL, PASSWORD));
    duplicate_login_origin
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    let mut missing_login_auth = bound_login(body(EMAIL, PASSWORD));
    missing_login_auth.headers_mut().remove("authorization");
    for (request, expected) in [
        (missing_login_origin, StatusCode::FORBIDDEN),
        (duplicate_login_origin, StatusCode::FORBIDDEN),
        (missing_login_auth, StatusCode::UNAUTHORIZED),
    ] {
        let response = app.clone().oneshot(request).await.expect("login authority");
        assert_eq!(response.status(), expected);
        let _ = response_parts(response).await;
    }
    let mut form = bound_login(body(EMAIL, PASSWORD));
    form.headers_mut().insert(
        "content-type",
        "application/x-www-form-urlencoded".parse().expect("type"),
    );
    let response = app.clone().oneshot(form).await.expect("form rejection");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let _ = response_parts(response).await;
    assert_eq!(counts(&pool).await, before);
    let wrong = app
        .clone()
        .oneshot(bound_login(body(EMAIL, "incorrect password")))
        .await
        .expect("wrong credentials");
    let unknown = app
        .clone()
        .oneshot(bound_login(body("unknown@example.test", PASSWORD)))
        .await
        .expect("unknown identity");
    let wrong = response_parts(wrong).await;
    let unknown = response_parts(unknown).await;
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert_same_denial(&wrong, &unknown, "credential denials must be neutral");
    let (cookie_a, proof_a, token_a) = bound_issue(&app).await;
    let (cookie_b, proof_b, token_b) = bound_issue(&app).await;
    assert!(
        cookie_a != cookie_b && proof_a != proof_b,
        "independent authority required"
    );
    let legacy = app
        .clone()
        .oneshot(request(body(EMAIL, PASSWORD)))
        .await
        .expect("cookie-only login");
    assert_eq!(legacy.status(), StatusCode::NO_CONTENT);
    let legacy_header = legacy
        .headers()
        .get(SET_COOKIE)
        .expect("legacy cookie")
        .to_str()
        .unwrap_or_else(|_| panic!("legacy cookie encoding"));
    let legacy_token = bearer(legacy_header);
    let legacy_cookie = legacy_header
        .split(';')
        .next()
        .unwrap_or_else(|| panic!("legacy cookie pair"))
        .to_owned();
    let bindings: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_session_csrf")
        .fetch_one(&pool)
        .await
        .expect("binding count");
    assert_eq!(bindings, 2);
    let mut wrong_origin = bound_logout(&cookie_a, &proof_a);
    wrong_origin.headers_mut().insert(
        "origin",
        "https://other.example.invalid".parse().expect("origin"),
    );
    let mut missing_origin = bound_logout(&cookie_a, &proof_a);
    missing_origin.headers_mut().remove("origin");
    let mut duplicate_origin = bound_logout(&cookie_a, &proof_a);
    duplicate_origin
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    let mut missing_csrf = bound_logout(&cookie_a, &proof_a);
    missing_csrf.headers_mut().remove("x-csrf-token");
    let mut missing_cookie = bound_logout(&cookie_a, &proof_a);
    missing_cookie.headers_mut().remove("cookie");
    let mut no_auth = bound_logout(&cookie_a, &proof_a);
    no_auth.headers_mut().remove("authorization");
    let mut duplicate_csrf = bound_logout(&cookie_a, &proof_a);
    duplicate_csrf.headers_mut().append(
        "x-csrf-token",
        proof_a.parse().unwrap_or_else(|_| panic!("header")),
    );
    let duplicate_cookie = format!("{cookie_a}; {cookie_a}");
    let mut split_cookie = bound_logout(&cookie_a, &proof_a);
    split_cookie.headers_mut().append(
        "cookie",
        cookie_a.parse().unwrap_or_else(|_| panic!("header")),
    );
    let mut extra_body = bound_logout(&cookie_a, &proof_a);
    *extra_body.body_mut() = Body::from("x");
    let mut query = bound_logout(&cookie_a, &proof_a);
    *query.uri_mut() = format!("{BOUND_LOGOUT}?x=1").parse().expect("uri");
    for (request, expected) in [
        (bound_logout(&cookie_a, &proof_b), StatusCode::UNAUTHORIZED),
        (
            bound_logout(&legacy_cookie, &proof_a),
            StatusCode::UNAUTHORIZED,
        ),
        (bound_logout(&cookie_a, "invalid"), StatusCode::UNAUTHORIZED),
        (
            bound_logout(&duplicate_cookie, &proof_a),
            StatusCode::UNAUTHORIZED,
        ),
        (duplicate_csrf, StatusCode::UNAUTHORIZED),
        (split_cookie, StatusCode::UNAUTHORIZED),
        (wrong_origin, StatusCode::FORBIDDEN),
        (missing_origin, StatusCode::FORBIDDEN),
        (duplicate_origin, StatusCode::FORBIDDEN),
        (missing_csrf, StatusCode::UNAUTHORIZED),
        (missing_cookie, StatusCode::UNAUTHORIZED),
        (no_auth, StatusCode::UNAUTHORIZED),
        (extra_body, StatusCode::BAD_REQUEST),
        (query, StatusCode::BAD_REQUEST),
    ] {
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("negative response");
        assert_eq!(response.status(), expected);
        let _ = response_parts(response).await;
    }
    for token in [&token_a, &token_b, &legacy_token] {
        customer_sessions::authenticate(&pool, token, &*time)
            .await
            .expect("session remains live");
    }
    let response = app
        .clone()
        .oneshot(bound_logout(&format!("other=value; {cookie_a}"), &proof_a))
        .await
        .expect("logout");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_private_headers(&response);
    let header = response.headers().get(SET_COOKIE).expect("removal cookie");
    assert!(header.is_sensitive());
    let removed = Cookie::parse(
        header
            .to_str()
            .unwrap_or_else(|_| panic!("cookie header"))
            .to_owned(),
    )
    .unwrap_or_else(|_| panic!("removal cookie"));
    assert!(removed.name() == "__Host-piwas-customer" && removed.value().is_empty());
    assert_eq!(removed.secure(), Some(true));
    assert_eq!(removed.http_only(), Some(true));
    assert_eq!(removed.same_site(), Some(SameSite::Lax));
    assert!(removed.path() == Some("/") && removed.domain().is_none());
    assert_eq!(
        removed.max_age(),
        Some(tower_sessions::cookie::time::Duration::ZERO)
    );
    assert!(
        to_bytes(response.into_body(), 4096)
            .await
            .unwrap_or_else(|_| panic!("logout body"))
            .is_empty()
    );
    assert!(matches!(
        customer_sessions::authenticate(&pool, &token_a, &*time).await,
        Err(SessionError::Denied)
    ));
    customer_sessions::authenticate(&pool, &token_b, &*time)
        .await
        .expect("other remains live");
    // A lost success response may have committed; a duplicate denies and does not
    // emit a misleading second tombstone or claim an idempotent receipt.
    let response = app
        .clone()
        .oneshot(bound_logout(&cookie_a, &proof_a))
        .await
        .expect("duplicate");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let _ = response_parts(response).await;
    for (path, method) in [(BOUND_LOGIN, "POST"), (BOUND_LOGOUT, "DELETE")] {
        let preflight = Request::builder()
            .method("OPTIONS")
            .uri(path)
            .header("origin", ORIGIN)
            .header("access-control-request-method", method)
            .body(Body::empty())
            .expect("preflight");
        let response = app
            .clone()
            .oneshot(preflight)
            .await
            .expect("preflight response");
        assert!(response.status().is_client_error());
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none()
        );
        assert!(response.headers().get(SET_COOKIE).is_none());
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_session_rejects_rotation_expiry_and_competing_logout(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let app = bound_app(&pool, time.clone());
    let (cookie, proof, old) = bound_issue(&app).await;
    let successor = customer_sessions::rotate(&pool, &old, &*time)
        .await
        .expect("rotate");
    let mut successor_cookie = String::from("__Host-piwas-customer=");
    for byte in successor.token.expose_bytes() {
        use std::fmt::Write;
        write!(successor_cookie, "{byte:02x}").expect("encode synthetic token");
    }
    for cookie in [&cookie, &successor_cookie] {
        let response = app
            .clone()
            .oneshot(bound_logout(cookie, &proof))
            .await
            .expect("rotated response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let _ = response_parts(response).await;
    }
    customer_sessions::authenticate(&pool, &successor.token, &*time)
        .await
        .expect("unbound successor remains live");
    let (cookie, proof, token) = bound_issue(&app).await;
    let (one, two) = tokio::join!(
        app.clone().oneshot(bound_logout(&cookie, &proof)),
        app.clone().oneshot(bound_logout(&cookie, &proof))
    );
    let one = one.expect("first response");
    let two = two.expect("second response");
    assert_eq!(
        usize::from(one.headers().contains_key(SET_COOKIE))
            + usize::from(two.headers().contains_key(SET_COOKIE)),
        1
    );
    assert!(matches!(
        (one.status(), two.status()),
        (StatusCode::NO_CONTENT, StatusCode::UNAUTHORIZED)
            | (StatusCode::UNAUTHORIZED, StatusCode::NO_CONTENT)
    ));
    assert!(matches!(
        customer_sessions::authenticate(&pool, &token, &*time).await,
        Err(SessionError::Denied)
    ));
    let (cookie, proof, _) = bound_issue(&app).await;
    time.0.store(44_200, Ordering::SeqCst);
    let response = app
        .oneshot(bound_logout(&cookie, &proof))
        .await
        .expect("expired response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let _ = response_parts(response).await;
}

#[derive(Clone)]
struct SessionLogSink(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for SessionLogSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("fixture lock"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SessionLogSink {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_session_trace_logs_exclude_cookie_proof_password_and_body(pool: PgPool) {
    use tracing::instrument::WithSubscriber;
    let _case = LOGIN_CASE.lock().await;
    let sink = SessionLogSink(Arc::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(sink.clone())
        .finish();
    let (cookie, proof) = async {
        tracing::warn!(code = "session_log_positive_control");
        let time = Arc::new(Time(AtomicU64::new(1000)));
        account(&pool, &time).await;
        let app = bound_app(&pool, time);
        let (cookie, proof, _) = bound_issue(&app).await;
        let invalid = app.clone().oneshot(bound_logout(&cookie, &"ab".repeat(32))).await.expect("wrong proof");
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        let _ = response_parts(invalid).await;
        let invalid = app.clone().oneshot(bound_login(r#"{"email":"private-body-marker@example.test","password":"private-password-marker","extra":true}"#)).await.expect("invalid input");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let _ = response_parts(invalid).await;
        pool.close().await;
        let unavailable = app.oneshot(bound_logout(&cookie, &proof)).await.expect("database failure");
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        let _ = response_parts(unavailable).await;
        (cookie, proof)
    }.with_subscriber(subscriber).await;
    let log = String::from_utf8(sink.0.lock().expect("capture").clone()).expect("UTF8");
    assert!(
        log.contains("session_log_positive_control"),
        "capture positive control missing"
    );
    assert!(log.contains("request_unavailable"), "failure event missing");
    for secret in [
        cookie
            .split_once('=')
            .map(|(_, value)| value)
            .expect("cookie"),
        &proof,
        PREVIEW,
        EMAIL,
        PASSWORD,
        "private-body-marker",
        "private-password-marker",
        &"ab".repeat(32),
    ] {
        assert!(!log.contains(secret), "sensitive value leaked to logs");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_login_closed_storage_fails_without_cookie_or_identity(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let audit = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("independent audit pool");
    let app = bound_app(&pool, time);
    pool.close().await;
    let response = app
        .oneshot(bound_login(body(EMAIL, PASSWORD)))
        .await
        .expect("closed storage response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let (_, response_body) = response_parts(response).await;
    assert_body(
        &response_body,
        br#"{"code":"login_unavailable"}"#,
        "generic denial required",
    );
    assert_eq!(counts(&audit).await, (0, 0));
    let bound: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_session_csrf")
        .fetch_one(&audit)
        .await
        .expect("binding count");
    assert_eq!(bound, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_login_cookie_clock_failure_keeps_committed_unreturned_session(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let fixed = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &fixed).await;
    let mut app_state = state(&pool, fixed);
    app_state.clock = Arc::new(FailAfterIssuance(AtomicUsize::new(0)));
    let mut values = enabled();
    values.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    let app = router_with_test_login(
        app_state,
        None,
        None,
        None,
        Config::parse(&values).expect("test config").test_login,
    )
    .expect("router");
    let response = app
        .oneshot(bound_login(body(EMAIL, PASSWORD)))
        .await
        .expect("clock failure response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let (_, response_body) = response_parts(response).await;
    assert_body(
        &response_body,
        br#"{"code":"login_unavailable"}"#,
        "generic denial required",
    );
    assert_eq!(counts(&pool).await, (1, 1));
    let bound: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_session_csrf")
        .fetch_one(&pool)
        .await
        .expect("binding count");
    assert_eq!(bound, 1);
}

#[path = "support/test_favourites.rs"]
mod favourites;
