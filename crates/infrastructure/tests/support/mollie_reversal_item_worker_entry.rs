//! The actual worker source composes enrollment, bounded fetch and inspection.
use super::super::reversal_supervision_cases::worker_entry::{Capture, Fields, transport};
use super::*;
use crate::mollie::reversal_item_worker_fixture as executable;
use std::sync::Mutex;
use tracing::instrument::WithSubscriber;

fn settings(key: &str) -> executable::Settings {
    executable::Settings {
        key: key.into(),
        profile: "pfl_SYNTHETIC_NotARealProfile".into(),
        batch: Some("1".into()),
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_worker_enrolls_runs_and_inspects_without_exposing_provider_data(pool: PgPool) {
    let (clock, frozen, ids, observation) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        clock.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let raw = raw_key();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let enrollment = executable::Enrollment::parse(
        &frozen.snapshot().binding.id,
        &observation,
        &page,
        "1",
        "operator-item",
    )
    .expect("valid enrollment");
    let server = Server::start(vec![profile()]);
    executable::configured_run(
        &pool,
        clock.as_ref(),
        &ids,
        settings(&raw),
        Some(enrollment),
        |key| transport(&server, key),
    )
    .with_subscriber(Capture(logs.clone()))
    .await
    .expect("enrollment");
    let job_id: String = sqlx::query_scalar("SELECT id FROM reversal_item_jobs")
        .fetch_one(&pool)
        .await
        .expect("job");
    assert_eq!(server.count(), 1);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, child(ReversalKind::Refund)),
    ]);
    executable::configured_run(&pool, clock.as_ref(), &ids, settings(&raw), None, |key| {
        transport(&server, key)
    })
    .with_subscriber(Capture(logs.clone()))
    .await
    .expect("batch");
    executable::inspect(&pool, &job_id)
        .with_subscriber(Capture(logs.clone()))
        .await
        .expect("inspection");
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.method, "GET");
            assert!(request.body.is_empty());
            assert_eq!(
                request.header("authorization"),
                Some(format!("Bearer {raw}").as_str())
            );
        }
        assert_eq!(
            requests[2].path,
            format!(
                "/v2/payments/{PAYMENT}/refunds/{}",
                child_id(ReversalKind::Refund)
            )
        );
    });
    let events = logs.lock().expect("logs");
    let field = |event: &Fields, name: &str, value: &str| {
        event.iter().any(|(k, v)| k == name && v == value)
    };
    assert!(
        events
            .iter()
            .any(|e| field(e, "code", "reversal_item_enrolled") && field(e, "job_id", &job_id))
    );
    assert!(
        events
            .iter()
            .any(|e| field(e, "code", "reversal_item_batch") && field(e, "processed", "1"))
    );
    assert!(
        events
            .iter()
            .any(|e| field(e, "code", "reversal_item_state")
                && field(e, "state", "observed")
                && field(e, "late_observation_available", "false"))
    );
    for event in events.iter() {
        for (_, value) in event {
            assert!(!value.contains(&raw));
            assert!(!value.contains(PAYMENT));
            assert!(!value.contains("pfl_SYNTHETIC_NotARealProfile"));
            assert!(!value.contains(child_id(ReversalKind::Refund)));
        }
    }
}
