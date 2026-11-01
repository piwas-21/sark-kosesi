//! Configuration and persistent operational counts have independent literal controls.
use super::*;
use std::sync::{Arc, Mutex};

type Fields = Vec<(String, String)>;
struct RecordFields<'a>(&'a mut Fields);
impl tracing::field::Visit for RecordFields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().into(), format!("{value:?}")));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().into(), value.into()));
    }
}
struct CaptureEvents(Arc<Mutex<Vec<Fields>>>);
impl tracing::Subscriber for CaptureEvents {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }
    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::TRACE)
    }
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
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
        event.record(&mut RecordFields(&mut fields));
        self.0.lock().expect("captured test events").push(fields);
    }
}

#[test]
fn callback_worker_logging_emits_only_the_redacted_code_and_literal_operational_counts() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(CaptureEvents(captured.clone()), || {
        tracing::callsite::rebuild_interest_cache();
        tracing::info!(unexpected_test_secret = "synthetic-not-a-real-credential");
        assert!(
            report(&CallbackBatch {
                processed: 2,
                drained: 3,
                needs_review: 0,
                retry_pending: 0
            })
            .is_ok()
        );
        assert_eq!(
            report(&CallbackBatch {
                processed: 2,
                drained: 3,
                needs_review: 1,
                retry_pending: 0
            })
            .expect_err("review remains nonzero")
            .to_string(),
            "callback reconciliation requires operator review"
        );
        assert_eq!(
            report(&CallbackBatch {
                processed: 0,
                drained: 0,
                needs_review: 0,
                retry_pending: 1
            })
            .expect_err("retry remains nonzero")
            .to_string(),
            "callback reconciliation requires retry"
        );
    });
    let events = captured.lock().expect("captured worker events");
    assert_eq!(
        events.len(),
        4,
        "capture calibration and all actual report outcomes"
    );
    assert_eq!(
        events[0],
        vec![(
            "unexpected_test_secret".into(),
            "synthetic-not-a-real-credential".into()
        )],
        "calibration detects unexpected sensitive-shaped fields without any allowlist"
    );
    for (index, processed, drained, review, retry) in [
        (1, "2", "3", "0", "0"),
        (2, "2", "3", "1", "0"),
        (3, "0", "0", "0", "1"),
    ] {
        let mut expected = Fields::from([
            ("code".into(), "callback_reconciliation_batch".into()),
            ("processed".into(), processed.into()),
            ("drained".into(), drained.into()),
            ("needs_review".into(), review.into()),
            ("retry_pending".into(), retry.into()),
        ]);
        let mut actual = events[index].clone();
        actual.sort();
        expected.sort();
        assert_eq!(
            actual, expected,
            "every real event field is retained, including repeated or unexpected fields"
        );
    }
}
#[test]
fn callback_configuration_accepts_only_test_credentials_and_fixed_batch_bounds() {
    let key = format!("test_{}", "synthetic".repeat(4));
    assert_eq!(
        capture(&key, "pfl_SYNTHETIC", None)
            .expect("default bounded configuration")
            .1,
        10
    );
    for (raw, expected) in [("1", 1), ("25", 25)] {
        assert_eq!(
            capture(&key, "pfl_SYNTHETIC", Some(raw))
                .expect("literal batch edge")
                .1,
            expected
        );
    }
    for raw in ["0", "26", "invalid", "65536"] {
        assert!(capture(&key, "pfl_SYNTHETIC", Some(raw)).is_err());
    }
    for invalid in ["live_DO_NOT_USE", "test_short"] {
        let error = capture(invalid, "pfl_SYNTHETIC", None)
            .err()
            .expect("reject live/malformed key")
            .to_string();
        assert!(!error.contains(invalid));
    }
    assert!(capture(&key, "wrong", None).is_err());
}
#[test]
fn unchanged_review_or_retry_counts_never_report_a_healthy_worker_pass() {
    assert!(report(&CallbackBatch::default()).is_ok());
    assert!(
        report(&CallbackBatch {
            processed: 1,
            drained: 2,
            needs_review: 0,
            retry_pending: 0
        })
        .is_ok()
    );
    assert_eq!(
        report(&CallbackBatch {
            processed: 0,
            drained: 0,
            needs_review: 1,
            retry_pending: 0
        })
        .expect_err("unchanged review remains visible")
        .to_string(),
        "callback reconciliation requires operator review"
    );
    assert_eq!(
        report(&CallbackBatch {
            processed: 0,
            drained: 0,
            needs_review: 0,
            retry_pending: 1
        })
        .expect_err("unchanged retry remains visible")
        .to_string(),
        "callback reconciliation requires retry"
    );
}
