mod guest_support;
use axum::http::StatusCode;
use commerce_api::{guest::GuestOrigin, router};
use commerce_infrastructure::MIGRATOR;
use guest_support::*;
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
use tower::ServiceExt;
#[test]
fn origin_is_exact_canonical_https_and_not_a_forwarded_host_guess() {
    for good in [ORIGIN, "https://localhost:8443", "https://[::1]:8443"] {
        assert!(GuestOrigin::parse(good).is_ok());
    }
    for bad in [
        "http://guest.example.test",
        "null",
        "https://guest.example.test/",
        "https://guest.example.test/path",
        "https://guest.example.test?x=1",
        "https://guest.example.test#fragment",
        "https://user:pass@guest.example.test",
        "https://GUEST.example.test",
        "https://guest.example.test:443",
        "https://guest.example.test.evil/path",
        "",
    ] {
        assert!(GuestOrigin::parse(bad).is_err(), "{bad}");
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn secure_sessions_keep_owners_separate_and_revoke_at_the_server(pool: PgPool) {
    let (app, _) = app(&pool);
    let (a, ac) = issue(&app, None).await;
    let (b, bc) = issue(&app, None).await;
    for (cookie, csrf, status) in [
        (&a, &bc, StatusCode::FORBIDDEN),
        (&b, &ac, StatusCode::FORBIDDEN),
        (&a, &ac, StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "PUT",
                "/api/guest/basket",
                Some(cookie),
                Some(csrf),
                &update("first", 0, 2),
            ))
            .await
            .expect("write");
        assert_eq!(response.status(), status);
    }
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/guest/basket",
            Some(&a),
            Some(&ac),
            &update("first", 0, 2),
        ))
        .await
        .expect("retry");
    assert_eq!(
        json(response).await,
        serde_json::json!({"revision":1,"replayed":true})
    );
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/guest/basket",
            Some(&a),
            Some(&ac),
            &update("first", 0, 3),
        ))
        .await
        .expect("conflict");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    for (cookie, revision) in [(&a, 1), (&b, 0)] {
        let response = app
            .clone()
            .oneshot(request("GET", "/api/guest/basket", Some(cookie), None, ""))
            .await
            .expect("read");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await["revision"], revision);
    }
    let response = app
        .clone()
        .oneshot(request("POST", "/api/guest/session", Some(&a), None, "{}"))
        .await
        .expect("reuse");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("set-cookie").is_none());
    assert_eq!(json(response).await["csrf_token"], ac);
    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/api/guest/session",
            Some(&a),
            Some(&ac),
            "",
        ))
        .await
        .expect("revoke");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let raw = response.headers()["set-cookie"].to_str().expect("removal");
    for flag in ["Max-Age=0", "Secure", "HttpOnly", "SameSite=Lax", "Path=/"] {
        assert!(raw.contains(flag), "{raw}");
    }
    let response = app
        .clone()
        .oneshot(request("GET", "/api/guest/basket", Some(&a), None, ""))
        .await
        .expect("old cookie");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(request("GET", "/api/guest/basket", Some(&b), None, ""))
        .await
        .expect("other owner");
    assert_eq!(response.status(), StatusCode::OK);
    let (fresh, fc) = issue(&app, Some(&a)).await;
    assert_ne!(fresh, a);
    assert_ne!(fc, ac);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn origins_csrf_closed_bodies_and_size_limits_fail_before_changes(pool: PgPool) {
    let (app, _) = app(&pool);
    for origin in [
        None,
        Some("null"),
        Some("https://evil.test"),
        Some("https://guest.example.test.evil"),
        Some("https://guest.example.test:443"),
        Some("https://guest.example.test/"),
    ] {
        let mut r = request("POST", "/api/guest/session", None, None, "{}");
        r.headers_mut().remove("origin");
        if let Some(origin) = origin {
            r.headers_mut()
                .insert("origin", origin.parse().expect("header"));
        }
        r.headers_mut()
            .insert("host", "guest.example.test".parse().expect("host"));
        r.headers_mut().insert(
            "x-forwarded-host",
            "guest.example.test".parse().expect("host"),
        );
        assert_eq!(
            app.clone().oneshot(r).await.expect("deny").status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut r = request("POST", "/api/guest/session", None, None, "{}");
    r.headers_mut()
        .append("origin", ORIGIN.parse().expect("origin"));
    assert_eq!(
        app.clone().oneshot(r).await.expect("duplicate").status(),
        StatusCode::FORBIDDEN
    );
    for (input, status) in [
        ("{\"basket_id\":\"x\"}".to_owned(), StatusCode::BAD_REQUEST),
        ("[]".into(), StatusCode::BAD_REQUEST),
        ("{".into(), StatusCode::BAD_REQUEST),
        (" ".repeat(4097), StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request("POST", "/api/guest/session", None, None, &input))
                .await
                .expect("closed")
                .status(),
            status
        );
    }
    let mut r = request("POST", "/api/guest/session", None, None, "{}");
    r.headers_mut().remove("content-type");
    assert_eq!(
        app.clone().oneshot(r).await.expect("JSON").status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_baskets")
            .fetch_one(&pool)
            .await
            .expect("no changes"),
        0
    );
    let (cookie, csrf) = issue(&app, None).await;
    for value in [
        None,
        Some("bad"),
        Some("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "PUT",
                    "/api/guest/basket",
                    Some(&cookie),
                    value,
                    &update("a", 0, 1)
                ))
                .await
                .expect("CSRF")
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut r = request(
        "PUT",
        "/api/guest/basket",
        Some(&cookie),
        Some(&csrf),
        &update("a", 0, 1),
    );
    r.headers_mut()
        .append("x-csrf-token", csrf.parse().expect("header"));
    assert_eq!(
        app.clone()
            .oneshot(r)
            .await
            .expect("duplicate CSRF")
            .status(),
        StatusCode::FORBIDDEN
    );
    for input in [
        update("a", -1, 1),
        update("a", 0, 0),
        update("a", 0, 101),
        update("a", 0, 1).replace("SET-A", "DEMO-A"),
        update("a", 0, 1).replace("\"quantity\":1", "\"quantity\":1,\"price\":1"),
        update("a", 0, 1).replacen('{', "{\"owner\":\"x\",", 1),
        r#"{"effect_key":"a","expected_revision":0,"lines":[["SET-A",1]]}"#.into(),
        r#"{"effect_key":"a","expected_revision":0,"lines":{}}"#.into(),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    "PUT",
                    "/api/guest/basket",
                    Some(&cookie),
                    Some(&csrf),
                    &input
                ))
                .await
                .expect("invalid update")
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        app.clone()
            .oneshot(request(
                "DELETE",
                "/api/guest/session",
                Some(&cookie),
                Some(&csrf),
                "{}"
            ))
            .await
            .expect("delete payload")
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_basket_effects")
            .fetch_one(&pool)
            .await
            .expect("no effects"),
        0
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_routes_are_opt_in_preview_protected_and_fail_closed_on_dependency_or_expiry(
    pool: PgPool,
) {
    let (disabled, _) = state(&pool);
    let disabled = router(disabled);
    assert_eq!(
        disabled
            .oneshot(request("POST", "/api/guest/session", None, None, "{}"))
            .await
            .expect("disabled")
            .status(),
        StatusCode::NOT_FOUND
    );
    let (app, clock) = app(&pool);
    let mut r = request("POST", "/api/guest/session", None, None, "{}");
    r.headers_mut().remove("authorization");
    assert_eq!(
        app.clone().oneshot(r).await.expect("private").status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_sessions")
            .fetch_one(&pool)
            .await
            .expect("no issuance"),
        0
    );
    for cookie in [
        None,
        Some("__Host-piwas-guest=unknown"),
        Some("__Host-piwas-guest=AAAAAAAAAAAAAAAAAAAAAA"),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request("GET", "/api/guest/basket", cookie, None, ""))
                .await
                .expect("no owner")
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let (cookie, _) = issue(&app, None).await;
    clock.0.fetch_add(604800, Ordering::SeqCst);
    assert_eq!(
        app.clone()
            .oneshot(request("GET", "/api/guest/basket", Some(&cookie), None, ""))
            .await
            .expect("expired")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    pool.close().await;
    let response = app
        .clone()
        .oneshot(request("POST", "/api/guest/session", None, None, "{}"))
        .await
        .expect("offline");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        json(response).await,
        serde_json::json!({"code":"guest_unavailable"})
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn http_rate_limits_cannot_be_bypassed_with_new_requests_or_forged_forwarding(pool: PgPool) {
    let (app, _) = app(&pool);
    let (cookie, csrf) = issue(&app, None).await;
    pool.execute("UPDATE guest_baskets SET rate_window=(SELECT created_at/60 FROM guest_sessions LIMIT 1),mutation_count=60").await.expect("rate ceiling fixture");
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/guest/basket",
            Some(&cookie),
            Some(&csrf),
            &update("a", 0, 1),
        ))
        .await
        .expect("rate");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "60");
    pool.execute("UPDATE guest_issue_quota SET rate_window=(SELECT created_at/60 FROM guest_sessions LIMIT 1),issued=64").await.expect("issue ceiling fixture");
    let response = app
        .clone()
        .oneshot(request("POST", "/api/guest/session", None, None, "{}"))
        .await
        .expect("rate");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_sessions")
            .fetch_one(&pool)
            .await
            .expect("count"),
        1
    );
}
