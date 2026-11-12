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
    customer_sessions::{self, CustomerSessionToken, SessionError},
};
use sqlx::{Executor, PgPool};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
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
    assert!(
        cookies[0] != cookies[1],
        "separate logins must issue distinct cookies"
    );
    assert_eq!(counts(&pool).await, (2, 2));
    for token in &tokens {
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
    assert!(
        wrong == unknown,
        "wrong and unknown credentials must receive the same denial"
    );
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert!(
        wrong.1 == br#"{"code":"login_denied"}"#,
        "credential denial body must be generic"
    );
    for _ in 0..4 {
        assert!(
            response_parts(
                app.clone()
                    .oneshot(request(body(EMAIL, "incorrect password long")))
                    .await
                    .expect("denied")
            )
            .await
                == wrong,
            "repeated invalid credentials must receive the same denial"
        );
    }
    assert!(
        response_parts(
            app.oneshot(request(body(EMAIL, PASSWORD)))
                .await
                .expect("limited")
        )
        .await
            == wrong,
        "throttled credentials must receive the same denial"
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
    assert!(
        payload == br#"{"code":"dependency_timeout"}"#,
        "deadline body must be generic"
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
