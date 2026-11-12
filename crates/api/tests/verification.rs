use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use commerce_api::{AppState, config::Config, router, router_with_verification};
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{
        self,
        delivery::{
            VerificationDeliveryKey, issue_for_delivery,
            opening::{OpenOutcome, claim_open_and_record},
            outcome::{DeliveryState, TransportOutcome, inspect},
            register_key,
        },
    },
};
use sqlx::PgPool;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Semaphore;
use tower::ServiceExt;
const PREVIEW: &str = "fixture_preview_key_12345678901234567890";
const ORIGIN: &str = "https://test.example.invalid";
const PATH: &str = "/api/test/verification/redeem";
struct Time;
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(1_700_000_000)
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
    let mut v = values();
    v.insert("TEST_VERIFICATION_ORIGIN".into(), ORIGIN.into());
    v.insert("TEST_VERIFICATION_ADMISSION_KEY".into(), "07".repeat(32));
    v
}
fn state(pool: &PgPool) -> AppState {
    AppState {
        pool: pool.clone(),
        preview_token: Arc::from(PREVIEW),
        capacity: Arc::new(Semaphore::new(32)),
        clock: Arc::new(Time),
        identifiers: Arc::new(Ids),
    }
}
fn app(pool: &PgPool) -> Router {
    router_with_verification(
        state(pool),
        None,
        None,
        Config::parse(&enabled()).expect("config").verification,
    )
    .expect("router")
}
fn request(body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(PATH)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request")
}
fn encode(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
async fn proof(pool: &PgPool, email: &str) -> String {
    let key = AdmissionKey::new(1, [7; 32]).expect("key");
    let token = email_verification::issue(
        pool,
        email,
        &Time,
        &key,
        AdmissionLimits::new(120, 5).expect("limits"),
    )
    .await
    .expect("issue");
    encode(token.expose_bytes())
}
fn body(email: &str, token: &str) -> String {
    serde_json::json!({"email":email,"token":token}).to_string()
}
async fn effects(pool: &PgPool) -> (i64, i64, i32) {
    sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(consumed_at) FROM email_verification_challenges), (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=3)").fetch_one(pool).await.expect("effects")
}
async fn checked(response: axum::response::Response) -> (StatusCode, Vec<u8>) {
    assert!(
        response
            .headers()
            .get("cache-control")
            .is_some_and(|v| v == "private, no-store"),
        "unexpected cache-control header"
    );
    assert!(
        response
            .headers()
            .get("referrer-policy")
            .is_some_and(|v| v == "no-referrer"),
        "unexpected referrer-policy header"
    );
    assert!(response.headers().get("set-cookie").is_none());
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), 4096)
            .await
            .unwrap_or_else(|_| panic!("response body unavailable"))
            .to_vec(),
    )
}
// Keep failure diagnostics independent of untrusted response bytes.
fn require_empty_success(response: &(StatusCode, Vec<u8>)) {
    assert!(
        response.0 == StatusCode::NO_CONTENT,
        "unexpected success status"
    );
    assert!(response.1.is_empty(), "unexpected success body");
}
fn require_same_denial(actual: &(StatusCode, Vec<u8>), expected: &(StatusCode, Vec<u8>)) {
    assert!(actual.0 == expected.0, "denial statuses differ");
    assert!(actual.1 == expected.1, "denial bodies differ");
}

#[test]
fn response_failure_diagnostics_do_not_render_proof_bytes() {
    let marker = "synthetic-proof-response-control-712093";
    let response = (StatusCode::NO_CONTENT, marker.as_bytes().to_vec());
    let denial = (StatusCode::UNAUTHORIZED, b"denied".to_vec());
    let exposed = (StatusCode::UNAUTHORIZED, marker.as_bytes().to_vec());
    require_empty_success(&(StatusCode::NO_CONTENT, vec![]));
    require_same_denial(&denial, &denial);
    for failure in [
        std::panic::catch_unwind(|| require_empty_success(&response)),
        std::panic::catch_unwind(|| require_same_denial(&exposed, &denial)),
    ] {
        let payload = failure.expect_err("proof-bearing response must fail");
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .expect("failure diagnostic");
        assert!(
            !message.contains(marker),
            "proof reached failure diagnostics"
        );
        assert!(
            !message.contains(&format!("{:?}", marker.as_bytes())),
            "proof bytes reached failure diagnostics"
        );
    }
}

#[test]
fn configuration_is_disabled_by_default_and_only_enabled_in_test() {
    assert!(
        Config::parse(&values())
            .expect("base")
            .verification
            .is_none()
    );
    assert!(
        Config::parse(&enabled())
            .expect("enabled")
            .verification
            .is_some()
    );
    for mode in ["local", "staging", "production"] {
        let mut v = enabled();
        v.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&v).is_err());
    }
    for key in [
        "TEST_VERIFICATION_ORIGIN",
        "TEST_VERIFICATION_ADMISSION_KEY",
    ] {
        let mut v = enabled();
        v.remove(key);
        assert!(Config::parse(&v).is_err());
    }
    for key in ["00".repeat(32), "AA".repeat(32), "ab".repeat(31)] {
        let mut v = enabled();
        v.insert("TEST_VERIFICATION_ADMISSION_KEY".into(), key);
        assert!(Config::parse(&v).is_err());
    }
    let mut v = enabled();
    v.insert(
        "TEST_VERIFICATION_ORIGIN".into(),
        "http://test.example.invalid".into(),
    );
    assert_eq!(
        Config::parse(&v).err(),
        Some("Invalid test verification origin")
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn redemption_is_atomic_and_neutral_on_replay_and_mismatch(pool: PgPool) {
    let token = proof(&pool, "owner@example.com").await;
    let app = app(&pool);
    let wrong = checked(
        app.clone()
            .oneshot(request(body("other@example.com", &token)))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert_eq!(effects(&pool).await, (0, 0, 1));
    let accepted = checked(
        app.clone()
            .oneshot(request(body("owner@example.com", &token)))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(accepted, (StatusCode::NO_CONTENT, vec![]));
    assert_eq!(effects(&pool).await, (1, 1, 2));
    let replay = checked(
        app.clone()
            .oneshot(request(body("owner@example.com", &token)))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(wrong, replay);
    assert_eq!(effects(&pool).await, (1, 1, 3));
    let fresh = proof(&pool, "owner@example.com").await;
    let duplicate = checked(
        app.oneshot(request(body("owner@example.com", &fresh)))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(wrong, duplicate);
    assert_eq!(effects(&pool).await, (1, 1, 4));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_inputs_do_not_debit_or_consume(pool: PgPool) {
    let token = proof(&pool, "owner@example.com").await;
    let app = app(&pool);
    let valid = body("owner@example.com", &token);
    let mut requests = vec![
        request(format!(
            "{{\"email\":\"owner@example.com\",\"email\":\"owner@example.com\",\"token\":\"{token}\"}}"
        )),
        request(format!(
            "{{\"email\":\"owner@example.com\",\"token\":\"{token}\",\"extra\":1}}"
        )),
        request("[1,2]".into()),
        request("x".repeat(4097)),
        request(body("owner@example.com", &token.to_uppercase())),
    ];
    let mut absent = request(valid.clone());
    absent.headers_mut().remove("origin");
    requests.push(absent);
    let mut spoof = request(valid.clone());
    spoof
        .headers_mut()
        .insert("origin", "https://evil.invalid".parse().expect("header"));
    spoof.headers_mut().insert(
        "x-forwarded-host",
        "test.example.invalid".parse().expect("header"),
    );
    requests.push(spoof);
    let mut duplicate = request(valid.clone());
    duplicate
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("header"));
    requests.push(duplicate);
    let mut query = request(valid);
    *query.uri_mut() = format!("{PATH}?token=x").parse().expect("uri");
    requests.push(query);
    for req in requests {
        let (status, _) = checked(app.clone().oneshot(req).await.expect("call")).await;
        assert!(matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE | StatusCode::FORBIDDEN
        ));
    }
    assert_eq!(effects(&pool).await, (0, 0, 0));
    let disabled = router(state(&pool))
        .oneshot(request(body("owner@example.com", &token)))
        .await
        .expect("disabled");
    assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn content_type_failures_do_not_debit_or_consume(pool: PgPool) {
    let token = proof(&pool, "owner@example.com").await;
    let payload = body("owner@example.com", &token);
    let app = app(&pool);
    let mut missing = request(payload.clone());
    missing.headers_mut().remove("content-type");
    let mut unsupported = request(payload.clone());
    unsupported
        .headers_mut()
        .insert("content-type", "text/plain".parse().expect("header"));
    let mut duplicate = request(payload);
    duplicate
        .headers_mut()
        .append("content-type", "application/json".parse().expect("header"));
    for req in [missing, unsupported, duplicate] {
        let response = checked(app.clone().oneshot(req).await.expect("call")).await;
        assert_eq!(
            response,
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                br#"{"code":"verification_json_required"}"#.to_vec()
            )
        );
        assert_eq!(effects(&pool).await, (0, 0, 0));
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn truncated_transport_body_is_redacted_and_has_no_effects(pool: PgPool) {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpStream},
        time::Duration,
    };
    let token = proof(&pool, "owner@example.com").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let router = app(&pool);
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    // A real HTTP premature EOF reaches Bytes extraction as a non-size body error.
    let payload = body("owner@example.com", &token);
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("deadline");
        write!(stream, "POST {PATH} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {PREVIEW}\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n{payload}").expect("partial body");
        stream.shutdown(Shutdown::Write).expect("premature EOF");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("bounded response");
        response
    }).await.expect("transport");
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    let lower = response.to_ascii_lowercase();
    assert!(lower.contains("cache-control: private, no-store"));
    assert!(lower.contains("referrer-policy: no-referrer"));
    assert!(response.contains("invalid_verification_request"));
    assert!(!response.contains("owner@example.com"));
    assert!(!response.contains(PREVIEW));
    assert!(!response.contains(&token));
    assert_eq!(effects(&pool).await, (0, 0, 0));
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_replay_creates_only_one_account(pool: PgPool) {
    let token = proof(&pool, "owner@example.com").await;
    let app = app(&pool);
    let payload = body("owner@example.com", &token);
    let (a, b) = tokio::join!(
        app.clone().oneshot(request(payload.clone())),
        app.oneshot(request(payload))
    );
    let a = checked(a.expect("call")).await.0;
    let b = checked(b.expect("call")).await.0;
    assert!(
        (a == StatusCode::NO_CONTENT && b == StatusCode::UNAUTHORIZED)
            || (b == StatusCode::NO_CONTENT && a == StatusCode::UNAUTHORIZED)
    );
    assert_eq!(effects(&pool).await, (1, 1, 2));
}

struct Earlier;
impl Clock for Earlier {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(1_699_998_000)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_throttle_and_storage_failure_are_redacted(pool: PgPool) {
    let key = AdmissionKey::new(1, [7; 32]).expect("key");
    let expired = email_verification::issue(
        &pool,
        "old@example.com",
        &Earlier,
        &key,
        AdmissionLimits::new(120, 5).expect("limits"),
    )
    .await
    .expect("issue");
    let app = app(&pool);
    let denied = checked(
        app.clone()
            .oneshot(request(body(
                "old@example.com",
                &encode(expired.expose_bytes()),
            )))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(denied.0, StatusCode::UNAUTHORIZED);
    let invalid = body("limited@example.com", &"11".repeat(32));
    for _ in 0..6 {
        assert_eq!(
            checked(
                app.clone()
                    .oneshot(request(invalid.clone()))
                    .await
                    .expect("call")
            )
            .await,
            denied
        );
    }
    assert_eq!(effects(&pool).await, (0, 0, 6));
    sqlx::query("ALTER TABLE email_verification_challenges RENAME TO unavailable_verification")
        .execute(&pool)
        .await
        .expect("interrupt");
    let unavailable = checked(
        app.oneshot(request(body("other@example.com", &"11".repeat(32))))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(unavailable.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(unavailable.1, br#"{"code":"verification_unavailable"}"#);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn auth_and_timeouts_keep_private_headers(pool: PgPool) {
    let token = proof(&pool, "owner@example.com").await;
    let app = app(&pool);
    let mut unauth = request(body("owner@example.com", &token));
    unauth.headers_mut().remove("authorization");
    assert_eq!(
        checked(app.clone().oneshot(unauth).await.expect("call"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query("SELECT verifier FROM email_verification_challenges FOR UPDATE")
        .fetch_all(&mut *tx)
        .await
        .expect("lock");
    let timed = checked(
        app.clone()
            .oneshot(request(body("owner@example.com", &token)))
            .await
            .expect("call"),
    )
    .await;
    assert_eq!(timed.0, StatusCode::GATEWAY_TIMEOUT);
    tx.rollback().await.expect("release");
    assert_eq!(effects(&pool).await, (0, 0, 1));
    let get = Request::builder()
        .uri(PATH)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .body(Body::empty())
        .expect("request");
    assert_eq!(
        checked(app.oneshot(get).await.expect("call")).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

/// A synthetic in-process handoff joins encrypted custody to the test-only API.
/// Its accepted outcome is local callback evidence, not mailbox delivery.
#[sqlx::test(migrator = "MIGRATOR")]
async fn encrypted_handoff_redeems_exact_proof_once_without_issuing_session(pool: PgPool) {
    let delivery_key = VerificationDeliveryKey::capture("synthetic-delivery", [41; 32])
        .expect("synthetic delivery key");
    let admission_key = AdmissionKey::new(1, [7; 32]).expect("synthetic admission key");
    let limits = AdmissionLimits::new(120, 5).expect("test limits");
    register_key(&pool, &Time, &delivery_key)
        .await
        .expect("register delivery key");
    let issued = issue_for_delivery(
        &pool,
        "Case+fixture@EXAMPLE.COM",
        &Time,
        &admission_key,
        limits,
        &delivery_key,
    )
    .await
    .expect("issue encrypted challenge");
    assert_eq!(
        inspect(&pool, &issued.verifier).await,
        Ok(DeliveryState::NotStarted)
    );
    let mut handoff = None;
    assert_eq!(
        claim_open_and_record(
            &pool,
            &issued.verifier,
            &Time,
            &delivery_key,
            |identity, bearer| {
                handoff = Some((identity.to_owned(), encode(bearer)));
                TransportOutcome::Accepted
            }
        )
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        inspect(&pool, &issued.verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
    let (identity, token) = handoff.expect("one synthetic handoff");
    assert!(
        identity == "Case+fixture@example.com",
        "unexpected handoff identity"
    );
    let before: (i64, i64, i64, i64, i32, i32, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM email_verification_delivery_starts),
         (SELECT count(*) FROM email_verification_delivery_outcomes),
         (SELECT count(*) FROM verified_customer_accounts),
         (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL),
         (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=2),
         (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=3),
         (SELECT coalesce(sum(admitted),0) FROM customer_credential_admission_targets WHERE purpose=2),
         (SELECT coalesce(sum(admitted),0) FROM customer_credential_admission_targets WHERE purpose=3)"
    ).fetch_one(&pool).await.expect("initial custody and admission");
    assert_eq!(before, (1, 1, 0, 0, 1, 0, 1, 0));
    let disabled = router(state(&pool))
        .oneshot(request(body(&identity, &token)))
        .await
        .expect("disabled route");
    assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
    assert_eq!(effects(&pool).await, (0, 0, 0));
    let app = app(&pool);
    let first = if &token[..2] == "11" { "22" } else { "11" };
    let invalid_token = format!("{first}{}", &token[2..]);
    let invalid = checked(
        app.clone()
            .oneshot(request(body(&identity, &invalid_token)))
            .await
            .expect("invalid request"),
    )
    .await;
    assert_eq!(invalid.0, StatusCode::UNAUTHORIZED);
    let accepted = checked(
        app.clone()
            .oneshot(request(body(&identity, &token)))
            .await
            .expect("redeem handoff"),
    )
    .await;
    require_empty_success(&accepted);
    let replay = checked(
        app.oneshot(request(body(&identity, &token)))
            .await
            .expect("replay handoff"),
    )
    .await;
    require_same_denial(&replay, &invalid);
    let after: (i64, i64, i64, i64, i32, i32, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM email_verification_delivery_starts),
         (SELECT count(*) FROM email_verification_delivery_outcomes),
         (SELECT count(*) FROM verified_customer_accounts),
         (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL),
         (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=2),
         (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=3),
         (SELECT coalesce(sum(admitted),0) FROM customer_credential_admission_targets WHERE purpose=2),
         (SELECT coalesce(sum(admitted),0) FROM customer_credential_admission_targets WHERE purpose=3)"
    ).fetch_one(&pool).await.expect("final custody and admission");
    assert_eq!(after, (1, 1, 1, 1, 1, 3, 1, 3));
    let quotas: (i64, i64, bool) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM customer_credential_admission_targets WHERE purpose=2),
         (SELECT count(*) FROM customer_credential_admission_targets WHERE purpose=3),
         (SELECT bool_and(global_limit=120 AND target_limit=5)
          FROM customer_credential_admission_scopes WHERE purpose IN (2,3))",
    )
    .fetch_one(&pool)
    .await
    .expect("one target per purpose and exact limits");
    assert_eq!(quotas, (1, 1, true));
    let exact: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM verified_customer_accounts a
         JOIN email_verification_challenges c ON c.verifier=a.verification_verifier
         JOIN email_verification_delivery_starts s ON s.verifier=c.verifier
         JOIN email_verification_delivery_outcomes o ON o.verifier=s.verifier
         WHERE c.verifier=$1 AND a.email_identity=$2 AND c.email_identity=$2
         AND c.delivery_mode='envelope' AND c.consumed_at=a.verified_at
         AND c.consumed_xid IS NOT NULL AND a.xmin::text=c.consumed_xid::text
         AND o.start_transaction_id=s.transaction_id AND o.outcome='accepted')",
    )
    .bind(issued.verifier.as_slice())
    .bind(&identity)
    .fetch_one(&pool)
    .await
    .expect("exact atomic account and local outcome");
    assert!(exact);
    let sessions: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM customer_sessions), (SELECT count(*) FROM guest_sessions)",
    )
    .fetch_one(&pool)
    .await
    .expect("no sessions");
    assert_eq!(sessions, (0, 0));
    assert_eq!(
        inspect(&pool, &issued.verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
}
