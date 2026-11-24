//! Test-only HTTP over real guest/order custody; no provider I/O.
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code)]
mod guest_support;
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment;
use axum::{Router, http::StatusCode};
use commerce_api::{TestGuestOperations, config::Config, router_with_test_guest_operations};
use commerce_application::{Locale, guest_access::GuestLimits};
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    guest_baskets::{GuestCredential, GuestSessionStore},
    payment_attempts::{DispatchOutcome, prepare_attempt, start_dispatch},
};
use guest_support::*;
use payment::{order, quote as fixture};
use sqlx::{Executor, PgPool};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tower::ServiceExt;
use tower_sessions::{Session, session::Id};

const INPUT: &str = r#"{"effect_key":"cancel-1","confirmed":true}"#;
#[path = "support/guest_pending_cancellation.rs"]
mod pending;
fn values() -> HashMap<String, String> {
    [
        ("APP_ENV", "test"),
        ("DATABASE_URL", "postgresql://localhost/isolated"),
        ("PREVIEW_TOKEN", TOKEN),
        ("API_BIND", "127.0.0.1:4567"),
        ("GUEST_ORIGIN", ORIGIN),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn enabled() -> HashMap<String, String> {
    let mut v = values();
    v.insert("TEST_GUEST_CANCELLATION_TRANSPORT".into(), "1".into());
    v
}
fn app_with(
    pool: &PgPool,
    values: &HashMap<String, String>,
) -> (Router, Arc<Time>, GuestSessionStore) {
    let config = Config::parse(values).expect("test config");
    let (state, time) = guest_support::state(pool);
    time.0.store(1100, Ordering::SeqCst);
    let store = GuestSessionStore::new(
        pool.clone(),
        state.clock.clone(),
        state.identifiers.clone(),
        GuestLimits::default(),
    )
    .expect("same guest ports");
    let app = router_with_test_guest_operations(
        state,
        config.guest_origin,
        config.callback_profile,
        config.verification,
        config.test_login,
        TestGuestOperations {
            order: config.test_guest_order,
            cancellation: config.test_guest_cancellation,
            pending_cancellation: config.test_guest_pending_cancellation,
        },
    )
    .expect("router");
    (app, time, store)
}
async fn accepted(
    pool: &PgPool,
) -> (
    Router,
    Arc<Time>,
    GuestSessionStore,
    String,
    String,
    commerce_domain::checkout_orders::OrderSnapshot,
) {
    accepted_with(pool, &enabled()).await
}
async fn accepted_with(
    pool: &PgPool,
    values: &HashMap<String, String>,
) -> (
    Router,
    Arc<Time>,
    GuestSessionStore,
    String,
    String,
    commerce_domain::checkout_orders::OrderSnapshot,
) {
    fixture::seed(pool).await;
    let (app, time, store) = app_with(pool, values);
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
    let raw = cookie.strip_prefix("__Host-piwas-guest=").expect("cookie");
    let id: Id = raw.parse().expect("session id");
    let session = Session::new(Some(id), Arc::new(store.clone()), None);
    let owner = GuestCredential::from_session(&session).expect("owned guest");
    let quote = issue_quote(
        &store,
        &owner,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
    )
    .await
    .expect("quote")
    .snapshot;
    let snapshot = create_order(
        &store,
        &owner,
        &csrf,
        "place",
        &order::intent(&quote.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("accepted order")
    .snapshot;
    time.0.store(1500, Ordering::SeqCst);
    (app, time, store, cookie, csrf, snapshot)
}
fn path(id: &str) -> String {
    format!("/api/test/guest/orders/{id}/cancellation")
}
async fn send(
    app: &Router,
    path: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: &str,
    status: StatusCode,
) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(request("POST", path, cookie, csrf, body))
        .await
        .expect("response");
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    json(response).await
}
async fn effects(pool: &PgPool) -> serde_json::Value {
    sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(x) ORDER BY component_code) FROM stock_balances x),'holds',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM stock_holds x),'movements',(SELECT jsonb_agg(to_jsonb(x) ORDER BY sequence) FROM stock_movements x),'redemptions',(SELECT jsonb_agg(to_jsonb(x) ORDER BY order_id) FROM promotion_redemptions x),'dispatches',(SELECT count(*) FROM payment_dispatch_starts),'refunds',(SELECT count(*) FROM refund_reservations))")
    .fetch_one(pool).await.expect("physical and financial custody")
}
#[test]
fn cancellation_flag_requires_exact_test_mode_and_guest_origin() {
    assert!(
        Config::parse(&values())
            .expect("default")
            .test_guest_cancellation
            .is_none()
    );
    assert!(
        Config::parse(&enabled())
            .expect("enabled")
            .test_guest_cancellation
            .is_some()
    );
    for mode in ["local", "staging", "production"] {
        let mut v = enabled();
        v.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&v).is_err());
    }
    for value in ["", "0", "true", "2"] {
        let mut v = enabled();
        v.insert("TEST_GUEST_CANCELLATION_TRANSPORT".into(), value.into());
        assert!(Config::parse(&v).is_err());
    }
    let mut v = enabled();
    v.remove("GUEST_ORIGIN");
    assert!(Config::parse(&v).is_err());
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_can_cancel_and_exact_retry_preserves_effects(pool: PgPool) {
    let (app, _, _, cookie, csrf, order) = accepted(&pool).await;
    let url = path(&order.identity.id);
    let before = effects(&pool).await;
    let mut with_extra = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    with_extra.headers_mut().append(
        "cookie",
        "other=unrelated; malformed"
            .parse()
            .expect("unrelated cookie"),
    );
    let result = app
        .clone()
        .oneshot(with_extra)
        .await
        .expect("separate unrelated cookie");
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(json(result).await, serde_json::json!({"replayed": false}));
    assert_eq!(
        send(
            &app,
            &url,
            Some(&cookie),
            Some(&csrf),
            INPUT,
            StatusCode::OK
        )
        .await,
        serde_json::json!({"replayed":true})
    );
    assert_eq!(
        send(
            &app,
            &url,
            Some(&cookie),
            Some(&csrf),
            &INPUT.replace("cancel-1", "cancel-2"),
            StatusCode::CONFLICT
        )
        .await["code"],
        "cancellation_conflict"
    );
    assert_eq!(effects(&pool).await, before);
    let count:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_canceled.v1')").fetch_one(&pool).await.expect("one receipt/event");
    assert_eq!(count, (1, 1));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_transport_rejects_forged_ownership_and_confirmation(pool: PgPool) {
    let (app, _, _, cookie, csrf, order) = accepted(&pool).await;
    let url = path(&order.identity.id);
    let (other, other_csrf) = issue(&app, None).await;
    let (disabled, _, _) = app_with(&pool, &values());
    let resp = disabled
        .clone()
        .oneshot(request("POST", &url, Some(&cookie), Some(&csrf), INPUT))
        .await
        .expect("disabled route");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        send(
            &app,
            &url,
            Some(&other),
            Some(&other_csrf),
            INPUT,
            StatusCode::NOT_FOUND
        )
        .await["code"],
        "cancellation_order_not_found"
    );
    send(
        &app,
        &url,
        None,
        Some(&csrf),
        INPUT,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    send(
        &app,
        &url,
        Some(&cookie),
        None,
        INPUT,
        StatusCode::FORBIDDEN,
    )
    .await;
    send(
        &app,
        &url,
        Some(&cookie),
        Some(&other_csrf),
        INPUT,
        StatusCode::FORBIDDEN,
    )
    .await;
    let invalid = [
        r#"{"effect_key":"x","confirmed":false}"#,
        r#"{"effect_key":"x","confirmed":true,"confirmed":false}"#,
        r#"{"effect_key":"bad key","confirmed":true}"#,
        r#"{"effect_key":"x","confirmed":true,"order_id":"else"}"#,
        "[]",
    ];
    for input in invalid {
        send(
            &app,
            &url,
            Some(&cookie),
            Some(&csrf),
            input,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    send(
        &app,
        &url,
        Some(&cookie),
        Some(&csrf),
        &" ".repeat(257),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    let id = &order.identity.id;
    let encoded = format!(
        "/api/test/guest/orders/%{:02X}{}/cancellation",
        id.as_bytes()[0],
        &id[1..]
    );
    for bad in [format!("{url}?x=1"), path("bad"), encoded] {
        send(
            &app,
            &bad,
            Some(&cookie),
            Some(&csrf),
            INPUT,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    for value in [
        format!("{cookie}; {cookie}"),
        format!("{cookie}; __Host-piwas-guest=invalid"),
        format!("other={}; {cookie}", "a".repeat(4097)),
        "other=unrelated".to_owned(),
    ] {
        send(
            &app,
            &url,
            Some(&value),
            Some(&csrf),
            INPUT,
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    let mut duplicate_csrf = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    duplicate_csrf
        .headers_mut()
        .append("x-csrf-token", csrf.parse().expect("csrf"));
    assert_eq!(
        app.clone()
            .oneshot(duplicate_csrf)
            .await
            .expect("duplicate csrf")
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut duplicate_origin = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    duplicate_origin
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    assert_eq!(
        app.clone()
            .oneshot(duplicate_origin)
            .await
            .expect("duplicate origin")
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut duplicate = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    duplicate
        .headers_mut()
        .append("cookie", cookie.parse().expect("cookie"));
    assert_eq!(
        app.clone()
            .oneshot(duplicate)
            .await
            .expect("duplicate cookie headers")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut req = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    req.headers_mut()
        .insert("origin", "https://evil.example".parse().expect("origin"));
    let resp = app.clone().oneshot(req).await.expect("foreign origin");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let mut req = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    req.headers_mut().remove("authorization");
    assert_eq!(
        app.clone().oneshot(req).await.expect("preview").status(),
        StatusCode::UNAUTHORIZED
    );
    let mut req = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    req.headers_mut()
        .insert("content-type", "text/plain".parse().expect("type"));
    assert_eq!(
        app.clone().oneshot(req).await.expect("type").status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_order_cancellations")
        .fetch_one(&pool)
        .await
        .expect("no writes");
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn dispatch_wins_real_race_and_expired_guest_cannot_see_conflict(pool: PgPool) {
    let (app, time, store, cookie, csrf, order) = accepted(&pool).await;
    let id: Id = cookie
        .strip_prefix("__Host-piwas-guest=")
        .expect("cookie")
        .parse()
        .expect("id");
    let session = Session::new(Some(id), Arc::new(store.clone()), None);
    let key = GuestCredential::from_session(&session).expect("guest");
    let config = payment::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &config,
    )
    .await
    .expect("prepared");
    let attempt = prepared.frozen.snapshot().binding.id.clone();
    let expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM guest_sessions WHERE basket_id=$1")
            .bind(&order.identity.basket_id)
            .fetch_one(&pool)
            .await
            .expect("guest expiry");
    let mut winner = pool.begin().await.expect("winner");
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,'dddddddddddddddddddddddddddddddd',1500)").bind(&attempt).execute(&mut *winner).await.expect("dispatch start");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('cccccccccccccccccccccccccccccccc',$1,'payment.reconciliation_required.v1',$2,1500)")
        .bind(&order.identity.id).bind(serde_json::json!({"schema_version":1,"order_id":order.identity.id,"attempt_id":attempt,"dispatch_id":"dddddddddddddddddddddddddddddddd"})).execute(&mut *winner).await.expect("recovery event");
    let url = path(&order.identity.id);
    let req = request("POST", &url, Some(&cookie), Some(&csrf), INPUT);
    let task = tokio::spawn({
        let app = app.clone();
        async move { app.oneshot(req).await.expect("request") }
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(id,33),created_at FROM payment_attempts%')").fetch_one(&pool).await.expect("actual attempt wait");
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("cancel reached actual attempt claim");
    time.0
        .store(u64::try_from(expiry).expect("expiry"), Ordering::SeqCst);
    winner.commit().await.expect("dispatch commits");
    assert_eq!(task.await.expect("task").status(), StatusCode::UNAUTHORIZED);
    time.0.store(1500, Ordering::SeqCst);
    send(
        &app,
        &url,
        Some(&cookie),
        Some(&csrf),
        INPUT,
        StatusCode::CONFLICT,
    )
    .await;
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &payment::Ids(AtomicU64::new(10000)),
            &attempt,
            &config
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM payment_dispatch_starts)").fetch_one(&pool).await.expect("one dispatch only");
    assert_eq!(counts, (0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn storage_failure_rolls_back_and_healthy_retry_succeeds(pool: PgPool) {
    let (app, _, _, cookie, csrf, order) = accepted(&pool).await;
    let url = path(&order.identity.id);
    let before = effects(&pool).await;
    pool.execute("CREATE FUNCTION test_cancel_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'private test fault'; END $$; CREATE TRIGGER test_cancel_fault BEFORE INSERT ON checkout_order_cancellations FOR EACH ROW EXECUTE FUNCTION test_cancel_fault()")
        .await.expect("isolated fault");
    let result = send(
        &app,
        &url,
        Some(&cookie),
        Some(&csrf),
        INPUT,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(
        result,
        serde_json::json!({"code":"cancellation_unavailable"})
    );
    assert_eq!(effects(&pool).await, before);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_order_cancellations")
        .fetch_one(&pool)
        .await
        .expect("rollback");
    assert_eq!(n, 0);
    pool.execute("DROP TRIGGER test_cancel_fault ON checkout_order_cancellations; DROP FUNCTION test_cancel_fault()").await.expect("remove fault");
    assert_eq!(
        send(
            &app,
            &url,
            Some(&cookie),
            Some(&csrf),
            INPUT,
            StatusCode::OK
        )
        .await,
        serde_json::json!({"replayed":false})
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stalled_request_body_times_out_without_cancellation(pool: PgPool) {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };
    let (app, _, _, cookie, csrf, order) = accepted(&pool).await;
    let before = effects(&pool).await;
    let url = path(&order.identity.id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let response=tokio::task::spawn_blocking(move || {
        let mut stream=TcpStream::connect_timeout(&address,Duration::from_secs(3)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("read deadline");
        write!(stream,"POST {url} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nOrigin: {ORIGIN}\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{").expect("partial body");
        let mut response=String::new();stream.take(4096).read_to_string(&mut response).expect("timeout response");
        assert!(!response.contains(&cookie));assert!(!response.contains(&csrf));response
    }).await.expect("transport");
    server.abort();
    assert!(response.starts_with("HTTP/1.1 504 Gateway Timeout"));
    assert!(response.contains("dependency_timeout"));
    assert_eq!(effects(&pool).await, before);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_order_cancellations")
        .fetch_one(&pool)
        .await
        .expect("no writes");
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn prepared_but_undispatched_attempt_can_be_canceled_and_never_started(pool: PgPool) {
    let (app, time, store, cookie, csrf, order) = accepted(&pool).await;
    let id: Id = cookie
        .strip_prefix("__Host-piwas-guest=")
        .expect("cookie")
        .parse()
        .expect("id");
    let session = Session::new(Some(id), Arc::new(store.clone()), None);
    let key = GuestCredential::from_session(&session).expect("guest");
    let config = payment::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &config,
    )
    .await
    .expect("prepared");
    let before = effects(&pool).await;
    assert_eq!(
        send(
            &app,
            &path(&order.identity.id),
            Some(&cookie),
            Some(&csrf),
            INPUT,
            StatusCode::OK
        )
        .await,
        serde_json::json!({"replayed":false})
    );
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &payment::Ids(AtomicU64::new(10000)),
            &prepared.frozen.snapshot().binding.id,
            &config
        )
        .await,
        Err(
            commerce_infrastructure::payment_attempts::PaymentStoreError::Boundary(
                commerce_infrastructure::checkout_orders::OrderStoreError::Conflict
            )
        )
    ));
    assert_eq!(effects(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_only_route_cannot_cancel_account_bound_order(pool: PgPool) {
    use admitted_proofs::{customer_passwords, email_verification, verified_customer_accounts};
    use commerce_infrastructure::{
        checkout_orders::create_verified_promoted_order,
        checkout_quotes::issue_verified_promoted_quote,
    };
    fixture::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("promotion grants");
    let mut policy = payment::promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    payment::promotion::draft(&pool, &policy, "draft")
        .await
        .expect("draft");
    payment::promotion::decide(&pool, "review", 1, 0, true)
        .await
        .expect("review");
    let (app, time, store) = app_with(&pool, &enabled());
    let email = "bound@example.test";
    let proof = email_verification::issue(&pool, email, time.as_ref())
        .await
        .expect("verification");
    verified_customer_accounts::establish(&pool, email, &proof, time.as_ref())
        .await
        .expect("account");
    let proof = customer_passwords::issue(&pool, email, time.as_ref())
        .await
        .expect("password proof");
    customer_passwords::replace(
        &pool,
        email,
        &proof,
        "correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("password");
    let account =
        admitted_login::login(&pool, email, "correct horse battery staple", time.as_ref())
            .await
            .expect("login")
            .token;
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
    let id: Id = cookie
        .strip_prefix("__Host-piwas-guest=")
        .expect("cookie")
        .parse()
        .expect("id");
    let session = Session::new(Some(id), Arc::new(store.clone()), None);
    let key = GuestCredential::from_session(&session).expect("guest");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &account,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("account-bound quote")
    .snapshot;
    let owned = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &account,
        "place",
        &order::intent(&quote.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("account order")
    .snapshot;
    let before = effects(&pool).await;
    send(
        &app,
        &path(&owned.identity.id),
        Some(&cookie),
        Some(&csrf),
        INPUT,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(effects(&pool).await, before);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_order_cancellations")
        .fetch_one(&pool)
        .await
        .expect("no cancellation");
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn truncated_http_body_is_rejected_without_cancellation(pool: PgPool) {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpStream},
        time::Duration,
    };
    let (app, _, _, cookie, csrf, order) = accepted(&pool).await;
    let before = effects(&pool).await;
    let url = path(&order.identity.id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let response=tokio::task::spawn_blocking(move || {
        let mut stream=TcpStream::connect_timeout(&address,Duration::from_secs(3)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(3))).expect("read deadline");
        write!(stream,"POST {url} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nOrigin: {ORIGIN}\r\nCookie: {cookie}\r\nx-csrf-token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{INPUT}",INPUT.len()+1).expect("partial request");
        stream.shutdown(Shutdown::Write).expect("premature EOF");
        let mut response=String::new();stream.take(4096).read_to_string(&mut response).expect("response");
        assert!(!response.contains(&cookie));assert!(!response.contains(&csrf));response
    }).await.expect("transport");
    server.abort();
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(response.contains("invalid_cancellation_request"));
    assert_eq!(effects(&pool).await, before);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_order_cancellations")
        .fetch_one(&pool)
        .await
        .expect("no writes");
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_cookie_aliases_cannot_shadow_or_duplicate_the_session(pool: PgPool) {
    let (app, _, _, cookie, csrf, order) = accepted(&pool).await;
    let value = cookie
        .strip_prefix("__Host-piwas-guest=")
        .expect("cookie value");
    let url = path(&order.identity.id);
    for alias in [
        format!("__Host-piwas-%67uest={value}"),
        format!("__Host-piwas-guest ={value}"),
        format!(
            "__Host-piwas-guest=%{:02X}{}",
            value.as_bytes()[0],
            &value[1..]
        ),
    ] {
        for raw in [
            format!("{cookie}; {alias}"),
            format!("{alias}; {cookie}"),
            alias.clone(),
        ] {
            send(
                &app,
                &url,
                Some(&raw),
                Some(&csrf),
                INPUT,
                StatusCode::UNAUTHORIZED,
            )
            .await;
        }
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM checkout_order_cancellations")
        .fetch_one(&pool)
        .await
        .expect("no ambiguous cancellation");
    assert_eq!(n, 0);
    assert_eq!(
        send(
            &app,
            &url,
            Some(&cookie),
            Some(&csrf),
            INPUT,
            StatusCode::OK
        )
        .await,
        serde_json::json!({"replayed":false})
    );
}
