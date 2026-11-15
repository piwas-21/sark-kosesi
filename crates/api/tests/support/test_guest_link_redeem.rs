//! Test-only guest-link redemption over a bound customer session.
use super::*;
use commerce_application::Locale;
use commerce_infrastructure::{
    checkout_orders::create_order, checkout_quotes::issue_quote, guest_baskets::GuestCredential,
    guest_order_links,
};

use super::linked_orders::{order_fixture, quote_fixture};

const URL: &str = "/api/test/customer/guest-order-link/redeem";
fn redeem_app(pool: &PgPool, time: Arc<Time>, enabled_route: bool) -> axum::Router {
    let mut config = enabled();
    config.insert("TEST_LOGIN_CSRF_TRANSPORT".into(), "1".into());
    if enabled_route {
        config.insert("TEST_GUEST_LINK_REDEEM_TRANSPORT".into(), "1".into());
    }
    router_with_test_login(
        state(pool, time),
        None,
        None,
        None,
        Config::parse(&config).expect("configuration").test_login,
    )
    .expect("router")
}
fn redeem(cookie: &str, csrf: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(URL)
        .header("authorization", format!("Bearer {PREVIEW}"))
        .header("origin", ORIGIN)
        .header("cookie", cookie)
        .header("x-csrf-token", csrf)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("request")
}
#[test]
fn redeem_requires_separate_test_configuration() {
    let mut config = enabled();
    config.insert("TEST_GUEST_LINK_REDEEM_TRANSPORT".into(), "1".into());
    assert!(Config::parse(&config).is_ok());
    for mode in ["local", "staging", "production"] {
        let mut value = config.clone();
        value.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&value).is_err());
    }
    for bad in ["", "0", "true", "2"] {
        let mut value = config.clone();
        value.insert("TEST_GUEST_LINK_REDEEM_TRANSPORT".into(), bad.into());
        assert!(Config::parse(&value).is_err());
    }
    for key in ["TEST_LOGIN_ORIGIN", "TEST_LOGIN_ADMISSION_KEY"] {
        let mut value = config.clone();
        value.remove(key);
        assert!(Config::parse(&value).is_err());
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn redeem_requires_exact_bound_session_and_consumes_only_once(pool: PgPool) {
    let _case = LOGIN_CASE.lock().await;
    quote_fixture::seed(&pool).await;
    let time = Arc::new(Time(AtomicU64::new(1000)));
    account(&pool, &time).await;
    let app = redeem_app(&pool, time.clone(), true);
    let (cookie, csrf, _) = bound_issue(&app).await;
    let (_, wrong_csrf, _) = bound_issue(&app).await;
    time.0.store(1100, Ordering::SeqCst);
    let (store, _) = quote_fixture::store(&pool, 1100);
    let (guest, guest_csrf) = quote_fixture::guest(&store).await;
    let key = GuestCredential::from_session(&guest).expect("guest");
    let quote = issue_quote(
        &store,
        &key,
        &guest_csrf,
        Locale::Nl,
        &quote_fixture::request("link-redeem"),
    )
    .await
    .expect("quote")
    .snapshot;
    let mut intent = order_fixture::intent(&quote.binding.quote_id);
    intent.buyer.email = EMAIL.into();
    let order = create_order(
        &store,
        &key,
        &guest_csrf,
        "link-redeem",
        &intent,
        &order_fixture::seller(),
    )
    .await
    .expect("order")
    .snapshot
    .identity
    .id;
    let proof = guest_order_links::issue(&store, &key, &guest_csrf, &order)
        .await
        .expect("proof");
    let proof_hex = proof
        .expose_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let body = format!(r#"{{"order_id":"{order}","proof":"{proof_hex}"}}"#);
    let status = |response: axum::response::Response| response.status();
    assert_eq!(
        status(
            redeem_app(&pool, time.clone(), false)
                .oneshot(redeem(&cookie, &csrf, body.clone()))
                .await
                .expect("disabled")
        ),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(
            app.clone()
                .oneshot(redeem(&cookie, &wrong_csrf, body.clone()))
                .await
                .expect("wrong csrf")
        ),
        StatusCode::UNAUTHORIZED
    );
    let mut bad_origin = redeem(&cookie, &csrf, body.clone());
    bad_origin
        .headers_mut()
        .insert("origin", "https://evil.example".parse().expect("header"));
    assert_eq!(
        status(app.clone().oneshot(bad_origin).await.expect("origin")),
        StatusCode::FORBIDDEN
    );
    let mut no_preview = redeem(&cookie, &csrf, body.clone());
    no_preview.headers_mut().remove("authorization");
    assert_eq!(
        status(app.clone().oneshot(no_preview).await.expect("preview")),
        StatusCode::UNAUTHORIZED
    );
    let mut duplicate_cookie = redeem(&cookie, &csrf, body.clone());
    duplicate_cookie
        .headers_mut()
        .append("cookie", cookie.parse().expect("header"));
    assert_eq!(
        status(app.clone().oneshot(duplicate_cookie).await.expect("cookie")),
        StatusCode::UNAUTHORIZED
    );
    let mut query = redeem(&cookie, &csrf, body.clone());
    *query.uri_mut() = format!("{URL}?unused=1").parse().expect("uri");
    assert_eq!(
        status(app.clone().oneshot(query).await.expect("query")),
        StatusCode::BAD_REQUEST
    );
    let extra = format!(r#"{{"order_id":"{order}","proof":"{proof_hex}","account_id":1}}"#);
    assert_eq!(
        status(
            app.clone()
                .oneshot(redeem(&cookie, &csrf, extra))
                .await
                .expect("extra")
        ),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(
            app.clone()
                .oneshot(redeem(&cookie, &csrf, " ".repeat(257)))
                .await
                .expect("large")
        ),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    // Hold the exact proof row until the in-flight request is waiting on its lock.
    let mut lock = pool.begin().await.expect("lock transaction");
    sqlx::query("SELECT verifier FROM guest_order_link_proofs FOR UPDATE")
        .fetch_one(&mut *lock)
        .await
        .expect("locked proof");
    let waiting_app = app.clone();
    let waiting_request = redeem(&cookie, &csrf, body.clone());
    let task = tokio::spawn(async move {
        waiting_app
            .oneshot(waiting_request)
            .await
            .expect("response")
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%FROM guest_order_link_proofs WHERE verifier%'")
                .fetch_one(&pool).await.expect("wait query");
            if waiting > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("proof lock wait");
    time.0.store(2000, Ordering::SeqCst);
    lock.commit().await.expect("release proof");
    assert_eq!(task.await.expect("join").status(), StatusCode::UNAUTHORIZED);
    time.0.store(1100, Ordering::SeqCst);
    let unspent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM guest_order_link_proofs WHERE consumed_at IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(unspent, 0);
    let response = app
        .clone()
        .oneshot(redeem(&cookie, &csrf, body.clone()))
        .await
        .expect("redeem");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_private_headers(&response);
    assert_eq!(
        status(
            app.oneshot(redeem(&cookie, &csrf, body))
                .await
                .expect("replay")
        ),
        StatusCode::UNAUTHORIZED
    );
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_order_links),(SELECT count(*) FROM guest_order_link_proofs WHERE consumed_at IS NOT NULL)")
        .fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (1, 1));
}
