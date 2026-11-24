//! Test-only exact owner progress over isolated native custody.
use super::*;
use axum::{body::Body, http::Request};
use commerce_infrastructure::payment_attempts::{identities::resolve, recognition::recognize};
use serde_json::{Value, json as value};

const FLAG: &str = "TEST_GUEST_ORDER_PROGRESS_TRANSPORT";

fn enabled() -> HashMap<String, String> {
    let mut settings = values();
    settings.insert(FLAG.into(), "1".into());
    settings
}

fn route(id: &str) -> String {
    format!("/api/test/guest/orders/{id}/progress")
}

async fn response(app: &Router, request: Request<Body>, status: StatusCode) -> Value {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("private response");
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    assert!(!response.headers().contains_key("set-cookie"));
    json(response).await
}

#[test]
fn exact_progress_flag_is_independent_and_requires_test_origin() {
    let off = Config::parse(&super::enabled()).expect("pre-dispatch cancellation alone");
    assert!(off.test_guest_order_progress.is_none());
    let on = Config::parse(&enabled()).expect("independently enabled progress");
    assert!(on.test_guest_order_progress.is_some());
    assert!(on.test_guest_order.is_none());
    assert!(on.test_guest_cancellation.is_none());
    assert!(on.test_guest_pending_cancellation.is_none());
    for environment in ["local", "staging", "production"] {
        let mut settings = enabled();
        settings.insert("APP_ENV".into(), environment.into());
        assert!(Config::parse(&settings).is_err());
    }
    for value in ["", "0", "true", "2"] {
        let mut settings = enabled();
        settings.insert(FLAG.into(), value.into());
        assert!(Config::parse(&settings).is_err());
    }
    let mut settings = enabled();
    settings.remove("GUEST_ORIGIN");
    assert!(Config::parse(&settings).is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn read_tracks_offline_start_and_pending_intent_without_inventing_a_payment(pool: PgPool) {
    let mut settings = enabled();
    settings.insert(
        "TEST_GUEST_PENDING_CANCELLATION_TRANSPORT".into(),
        "1".into(),
    );
    let (app, time, store, cookie, csrf, order) = Box::pin(accepted_with(&pool, &settings)).await;
    let target = route(&order.identity.id);
    let read = || request("GET", &target, Some(&cookie), None, "");
    let unstarted = value!({
        "order": "accepted", "pending_cancellation": false, "gross_payment": "no_recorded_dispatch",
    });
    assert_eq!(response(&app, read(), StatusCode::OK).await, unstarted);
    let session_id: Id = cookie
        .strip_prefix("__Host-piwas-guest=")
        .expect("canonical cookie")
        .parse()
        .expect("session ID");
    let session = Session::new(Some(session_id), Arc::new(store.clone()), None);
    let key = GuestCredential::from_session(&session).expect("exact owner");
    let configuration = payment::config();
    let preparation = Box::pin(prepare_attempt(
        &store,
        &key,
        &csrf,
        "progress-http-attempt",
        &payment::intent(&order),
        &configuration,
    ))
    .await
    .expect("offline attempt");
    assert_eq!(response(&app, read(), StatusCode::OK).await, unstarted);
    let ids = payment::Ids(AtomicU64::new(90_000));
    assert!(matches!(
        Box::pin(start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &preparation.frozen.snapshot().binding.id,
            &configuration,
        ))
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    assert_eq!(
        response(&app, read(), StatusCode::OK).await,
        value!({"order":"accepted","pending_cancellation":false,"gross_payment":"no_recognition"})
    );
    let pending = format!(
        "/api/test/guest/orders/{}/pending-cancellation",
        order.identity.id
    );
    send(
        &app,
        &pending,
        Some(&cookie),
        Some(&csrf),
        r#"{"effect_key":"progress-pending","confirmed":true}"#,
        StatusCode::OK,
    )
    .await;
    let before = effects(&pool).await;
    let active = value!({
        "order": "accepted", "pending_cancellation": true, "gross_payment": "no_recognition",
    });
    for _ in 0..2 {
        assert_eq!(response(&app, read(), StatusCode::OK).await, active);
    }
    assert_eq!(effects(&pool).await, before);
    let facts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM payment_observations),(SELECT count(*) FROM payment_recognitions),(SELECT count(*) FROM checkout_post_dispatch_requests)")
        .fetch_one(&pool).await.expect("native custody footprint");
    assert_eq!(facts, (0, 0, 1));

    // Advance only the isolated native recognition journal after proving the
    // offline start and pending intent did not create any payment facts.
    let frozen = preparation.frozen.snapshot();
    let attempt = &frozen.binding.id;
    time.0.store(1530, Ordering::SeqCst);
    for (revision, provider, status, expected) in [
        (0, "", "", "unconfirmed"),
        (1, "tr_SyntheticA", "paid", "received"),
        (2, "tr_SyntheticB", "open", "needs_review"),
    ] {
        if revision != 0 {
            Box::pin(payment::append_observation(
                &pool, frozen, revision, provider, status,
            ))
            .await;
            assert_eq!(
                response(&app, read(), StatusCode::SERVICE_UNAVAILABLE).await,
                value!({"code":"order_progress_unavailable"}),
                "an obsolete recognition must be withheld until refreshed"
            );
        }
        let effect = format!("progress-resolve-{revision}");
        Box::pin(resolve(&pool, time.as_ref(), &ids, attempt, &effect))
            .await
            .expect("resolve the synthetic observation prefix");
        Box::pin(recognize(&pool, time.as_ref(), &ids, attempt))
            .await
            .expect("recognize the current synthetic prefix");
        let before = effects(&pool).await;
        assert_eq!(
            response(&app, read(), StatusCode::OK).await,
            value!({"order":"accepted","pending_cancellation":true,"gross_payment":expected})
        );
        assert_eq!(effects(&pool).await, before);
    }

    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated missing dispatch event");
    assert_eq!(
        response(&app, read(), StatusCode::SERVICE_UNAVAILABLE).await,
        value!({"code":"order_progress_unavailable"})
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn test_route_enforces_origin_exact_request_guest_and_toggle(pool: PgPool) {
    let mut settings = super::enabled();
    settings.insert(FLAG.into(), "1".into());
    let (app, _, _, cookie, csrf, order) = Box::pin(accepted_with(&pool, &settings)).await;
    let config = Config::parse(&settings).expect("validated progress configuration");
    let (state, _) = guest_support::state(&pool);
    assert!(
        router_with_test_guest_operations(
            state,
            None,
            None,
            None,
            None,
            TestGuestOperations {
                order_progress: config.test_guest_order_progress,
                ..TestGuestOperations::default()
            }
        )
        .is_err()
    );
    let target = route(&order.identity.id);
    let read = || request("GET", &target, Some(&cookie), None, "");
    let (off, _, _) = app_with(&pool, &super::enabled());
    assert_eq!(
        off.oneshot(read())
            .await
            .expect("disabled progress")
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut missing_preview = read();
    missing_preview.headers_mut().remove("authorization");
    assert_eq!(
        app.clone()
            .oneshot(missing_preview)
            .await
            .expect("preview gate")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut wrong_origin = read();
    wrong_origin
        .headers_mut()
        .insert("origin", "https://other.test".parse().expect("origin"));
    let mut missing_origin = read();
    missing_origin.headers_mut().remove("origin");
    let mut duplicate_origin = read();
    duplicate_origin
        .headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    for invalid in [wrong_origin, missing_origin, duplicate_origin] {
        assert_eq!(
            response(&app, invalid, StatusCode::FORBIDDEN).await,
            value!({"code":"guest_origin_required"})
        );
    }
    let encoded = route(&format!(
        "%{:02x}{}",
        order.identity.id.as_bytes()[0],
        &order.identity.id[1..]
    ));
    for (path, body) in [
        (&encoded, ""),
        (&format!("{target}?state=accepted"), ""),
        (&route("invalid-id"), ""),
        (&target, "{}"),
    ] {
        assert_eq!(
            response(
                &app,
                request("GET", path, Some(&cookie), None, body),
                StatusCode::BAD_REQUEST
            )
            .await,
            value!({"code":"invalid_order_progress_request"})
        );
    }
    let mut duplicate_cookie = read();
    duplicate_cookie
        .headers_mut()
        .append("cookie", cookie.parse().expect("canonical value"));
    for no_guest in [request("GET", &target, None, None, ""), duplicate_cookie] {
        assert_eq!(
            response(&app, no_guest, StatusCode::UNAUTHORIZED).await,
            value!({"code":"guest_session_required"})
        );
    }
    let (foreign, _) = issue(&app, None).await;
    let missing = route("99999999999999999999999999999999");
    for bad_target in [
        request("GET", &target, Some(&foreign), None, ""),
        request("GET", &missing, Some(&cookie), None, ""),
    ] {
        assert_eq!(
            response(&app, bad_target, StatusCode::NOT_FOUND).await,
            value!({"code":"order_progress_not_found"})
        );
    }
    assert_eq!(
        app.clone()
            .oneshot(request("POST", &target, Some(&cookie), Some(&csrf), "{}"))
            .await
            .expect("read only")
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    send(
        &app,
        &path(&order.identity.id),
        Some(&cookie),
        Some(&csrf),
        INPUT,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        response(&app, read(), StatusCode::OK).await,
        value!({"order":"cancelled_before_dispatch","pending_cancellation":false,"gross_payment":"no_recorded_dispatch"})
    );
    let revoked = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/api/guest/session",
            Some(&cookie),
            Some(&csrf),
            "",
        ))
        .await
        .expect("explicit fixture revocation");
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response(&app, read(), StatusCode::UNAUTHORIZED).await,
        value!({"code":"guest_session_required"})
    );
}
