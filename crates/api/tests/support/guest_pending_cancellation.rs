//! Exact test-only HTTP reads and requests after an offline synthetic start.
use super::*;

const FLAG: &str = "TEST_GUEST_PENDING_CANCELLATION_TRANSPORT";
const INTENT: &str = r#"{"effect_key":"pending-cancel-1","confirmed":true}"#;

fn enabled() -> HashMap<String, String> {
    let mut settings = values();
    settings.insert(FLAG.into(), "1".into());
    settings
}

fn route(id: &str) -> String {
    format!("/api/test/guest/orders/{id}/pending-cancellation")
}

async fn response(
    app: &Router,
    req: axum::http::Request<axum::body::Body>,
    status: StatusCode,
) -> serde_json::Value {
    let response = app.clone().oneshot(req).await.expect("private response");
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    json(response).await
}

#[test]
fn pending_flag_requires_exact_test_mode_and_guest_origin() {
    let off = Config::parse(&super::enabled()).expect("pre-dispatch alone");
    assert!(off.test_guest_pending_cancellation.is_none());
    let on = Config::parse(&enabled()).expect("independently enabled pending route");
    assert!(on.test_guest_pending_cancellation.is_some());
    assert!(on.test_guest_cancellation.is_none());
    for mode in ["local", "staging", "production"] {
        let mut settings = enabled();
        settings.insert("APP_ENV".into(), mode.into());
        assert!(Config::parse(&settings).is_err());
    }
    for value in ["", "0", "true", "2"] {
        let mut settings = enabled();
        settings.insert(FLAG.into(), value.into());
        assert!(Config::parse(&settings).is_err());
    }
    let mut missing_origin = enabled();
    missing_origin.remove("GUEST_ORIGIN");
    assert!(Config::parse(&missing_origin).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pending_intent_round_trips_and_replays_without_canceling_the_order(pool: PgPool) {
    let (app, clock, store, cookie, csrf, order) = Box::pin(accepted_with(&pool, &enabled())).await;
    let config = Config::parse(&enabled()).expect("validated pending config");
    let (state, _) = guest_support::state(&pool);
    assert!(
        router_with_test_guest_operations(
            state,
            None,
            None,
            None,
            None,
            TestGuestOperations {
                pending_cancellation: config.test_guest_pending_cancellation,
                ..TestGuestOperations::default()
            }
        )
        .is_err()
    );
    let pending = route(&order.identity.id);
    let read = || request("GET", &pending, Some(&cookie), None, "");
    assert_eq!(
        response(&app, read(), StatusCode::OK).await,
        serde_json::json!({"request":null})
    );
    assert_eq!(
        send(
            &app,
            &pending,
            Some(&cookie),
            Some(&csrf),
            INTENT,
            StatusCode::CONFLICT
        )
        .await["code"],
        "cancellation_conflict"
    );
    let (pre_only, _, _) = app_with(&pool, &super::enabled());
    assert_eq!(
        pre_only
            .clone()
            .oneshot(read())
            .await
            .expect("disabled pending route")
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pre_only
            .oneshot(request(
                "POST",
                &pending,
                Some(&cookie),
                Some(&csrf),
                INTENT
            ))
            .await
            .expect("disabled pending submission")
            .status(),
        StatusCode::NOT_FOUND
    );
    let pre = request(
        "POST",
        &path(&order.identity.id),
        Some(&cookie),
        Some(&csrf),
        INPUT,
    );
    assert_eq!(
        app.clone()
            .oneshot(pre)
            .await
            .expect("disabled pre-dispatch route")
            .status(),
        StatusCode::NOT_FOUND
    );

    let session_id: Id = cookie
        .strip_prefix("__Host-piwas-guest=")
        .expect("canonical cookie")
        .parse()
        .expect("session ID");
    let session = Session::new(Some(session_id), Arc::new(store.clone()), None);
    let owner = GuestCredential::from_session(&session).expect("owned guest");
    let configuration = payment::config();
    let prepared = Box::pin(prepare_attempt(
        &store,
        &owner,
        &csrf,
        "pending-http-pay",
        &payment::intent(&order),
        &configuration,
    ))
    .await
    .expect("offline attempt");
    assert_eq!(
        send(
            &app,
            &pending,
            Some(&cookie),
            Some(&csrf),
            INTENT,
            StatusCode::CONFLICT
        )
        .await["code"],
        "cancellation_conflict"
    );
    let ids = payment::Ids(AtomicU64::new(82_000));
    assert!(matches!(
        Box::pin(start_dispatch(
            &pool,
            clock.as_ref(),
            &ids,
            &prepared.frozen.snapshot().binding.id,
            &configuration
        ))
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    let before = effects(&pool).await;
    assert_eq!(
        response(&app, read(), StatusCode::OK).await,
        serde_json::json!({"request":null})
    );
    let wrong_csrf = "0".repeat(64);
    assert_eq!(
        send(
            &app,
            &pending,
            Some(&cookie),
            Some(&wrong_csrf),
            INTENT,
            StatusCode::FORBIDDEN
        )
        .await["code"],
        "guest_csrf_required"
    );
    let (foreign_cookie, foreign_csrf) = issue(&app, None).await;
    assert_eq!(
        send(
            &app,
            &pending,
            Some(&foreign_cookie),
            Some(&foreign_csrf),
            INTENT,
            StatusCode::NOT_FOUND
        )
        .await["code"],
        "cancellation_order_not_found"
    );
    assert_eq!(
        response(
            &app,
            request("GET", &pending, Some(&foreign_cookie), None, ""),
            StatusCode::NOT_FOUND
        )
        .await["code"],
        "cancellation_order_not_found"
    );
    assert_eq!(
        response(
            &app,
            request("GET", &pending, None, None, ""),
            StatusCode::UNAUTHORIZED
        )
        .await["code"],
        "guest_session_required"
    );

    let body = send(
        &app,
        &pending,
        Some(&cookie),
        Some(&csrf),
        INTENT,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        body,
        serde_json::json!({"requested_at":1500,"replayed":false})
    );
    clock.0.store(1501, Ordering::SeqCst);
    assert_eq!(
        send(
            &app,
            &pending,
            Some(&cookie),
            Some(&csrf),
            INTENT,
            StatusCode::OK
        )
        .await,
        serde_json::json!({"requested_at":1500,"replayed":true})
    );
    let mut no_origin = read();
    no_origin.headers_mut().remove("origin");
    assert_eq!(
        response(&app, no_origin, StatusCode::FORBIDDEN).await["code"],
        "guest_origin_required"
    );
    assert_eq!(
        response(&app, read(), StatusCode::OK).await,
        serde_json::json!({"request":{"requested_at":1500}})
    );
    assert_eq!(
        send(
            &app,
            &pending,
            Some(&cookie),
            Some(&csrf),
            &INTENT.replace("pending-cancel-1", "another"),
            StatusCode::CONFLICT
        )
        .await["code"],
        "cancellation_conflict"
    );
    assert_eq!(effects(&pool).await, before);
    let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_post_dispatch_requests),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'),(SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM payment_observations)")
        .fetch_one(&pool).await.expect("one pending receipt and event only");
    assert_eq!(counts, (1, 1, 0, 0));

    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated damaged pending event");
    assert_eq!(
        response(&app, read(), StatusCode::SERVICE_UNAVAILABLE).await["code"],
        "cancellation_unavailable"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pending_route_rejects_noncanonical_requests_and_closed_body_shapes(pool: PgPool) {
    let (app, _, _, cookie, csrf, order) = Box::pin(accepted_with(&pool, &enabled())).await;
    let pending = route(&order.identity.id);
    for input in [
        r#"{"effect_key":"pending","confirmed":false}"#,
        r#"{"effect_key":"bad key","confirmed":true}"#,
        r#"{"effect_key":"pending","confirmed":true,"customer":"unexpected"}"#,
        r#"{"effect_key":"pending","effect_key":"duplicate","confirmed":true}"#,
        r#"["pending",true]"#,
    ] {
        send(
            &app,
            &pending,
            Some(&cookie),
            Some(&csrf),
            input,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    send(
        &app,
        &pending,
        Some(&cookie),
        Some(&csrf),
        &"x".repeat(300),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    for (method, body) in [("POST", INTENT), ("GET", "")] {
        let mut wrong_origin = request(method, &pending, Some(&cookie), Some(&csrf), body);
        wrong_origin.headers_mut().append(
            "origin",
            "https://other.example.test".parse().expect("wrong origin"),
        );
        assert_eq!(
            response(&app, wrong_origin, StatusCode::FORBIDDEN).await["code"],
            "guest_origin_required"
        );
        let mut wrong_origin = request(method, &pending, Some(&cookie), Some(&csrf), body);
        wrong_origin.headers_mut().insert(
            "origin",
            "https://other.example.test".parse().expect("wrong origin"),
        );
        assert_eq!(
            response(&app, wrong_origin, StatusCode::FORBIDDEN).await["code"],
            "guest_origin_required"
        );
    }

    let mut duplicate = request("GET", &pending, Some(&cookie), None, "");
    duplicate
        .headers_mut()
        .append("cookie", cookie.parse().expect("duplicate cookie"));
    assert_eq!(
        response(&app, duplicate, StatusCode::UNAUTHORIZED).await["code"],
        "guest_session_required"
    );
    for (url, body) in [
        (format!("{pending}?untrusted=1"), ""),
        (route("not-canonical"), ""),
        (pending, "{}"),
    ] {
        assert_eq!(
            response(
                &app,
                request("GET", &url, Some(&cookie), None, body),
                StatusCode::BAD_REQUEST
            )
            .await["code"],
            "invalid_cancellation_request"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM checkout_post_dispatch_requests")
            .fetch_one(&pool)
            .await
            .expect("no intent"),
        0
    );
}
