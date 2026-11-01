//! Execute the actual worker orchestration with the existing authenticated loopback port.
use super::*;
use crate::mollie::callback_worker_fixture as executable;
fn transport(server: &Server, key: TestCredential) -> Result<ReconciliationClient, MollieError> {
    Ok(ReconciliationClient {
        http: http::Http::build(
            server.base.clone(),
            key.authorization,
            Duration::from_secs(2),
        )?,
        profile: key.binding,
    })
}
fn settings(key: &str) -> executable::Settings {
    executable::Settings {
        key: key.into(),
        profile: PROFILE.into(),
        batch: Some("1".into()),
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_worker_loopback_verifies_profile_processes_open_and_reports_healthy_completion(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_WORKER_OPEN").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_WORKER_OPEN", "open")),
    ]);
    let raw = raw_key();
    executable::configured_run(&pool, clock.as_ref(), &i, settings(&raw), |key| {
        transport(&server, key)
    })
    .await
    .expect("real worker processing without network-origin override");
    assert!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .settled
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_worker_keeps_retry_and_finance_review_nonzero_after_successful_profile_verification(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_WORKER_RETRY").await;
    let raw = raw_key();
    let server = Server::start(vec![profile(), Reply::Json(503, json!({}))]);
    let error = executable::configured_run(&pool, clock.as_ref(), &i, settings(&raw), |key| {
        transport(&server, key)
    })
    .await
    .expect_err("worker reports retry");
    assert_eq!(error.to_string(), "callback reconciliation requires retry");
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_WORKER_PAID").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_WORKER_PAID", "paid")),
    ]);
    let error = executable::configured_run(&pool, clock.as_ref(), &i, settings(&raw), |key| {
        transport(&server, key)
    })
    .await
    .expect_err("worker reports finance review");
    assert_eq!(
        error.to_string(),
        "callback reconciliation requires operator review"
    );
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_worker_factory_profile_and_storage_failures_are_static_and_do_not_grant_fetch_authority(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    let raw = raw_key();
    let error = executable::configured_run(&pool, clock.as_ref(), &i, settings(&raw), |_| {
        Err(MollieError::InvalidConfiguration)
    })
    .await
    .expect_err("transport factory error");
    assert_eq!(error.to_string(), "test payment transport unavailable");
    let server = Server::start(vec![Reply::Json(
        200,
        json!({"resource":"profile","id":"pfl_WRONG"}),
    )]);
    let error = executable::configured_run(&pool, clock.as_ref(), &i, settings(&raw), |key| {
        transport(&server, key)
    })
    .await
    .expect_err("wrong authenticated profile");
    assert_eq!(
        error.to_string(),
        "test payment profile verification failed"
    );
    assert_eq!(intake_counts(&pool).await, (0, 0, 0));
    let server = Server::start(vec![profile()]);
    pool.close().await;
    let error = executable::configured_run(&pool, clock.as_ref(), &i, settings(&raw), |key| {
        transport(&server, key)
    })
    .await
    .expect_err("closed storage");
    assert_eq!(error.to_string(), "callback reconciliation unavailable");
    assert!(!error.to_string().contains(&raw));
}
