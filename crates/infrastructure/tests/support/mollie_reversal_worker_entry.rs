//! Exercise the canonical worker composition with an authenticated loopback transport.
use super::*;
use crate::mollie::reversal_worker_fixture as executable;
use std::sync::Mutex;
use tracing::instrument::WithSubscriber;

type Fields = Vec<(String, String)>;
struct Record<'a>(&'a mut Fields);
impl tracing::field::Visit for Record<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().into(), format!("{value:?}")));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().into(), value.into()));
    }
}
struct Capture(Arc<Mutex<Vec<Fields>>>);
impl tracing::Subscriber for Capture {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }
    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::INFO)
    }
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields::new();
        event.record(&mut Record(&mut fields));
        self.0.lock().expect("test output").push(fields);
    }
}
fn settings(key: &str) -> executable::Settings {
    executable::Settings {
        key: key.into(),
        profile: "pfl_SYNTHETIC_NotARealProfile".into(),
        batch: Some("1".into()),
    }
}
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

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_worker_enrolls_runs_and_inspects_with_redacted_output(pool: PgPool) {
    let (clock, frozen, ids, observation) = fixture(&pool).await;
    let raw = raw_key();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let enrollment = executable::Enrollment::parse(
        &frozen.snapshot().binding.id,
        &observation,
        "refund",
        "operator-scan",
    )
    .expect("valid enrollment arguments");
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
    .expect("enroll through worker");
    assert_eq!(server.count(), 1, "enrollment verifies one profile");
    let job_id: String = sqlx::query_scalar("SELECT id FROM reversal_scan_jobs")
        .fetch_one(&pool)
        .await
        .expect("enrolled job");
    server.inspect(|requests| {
        assert_eq!(requests[0].path, "/v2/profiles/me");
        assert_eq!(requests[0].method, "GET");
        assert_eq!(
            requests[0].header("authorization"),
            Some(format!("Bearer {raw}").as_str())
        );
        assert!(requests[0].body.is_empty());
    });

    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(Some("re_NEXT")),
    ]);
    executable::configured_run(&pool, clock.as_ref(), &ids, settings(&raw), None, |key| {
        transport(&server, key)
    })
    .with_subscriber(Capture(logs.clone()))
    .await
    .expect("run through worker");
    assert_eq!(server.count(), 3, "one profile, paid parent and page");
    server.inspect(|requests| {
        assert_eq!(
            requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            vec![
                "/v2/profiles/me",
                "/v2/payments/tr_SYNTHETIC_NotARealPayment",
                "/v2/payments/tr_SYNTHETIC_NotARealPayment/refunds?limit=10"
            ]
        );
        for request in requests {
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.header("authorization"),
                Some(format!("Bearer {raw}").as_str())
            );
            assert!(request.body.is_empty());
        }
    });
    let checkpoint = read_job(&pool, &job_id)
        .await
        .expect("validated checkpoint")
        .expect("job");
    assert_eq!(checkpoint.state, "pending");
    assert_eq!(checkpoint.generation, 1);
    assert!(checkpoint.tip_page_id.is_some());
    executable::inspect(&pool, &job_id)
        .with_subscriber(Capture(logs.clone()))
        .await
        .expect("credential-free inspection");
    assert_eq!(server.count(), 3, "inspection performs no provider request");

    let events = logs.lock().expect("captured output");
    let field = |event: &Fields, name: &str, value: &str| {
        event.iter().any(|(k, v)| k == name && v == value)
    };
    assert!(
        events
            .iter()
            .any(|e| field(e, "code", "reversal_scan_enrolled") && field(e, "job_id", &job_id))
    );
    assert!(
        events
            .iter()
            .any(|e| field(e, "code", "reversal_page_batch") && field(e, "processed", "1"))
    );
    assert!(
        events
            .iter()
            .any(|e| field(e, "code", "reversal_scan_state")
                && field(e, "state", "pending")
                && field(e, "generation", "1")
                && field(e, "reason", "none"))
    );
    for event in events.iter() {
        for (_, value) in event {
            assert!(!value.contains(&raw));
            assert!(!value.contains(PAYMENT));
            assert!(!value.contains("pfl_SYNTHETIC_NotARealProfile"));
            assert!(!value.contains("re_NEXT"));
        }
    }
}
