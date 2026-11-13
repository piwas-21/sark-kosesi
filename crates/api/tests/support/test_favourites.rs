//! HTTP-to-custody controls for the separately opted-in test favourites read.
use super::*;
use commerce_application::Locale;
use commerce_infrastructure::customer_favourites;

#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}

const FAVOURITES: &str = "/api/test/customer/favourites/nl";
const OTHER: &str = "other@example.test";

fn favourites_app(pool: &PgPool, time: Arc<Time>) -> axum::Router {
    let mut config = enabled();
    config.insert("TEST_FAVOURITES_TRANSPORT".into(), "1".into());
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

fn read(path: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("read request")
}

async fn login_cookie(app: &axum::Router, email: &str) -> (String, CustomerSessionToken) {
    let response = app
        .clone()
        .oneshot(request(body(email, PASSWORD)))
        .await
        .expect("login");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let header = response
        .headers()
        .get(SET_COOKIE)
        .expect("issued cookie")
        .to_str()
        .expect("cookie header");
    let parsed = Cookie::parse(header.to_owned()).expect("synthetic cookie");
    assert_eq!(parsed.name(), "__Host-piwas-customer");
    let mut bytes = [0_u8; 32];
    let value = parsed.value();
    assert_eq!(value.len(), 64);
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .expect("synthetic session token");
    }
    let token = CustomerSessionToken::from_bytes(bytes);
    let pair = header.split(';').next().expect("cookie pair").to_owned();
    (pair, token)
}

async fn create_other(pool: &PgPool, time: &Time) {
    let proof = email_verification::issue(pool, OTHER, time)
        .await
        .expect("verification");
    verified_customer_accounts::establish(pool, OTHER, &proof, time)
        .await
        .expect("account");
    let proof = customer_passwords::issue(pool, OTHER, time)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, OTHER, &proof, PASSWORD, time)
        .await
        .expect("password");
}

async fn assert_result(app: &axum::Router, request: Request<Body>, status: StatusCode) -> Vec<u8> {
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
        .expect("bounded response")
        .to_vec()
}

#[test]
fn requires_separate_complete_test_configuration() {
    let mut config = enabled();
    config.insert("TEST_FAVOURITES_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&config).is_ok());
    for mode in ["local", "staging", "production"] {
        let mut changed = config.clone();
        changed.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&changed).is_err());
    }
    for value in ["", "0", "true", "2"] {
        let mut changed = config.clone();
        changed.insert("TEST_FAVOURITES_TRANSPORT".into(), value.into());
        assert!(Config::parse(&changed).is_err());
    }
    for key in ["TEST_LOGIN_ORIGIN", "TEST_LOGIN_ADMISSION_KEY"] {
        let mut changed = config.clone();
        changed.remove(key);
        assert!(Config::parse(&changed).is_err());
    }
    let mut missing = values();
    missing.insert("TEST_FAVOURITES_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&missing).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_cookie_reads_current_eligible_favourites_and_withdrawal(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let _fixture = catalogue::media_seed(&pool).await;
    let time = Arc::new(Time(AtomicU64::new(1100)));
    account(&pool, &time).await;
    create_other(&pool, &time).await;
    catalogue::approve(&pool).await;
    catalogue::record_publication(
        &pool,
        &catalogue::author(),
        time.as_ref(),
        catalogue::request("activate", &catalogue::choice()),
    )
    .await
    .expect("activate synthetic product");
    let app = favourites_app(&pool, time.clone());
    let (owner_cookie, owner_token) = login_cookie(&app, EMAIL).await;
    let (other_cookie, _) = login_cookie(&app, OTHER).await;
    assert_eq!(
        customer_favourites::set(
            &pool,
            &owner_token,
            time.as_ref(),
            Locale::Nl,
            "SET-FILLED",
            true,
            0
        )
        .await,
        Ok(1)
    );
    let data = assert_result(&app, read(FAVOURITES, &owner_cookie), StatusCode::OK).await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&data).expect("JSON"),
        serde_json::json!({"revision":"1","items":[{"sku":"SET-FILLED","publication_revision":1}]})
    );
    let other = assert_result(&app, read(FAVOURITES, &other_cookie), StatusCode::OK).await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&other).expect("JSON"),
        serde_json::json!({"revision":"0","items":[]})
    );
    for locale in ["de", "ku"] {
        let data = assert_result(
            &app,
            read(
                &format!("/api/test/customer/favourites/{locale}"),
                &owner_cookie,
            ),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&data).expect("JSON"),
            serde_json::json!({"revision":"1","items":[]})
        );
    }
    assert_result(
        &app,
        read("/api/test/customer/favourites/kmr", &owner_cookie),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let mut withdrawal = catalogue::request("withdraw", &catalogue::PublicationChoice::Withdraw {});
    withdrawal.expected_revision = 1;
    catalogue::record_publication(&pool, &catalogue::author(), time.as_ref(), withdrawal)
        .await
        .expect("withdraw");
    let data = assert_result(&app, read(FAVOURITES, &owner_cookie), StatusCode::OK).await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&data).expect("JSON"),
        serde_json::json!({"revision":"1","items":[]})
    );
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_favourite_buckets), (SELECT count(*) FROM customer_favourites)").fetch_one(&pool).await.expect("bucket state");
    assert_eq!(
        counts,
        (2, 1),
        "a first read initializes the owner's empty bucket"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn transport_rejects_wrong_authority_and_never_exposes_private_state(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let disabled = app(&pool, time.clone());
    let app = favourites_app(&pool, time.clone());
    let (cookie, token) = login_cookie(&app, EMAIL).await;
    assert_result(&disabled, read(FAVOURITES, &cookie), StatusCode::NOT_FOUND).await;
    let mut no_auth = read(FAVOURITES, &cookie);
    no_auth.headers_mut().remove("authorization");
    assert_result(&app, no_auth, StatusCode::UNAUTHORIZED).await;
    let mut no_origin = read(FAVOURITES, &cookie);
    no_origin.headers_mut().remove("origin");
    assert_result(&app, no_origin, StatusCode::FORBIDDEN).await;
    let mut bad_origin = read(FAVOURITES, &cookie);
    bad_origin.headers_mut().insert(
        "origin",
        "https://other.example.invalid".parse().expect("origin"),
    );
    assert_result(&app, bad_origin, StatusCode::FORBIDDEN).await;
    let mut duplicate_origin = read(FAVOURITES, &cookie);
    duplicate_origin
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    assert_result(&app, duplicate_origin, StatusCode::FORBIDDEN).await;
    let mut duplicate_cookie = read(FAVOURITES, &cookie);
    duplicate_cookie
        .headers_mut()
        .append("cookie", cookie.parse().expect("cookie"));
    assert_result(&app, duplicate_cookie, StatusCode::UNAUTHORIZED).await;
    assert_result(
        &app,
        read(FAVOURITES, &format!("{cookie}; {cookie}")),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_result(
        &app,
        read(FAVOURITES, "__Host-piwas-customer=invalid"),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert_result(
        &app,
        read(&format!("{FAVOURITES}?account_id=1"), &cookie),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let mut body = read(FAVOURITES, &cookie);
    *body.body_mut() = Body::from("unexpected");
    assert_result(&app, body, StatusCode::BAD_REQUEST).await;
    let mut head = read(FAVOURITES, &cookie);
    *head.method_mut() = axum::http::Method::HEAD;
    assert_result(&app, head, StatusCode::METHOD_NOT_ALLOWED).await;
    let mut options = read(FAVOURITES, &cookie);
    *options.method_mut() = axum::http::Method::OPTIONS;
    assert_result(&app, options, StatusCode::METHOD_NOT_ALLOWED).await;
    customer_sessions::logout(&pool, &token, time.as_ref())
        .await
        .expect("revoke");
    let denied = assert_result(&app, read(FAVOURITES, &cookie), StatusCode::UNAUTHORIZED).await;
    let text = String::from_utf8(denied).expect("error UTF-8");
    assert!(!text.contains(&cookie));
    let buckets: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_favourite_buckets")
        .fetch_one(&pool)
        .await
        .expect("bucket count");
    assert_eq!(buckets, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_bucket_wait_and_rotation_deny_the_old_cookie(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let app = favourites_app(&pool, time.clone());
    let (cookie, token) = login_cookie(&app, EMAIL).await;
    let identity = customer_sessions::authenticate(&pool, &token, time.as_ref())
        .await
        .expect("current session");
    assert_result(&app, read(FAVOURITES, &cookie), StatusCode::OK).await;
    let mut held = pool.begin().await.expect("hold bucket");
    sqlx::query("SELECT revision FROM customer_favourite_buckets WHERE account_id=$1 FOR UPDATE")
        .bind(identity.account_id)
        .execute(&mut *held)
        .await
        .expect("lock bucket");
    let waiting_app = app.clone();
    let waiting_cookie = cookie.clone();
    let waiting = tokio::spawn(async move {
        waiting_app
            .oneshot(read(FAVOURITES, &waiting_cookie))
            .await
            .expect("waiting response")
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revision FROM customer_favourite_buckets%'")
                .fetch_one(&pool).await.expect("inspect wait");
            if count == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("reader reached bucket lock");
    time.0.store(44_200, Ordering::SeqCst);
    held.rollback().await.expect("release bucket");
    let response = waiting.await.expect("request task");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_private_headers(&response);
    assert_result(&app, read(FAVOURITES, &cookie), StatusCode::UNAUTHORIZED).await;
    time.0.store(1000, Ordering::SeqCst);
    let (rotated_cookie, old_token) = login_cookie(&app, EMAIL).await;
    customer_sessions::rotate(&pool, &old_token, time.as_ref())
        .await
        .expect("rotate");
    assert_result(
        &app,
        read(FAVOURITES, &rotated_cookie),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}
