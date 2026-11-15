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
    let (cookie, csrf, session) = bound_issue(&app).await;
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

    let mut wrong_type = redeem(&cookie, &csrf, body.clone());
    wrong_type.headers_mut().remove("content-type");
    assert_eq!(
        status(app.clone().oneshot(wrong_type).await.expect("content type")),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    for invalid in [
        "[]".to_owned(),
        format!(r#"{{"order_id":"a","proof":"{proof_hex}"}}"#),
        format!(
            r#"{{"order_id":"{}","proof":"{proof_hex}"}}"#,
            "z".repeat(32)
        ),
        format!(r#"{{"order_id":"{order}","proof":"invalid"}}"#),
    ] {
        assert_eq!(
            status(
                app.clone()
                    .oneshot(redeem(&cookie, &csrf, invalid))
                    .await
                    .expect("invalid input")
            ),
            StatusCode::BAD_REQUEST
        );
    }
    pool.execute("CREATE FUNCTION fixture_reject_link_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END $$").await.expect("fault function");
    pool.execute("CREATE TRIGGER fixture_reject_link_update BEFORE UPDATE ON guest_order_link_proofs FOR EACH ROW EXECUTE FUNCTION fixture_reject_link_update()").await.expect("fault trigger");
    assert_eq!(
        status(
            app.clone()
                .oneshot(redeem(&cookie, &csrf, body.clone()))
                .await
                .expect("storage error")
        ),
        StatusCode::SERVICE_UNAVAILABLE
    );
    pool.execute("DROP TRIGGER fixture_reject_link_update ON guest_order_link_proofs")
        .await
        .expect("remove fault");

    // A real premature EOF exercises the non-size body extraction failure.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let server_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, server_app).await });
    let wire_cookie = cookie.clone();
    let wire_csrf = csrf.clone();
    let wire_body = body.clone();
    let response = tokio::task::spawn_blocking(move || {
        use std::{io::{Read, Write}, net::{Shutdown, TcpStream}};
        let mut stream = TcpStream::connect(address).expect("connect");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).expect("deadline");
        write!(stream, "POST {URL} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {PREVIEW}\r\nOrigin: {ORIGIN}\r\nCookie: {wire_cookie}\r\nx-csrf-token: {wire_csrf}\r\nContent-Type: application/json\r\nContent-Length: 256\r\nConnection: close\r\n\r\n{wire_body}").expect("partial body");
        stream.shutdown(Shutdown::Write).expect("EOF");
        let mut output = String::new();
        stream.read_to_string(&mut output).expect("response");
        output
    }).await.expect("transport");
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(!response.contains(&proof_hex));
    server.abort();

    // Both requests hit a real proof-lock timeout. Only the first session is
    // still live when that storage failure returns; the second must fail closed.
    let mut csrf_bytes = [0; 32];
    for (index, pair) in csrf.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        csrf_bytes[index] = digit(pair[0]) * 16 + digit(pair[1]);
    }
    for expires_while_waiting in [false, true] {
        let mut lock = pool.begin().await.expect("lock transaction");
        sqlx::query("SELECT verifier FROM guest_order_link_proofs FOR UPDATE")
            .fetch_one(&mut *lock)
            .await
            .expect("locked proof");
        let task_pool = pool.clone();
        let task_time = time.clone();
        let task_order = order.clone();
        let task_session = CustomerSessionToken::from_bytes(*session.expose_bytes());
        let task_csrf = CustomerCsrfToken::from_bytes(csrf_bytes);
        let task_proof = guest_order_links::GuestOrderLinkToken::from_bytes(*proof.expose_bytes());
        let task = tokio::spawn(async move {
            guest_order_links::link_with_csrf(
                &task_pool,
                &task_session,
                &task_csrf,
                &task_order,
                &task_proof,
                task_time.as_ref(),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%FROM guest_order_link_proofs WHERE verifier%'")
                    .fetch_one(&pool).await.expect("wait query");
                if waiting > 0 { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("proof lock wait");
        if expires_while_waiting {
            time.0.store(44200, Ordering::SeqCst);
        }
        let result = task.await.expect("join");
        if expires_while_waiting {
            assert!(matches!(result, Err(guest_order_links::LinkError::Denied)));
        } else {
            assert!(matches!(
                result,
                Err(guest_order_links::LinkError::Unavailable)
            ));
        }
        lock.commit().await.expect("release proof");
        time.0.store(1100, Ordering::SeqCst);
    }

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
