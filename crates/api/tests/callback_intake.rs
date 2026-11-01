#[path = "support/callback_intake_support.rs"]
mod support;
#[path = "support/callback_intake_transport.rs"]
mod transport;
use axum::{body::Body, http::StatusCode};
use commerce_api::{config::Config, contract, router};
use commerce_infrastructure::MIGRATOR;
use sqlx::{Executor, PgPool};
use std::{collections::HashMap, sync::atomic::Ordering};
use support::*;
use tower::ServiceExt;

#[test]
fn callback_configuration_is_explicit_private_and_contract_is_closed() {
    let mut values = HashMap::from([
        ("APP_ENV".into(), "test".into()),
        (
            "DATABASE_URL".into(),
            "postgresql://localhost/isolated".into(),
        ),
        ("PREVIEW_TOKEN".into(), TOKEN.into()),
        ("API_BIND".into(), "127.0.0.1:4567".into()),
    ]);
    assert!(
        Config::parse(&values)
            .expect("default")
            .callback_profile
            .is_none()
    );
    values.insert("CALLBACK_INTAKE_PROFILE_ID".into(), PROFILE.into());
    assert!(
        Config::parse(&values)
            .expect("enabled")
            .callback_profile
            .is_some()
    );
    let mut malformed_postgres = values.clone();
    malformed_postgres.insert(
        "DATABASE_URL".into(),
        "postgresql://127.0.0.1:invalid/database".into(),
    );
    assert_eq!(
        Config::parse(&malformed_postgres)
            .err()
            .expect("invalid PostgreSQL port despite an accepted URL prefix"),
        "PostgreSQL URL required"
    );
    let mut url_safe_token = values.clone();
    let token = format!("{}-_", "a".repeat(30));
    assert_eq!(token.len(), 32);
    url_safe_token.insert("PREVIEW_TOKEN".into(), token.clone());
    let accepted = Config::parse(&url_safe_token).expect("hyphen and underscore are URL-safe");
    assert_eq!(accepted.preview_token, token);
    assert!(accepted.callback_profile.is_some());
    for profile in [
        "",
        "pfl_",
        "tr_wrong",
        " pfl_test",
        "pfl_test ",
        "pfl_\nprivate",
    ] {
        values.insert("CALLBACK_INTAKE_PROFILE_ID".into(), profile.into());
        let error = Config::parse(&values).err().expect("invalid profile");
        assert!(!error.contains(profile) || profile.is_empty());
    }
    values.insert("CALLBACK_INTAKE_PROFILE_ID".into(), PROFILE.into());
    for (name, value) in [("APP_ENV", "production"), ("API_BIND", "0.0.0.0:4567")] {
        let mut invalid = values.clone();
        invalid.insert(name.into(), value.into());
        assert!(Config::parse(&invalid).is_err());
    }
    values.insert("GUEST_ORIGIN".into(), ORIGIN.into());
    let config = Config::parse(&values).expect("independent options");
    assert!(config.guest_origin.is_some() && config.callback_profile.is_some());
    let document: serde_json::Value =
        serde_json::from_str(&contract::openapi().expect("OpenAPI")).expect("JSON");
    let post = &document["paths"][PATH]["post"];
    assert_eq!(post["security"], serde_json::json!([{"preview":[]}]));
    assert!(post["responses"]["200"].is_object());
    assert!(post["requestBody"]["content"]["application/x-www-form-urlencoded"].is_object());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn acknowledged_intakes_commit_complete_custody_without_order_or_payment_authority(
    pool: PgPool,
) {
    let (app, _, _) = private_app(&pool);
    for body in [
        "id=tr_NOT_IN_OUR_ORDERS",
        "%69d=tr_NOT_IN_OUR_ORDERS",
        "id=tr_DIFFERENT_UPDATE",
    ] {
        let response = app
            .clone()
            .oneshot(request(body))
            .await
            .expect("private admission");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        assert!(response.headers().get("set-cookie").is_none());
        assert!(
            bytes(response).await.is_empty(),
            "same empty acknowledgement for unknown and repeated IDs"
        );
    }
    assert_eq!(custody(&pool).await, (3, 3, 3, 3));
    let exact: i64 = sqlx::query_scalar("SELECT count(*) FROM callback_intakes i JOIN callback_intake_events e ON e.intake_id=i.id JOIN callback_inbox_jobs j ON j.intake_id=i.id JOIN callback_inbox_active a ON a.intake_id=i.id WHERE i.profile_id=$1 AND i.received_at=1500 AND e.kind='payment.callback_received.v1' AND e.created_at=1500 AND e.payload=jsonb_build_object('schema_version',1,'intake_id',i.id,'profile_id',$1::text,'provider_id',i.provider_id,'received_at',1500) AND j.source_event_id=e.id AND j.state='pending' AND j.generation=0 AND NOT j.settled AND j.created_at=1500 AND j.updated_at=1500 AND j.due_at=1500 AND a.profile_id=$1 AND a.created_at=1500")
        .bind(PROFILE).fetch_one(&pool).await.expect("independent exact committed proof");
    assert_eq!(exact, 3);
    let quota: Vec<(String, i32)> =
        sqlx::query_as("SELECT scope,admitted FROM callback_admission_quota ORDER BY scope")
            .fetch_all(&pool)
            .await
            .expect("quotas");
    assert_eq!(quota, vec![("global".into(), 3), (PROFILE.into(), 3)]);
    no_financial_effects(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_form_media_and_byte_limits_have_no_partial_admission(pool: PgPool) {
    let (app, _, _) = private_app(&pool);
    for (body, status) in [
        ("", StatusCode::BAD_REQUEST),
        ("id=", StatusCode::BAD_REQUEST),
        ("id=tr_", StatusCode::BAD_REQUEST),
        ("id=wrong", StatusCode::BAD_REQUEST),
        ("id=tr_a&id=tr_b", StatusCode::BAD_REQUEST),
        ("id=tr_a&%69d=tr_b", StatusCode::BAD_REQUEST),
        ("id=tr_a&profile_id=pfl_other", StatusCode::BAD_REQUEST),
        ("id=tr_a&status=paid", StatusCode::BAD_REQUEST),
        ("id=tr_a&", StatusCode::BAD_REQUEST),
        ("&id=tr_a", StatusCode::BAD_REQUEST),
        ("id=tr_%", StatusCode::BAD_REQUEST),
        ("id=tr_%GG", StatusCode::BAD_REQUEST),
        ("id=tr_%ff", StatusCode::BAD_REQUEST),
        ("id=tr_%C0%AF", StatusCode::BAD_REQUEST),
        ("id=tr_%00", StatusCode::BAD_REQUEST),
        ("id=tr_a%0A", StatusCode::BAD_REQUEST),
        ("id=+tr_a", StatusCode::BAD_REQUEST),
        ("id=tr_a+", StatusCode::BAD_REQUEST),
        ("{\"id\":\"tr_a\"}", StatusCode::BAD_REQUEST),
    ] {
        failure(
            app.clone().oneshot(request(body)).await.expect("rejection"),
            status,
        )
        .await;
    }
    for content_type in [
        "application/json",
        "text/plain",
        "application/x-www-form-urlencoded; charset=latin1",
        "application/x-www-form-urlencoded; charset=UTF-8; charset=UTF-8",
        "application/x-www-form-urlencoded; boundary=private",
        "",
    ] {
        let mut r = request("id=tr_MEDIA");
        r.headers_mut()
            .insert("content-type", content_type.parse().expect("test header"));
        failure(
            app.clone().oneshot(r).await.expect("media rejection"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        )
        .await;
    }
    let mut absent = request("id=tr_ABSENT");
    absent.headers_mut().remove("content-type");
    failure(
        app.clone().oneshot(absent).await.expect("no media"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
    )
    .await;
    let mut duplicate = request("id=tr_DUPLICATE");
    duplicate.headers_mut().append(
        "content-type",
        "application/x-www-form-urlencoded"
            .parse()
            .expect("duplicate header"),
    );
    failure(
        app.clone()
            .oneshot(duplicate)
            .await
            .expect("duplicate media"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
    )
    .await;
    for encoding in ["gzip", "identity", "br"] {
        let mut encoded = request("id=tr_ENCODED");
        encoded
            .headers_mut()
            .insert("content-encoding", encoding.parse().expect("encoding"));
        failure(
            app.clone().oneshot(encoded).await.expect("encoding denied"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        )
        .await;
    }
    let invalid_utf8 = request(Body::from(vec![b'i', b'd', b'=', b't', b'r', b'_', 0xff]));
    failure(
        app.clone().oneshot(invalid_utf8).await.expect("raw UTF8"),
        StatusCode::BAD_REQUEST,
    )
    .await;
    for (length, status) in [
        (1024, StatusCode::BAD_REQUEST),
        (1025, StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let raw = format!("id=tr_{}", "x".repeat(length - 6));
        assert_eq!(raw.len(), length);
        failure(
            app.clone()
                .oneshot(request(raw))
                .await
                .expect("raw byte bound"),
            status,
        )
        .await;
    }
    assert_eq!(custody(&pool).await, (0, 0, 0, 0));
    let max_id = format!("tr_{}", "a".repeat(253));
    assert_eq!(max_id.len(), 256);
    let response = app
        .clone()
        .oneshot(request(format!("id={max_id}")))
        .await
        .expect("literal maximum provider ID");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(bytes(response).await.is_empty());
    failure(
        app.clone()
            .oneshot(request(format!("id={max_id}x")))
            .await
            .expect("decoded overflow"),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(custody(&pool).await, (1, 1, 1, 1));
    no_financial_effects(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn opt_in_and_existing_bearer_protection_hold_with_optional_guest_routes(pool: PgPool) {
    let (base, _, _) = state(&pool);
    let disabled = router(base.clone())
        .oneshot(request("id=tr_DISABLED"))
        .await
        .expect("disabled endpoint");
    assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
    let app = configured(base, PROFILE, true);
    for token in [None, Some("Bearer wrong"), Some("Basic spoof")] {
        let mut r = request("id=tr_UNAUTHENTICATED");
        r.headers_mut().remove("authorization");
        if let Some(value) = token {
            r.headers_mut()
                .insert("authorization", value.parse().expect("token header"));
        }
        r.headers_mut()
            .insert("cookie", "preview=pretend".parse().expect("cookie"));
        r.headers_mut()
            .insert("x-forwarded-for", "127.0.0.1".parse().expect("forwarded"));
        failure(
            app.clone().oneshot(r).await.expect("authentication"),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    for method in ["GET", "PUT", "DELETE", "HEAD"] {
        let mut r = request("");
        *r.method_mut() = method.parse().expect("method");
        assert_eq!(
            app.clone().oneshot(r).await.expect("wrong method").status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    let mut guest = request("{}");
    *guest.uri_mut() = "/api/guest/session".parse().expect("guest path");
    guest
        .headers_mut()
        .insert("content-type", "application/json".parse().expect("JSON"));
    guest
        .headers_mut()
        .insert("origin", ORIGIN.parse().expect("origin"));
    let response = app
        .clone()
        .oneshot(guest)
        .await
        .expect("guest remains available");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("set-cookie").is_some());
    assert_eq!(
        app.clone()
            .oneshot(request("id=tr_WITH_GUEST"))
            .await
            .expect("callback alongside guest")
            .status(),
        StatusCode::OK
    );
    assert_eq!(custody(&pool).await, (1, 1, 1, 1));
    no_financial_effects(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn profile_rate_capacity_and_forwarded_headers_cannot_bypass_native_admission(pool: PgPool) {
    let (app, clock, _) = private_app(&pool);
    for n in 0..30 {
        assert_eq!(
            app.clone()
                .oneshot(request(format!("id=tr_PROFILE_{n}")))
                .await
                .expect("profile rate slot")
                .status(),
            StatusCode::OK
        );
    }
    let mut spoof = request("id=tr_RATE_OVER");
    for (key, value) in [
        ("x-forwarded-for", "203.0.113.7"),
        ("x-real-ip", "198.51.100.9"),
        ("x-forwarded-host", "other-profile.invalid"),
    ] {
        spoof
            .headers_mut()
            .insert(key, value.parse().expect("forwarded header"));
    }
    failure(
        app.clone().oneshot(spoof).await.expect("native rate limit"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(custody(&pool).await, (30, 30, 30, 30));
    clock.at.store(1560, Ordering::SeqCst);
    for n in 30..50 {
        assert_eq!(
            app.clone()
                .oneshot(request(format!("id=tr_PROFILE_{n}")))
                .await
                .expect("profile capacity slot")
                .status(),
            StatusCode::OK
        );
    }
    failure(
        app.clone()
            .oneshot(request("id=tr_CAPACITY_OVER"))
            .await
            .expect("native capacity"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(custody(&pool).await, (50, 50, 50, 50));
    let admitted: i32 =
        sqlx::query_scalar("SELECT admitted FROM callback_admission_quota WHERE scope=$1")
            .bind(PROFILE)
            .fetch_one(&pool)
            .await
            .expect("failed admission rollback");
    assert_eq!(
        admitted, 20,
        "capacity failure cannot partially consume rate quota"
    );
    no_financial_effects(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn global_rate_and_capacity_span_all_server_configured_profiles(pool: PgPool) {
    let (state, clock, _) = state(&pool);
    let apps: Vec<_> = (0..5)
        .map(|n| configured(state.clone(), &format!("pfl_CALLBACK_GLOBAL_{n}"), false))
        .collect();
    for (profile, app) in apps.iter().take(4).enumerate() {
        for n in 0..30 {
            assert_eq!(
                app.clone()
                    .oneshot(request(format!("id=tr_GLOBAL_{profile}_{n}")))
                    .await
                    .expect("global rate slot")
                    .status(),
                StatusCode::OK
            );
        }
    }
    failure(
        apps[4]
            .clone()
            .oneshot(request("id=tr_GLOBAL_RATE_OVER"))
            .await
            .expect("global rate limit"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(custody(&pool).await, (120, 120, 120, 120));
    clock.at.store(1560, Ordering::SeqCst);
    for (profile, app) in apps.iter().take(4).enumerate() {
        for n in 30..50 {
            assert_eq!(
                app.clone()
                    .oneshot(request(format!("id=tr_GLOBAL_{profile}_{n}")))
                    .await
                    .expect("global capacity slot")
                    .status(),
                StatusCode::OK
            );
        }
    }
    clock.at.store(1620, Ordering::SeqCst);
    failure(
        apps[4]
            .clone()
            .oneshot(request("id=tr_GLOBAL_CAPACITY_OVER"))
            .await
            .expect("global capacity limit"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(custody(&pool).await, (200, 200, 200, 200));
    no_financial_effects(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn success_acknowledgement_waits_for_the_actual_deferred_commit(pool: PgPool) {
    pool.execute("CREATE FUNCTION callback_commit_wait_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext(current_database())::bigint); RETURN NULL; END $$; CREATE CONSTRAINT TRIGGER callback_commit_wait_fixture AFTER INSERT ON callback_intakes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION callback_commit_wait_fixture()")
        .await.expect("real deferred commit wait");
    let mut blocker = pool.begin().await.expect("commit blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext(current_database())::bigint)")
        .execute(&mut *blocker)
        .await
        .expect("hold commit barrier");
    let (app, _, _) = private_app(&pool);
    let task = tokio::spawn(app.oneshot(request("id=tr_COMMIT_BOUNDARY")));
    wait_for_lock(&pool, "COMMIT").await;
    assert!(
        !task.is_finished(),
        "no acknowledgement before actual commit completes"
    );
    assert_eq!(
        custody(&pool).await,
        (0, 0, 0, 0),
        "uncommitted source is invisible"
    );
    blocker.rollback().await.expect("release commit barrier");
    let response = task
        .await
        .expect("request task")
        .expect("committed response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(bytes(response).await.is_empty());
    assert_eq!(custody(&pool).await, (1, 1, 1, 1));
    no_financial_effects(&pool).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn global_http_deadline_cancels_a_real_admission_wait_without_a_false_ack(pool: PgPool) {
    let mut blocker = pool.begin().await.expect("quota blocker");
    sqlx::query("SELECT scope FROM callback_admission_quota WHERE scope='global' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("hold actual quota row");
    let (app, _, _) = private_app(&pool);
    let task = tokio::spawn(app.clone().oneshot(request("id=tr_TIMEOUT_PRIVATE")));
    wait_for_lock(&pool, "INSERT INTO callback_intakes").await;
    let response = task
        .await
        .expect("bounded request task")
        .expect("HTTP deadline response");
    let body = failure(response, StatusCode::GATEWAY_TIMEOUT).await;
    assert_eq!(body, "{\"code\":\"dependency_timeout\"}");
    assert_eq!(custody(&pool).await, (0, 0, 0, 0));
    blocker.rollback().await.expect("release quota");
    // Returning the cancelled transaction to the pool must roll it back before reuse.
    assert_eq!(
        app.clone()
            .oneshot(request("id=tr_TIMEOUT_RECOVERED"))
            .await
            .expect("healthy retry")
            .status(),
        StatusCode::OK
    );
    assert_eq!(custody(&pool).await, (1, 1, 1, 1));
    let retained: Vec<String> = sqlx::query_scalar("SELECT provider_id FROM callback_intakes")
        .fetch_all(&pool)
        .await
        .expect("retained source");
    assert_eq!(retained, vec!["tr_TIMEOUT_RECOVERED"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelled_admission_rolls_back_and_the_same_notification_can_retry(pool: PgPool) {
    let mut blocker = pool.begin().await.expect("quota blocker");
    sqlx::query("SELECT scope FROM callback_admission_quota WHERE scope='global' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("hold actual quota row");
    let (app, _, _) = private_app(&pool);
    let task = tokio::spawn(app.clone().oneshot(request("id=tr_CANCEL_RETRY")));
    wait_for_lock(&pool, "INSERT INTO callback_intakes").await;
    task.abort();
    assert!(task.await.expect_err("request cancelled").is_cancelled());
    assert_eq!(custody(&pool).await, (0, 0, 0, 0));
    blocker.rollback().await.expect("release quota");
    assert_eq!(
        app.clone()
            .oneshot(request("id=tr_CANCEL_RETRY"))
            .await
            .expect("same notification safely retries")
            .status(),
        StatusCode::OK
    );
    assert_eq!(custody(&pool).await, (1, 1, 1, 1));
    let admitted: i32 =
        sqlx::query_scalar("SELECT admitted FROM callback_admission_quota WHERE scope='global'")
            .fetch_one(&pool)
            .await
            .expect("quota once");
    assert_eq!(admitted, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_rewind_during_actual_admission_wait_leaves_no_source_or_ack(pool: PgPool) {
    let mut blocker = pool.begin().await.expect("quota blocker");
    sqlx::query("SELECT scope FROM callback_admission_quota WHERE scope='global' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .expect("hold actual quota row");
    let (app, clock, _) = private_app(&pool);
    let task = tokio::spawn(app.clone().oneshot(request("id=tr_CLOCK_REWIND")));
    wait_for_lock(&pool, "INSERT INTO callback_intakes").await;
    clock.at.store(1499, Ordering::SeqCst);
    blocker.rollback().await.expect("release quota");
    failure(
        task.await.expect("clock request").expect("rejected rewind"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(custody(&pool).await, (0, 0, 0, 0));
    clock.at.store(1500, Ordering::SeqCst);
    assert_eq!(
        app.clone()
            .oneshot(request("id=tr_CLOCK_REWIND"))
            .await
            .expect("clock recovery")
            .status(),
        StatusCode::OK
    );
    assert_eq!(custody(&pool).await, (1, 1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unavailable_clock_entropy_or_storage_errors_stay_static_and_retryable(pool: PgPool) {
    let (app, clock, _) = private_app(&pool);
    clock.fail.store(true, Ordering::SeqCst);
    failure(
        app.oneshot(request("id=tr_CLOCK_SECRET"))
            .await
            .expect("clock failure"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(custody(&pool).await, (0, 0, 0, 0));
    for fail_at in [2, 3] {
        let (app, _, ids) = private_app(&pool);
        ids.fail_at.store(fail_at, Ordering::SeqCst);
        let body = failure(
            app.oneshot(request("id=tr_ENTROPY_SECRET"))
                .await
                .expect("fresh ID port failure"),
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
        assert!(!body.contains("ENTROPY_SECRET"));
        assert_eq!(custody(&pool).await, (0, 0, 0, 0));
    }
    let (app, _, _) = private_app(&pool);
    pool.close().await;
    let body = failure(
        app.oneshot(request("id=tr_DATABASE_SECRET"))
            .await
            .expect("storage failure"),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    for secret in ["tr_DATABASE_SECRET", TOKEN, "postgres", "sqlx"] {
        assert!(!body.contains(secret));
    }
}
