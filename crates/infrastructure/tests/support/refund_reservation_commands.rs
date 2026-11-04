//! Command boundary and transactional fault controls against accepted payment custody.
use super::*;
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use std::sync::atomic::AtomicU64;

struct FailingClock {
    successful_reads: u64,
    calls: AtomicU64,
}
impl Clock for FailingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.successful_reads {
            Ok(1500)
        } else {
            Err(ContextError::ClockUnavailable)
        }
    }
}
struct OneIdentifier(AtomicU64);
impl Identifiers for OneIdentifier {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok([0x71; 16])
        } else {
            Err(ContextError::EntropyUnavailable)
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_commands_fail_without_reservation_and_valid_control_succeeds(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let long_reason = "x".repeat(241);
    let long_sku = "x".repeat(66);
    let long_key = "x".repeat(129);
    let too_many: Vec<_> = (0..21)
        .map(|_| RefundLine {
            sku: "x",
            gross_minor: 0,
        })
        .collect();
    let empty_sku = [RefundLine {
        sku: "",
        gross_minor: 1,
    }];
    let oversized_sku = [RefundLine {
        sku: &long_sku,
        gross_minor: 1,
    }];
    let negative_line = [RefundLine {
        sku: "SET-COVERS",
        gross_minor: -1,
    }];
    let invalid: Vec<RefundRequest<'_>> = vec![
        request("invalid", &proof, "key", &lines, 0),
        request(order, "invalid", "key", &lines, 0),
        request(order, &proof, "", &lines, 0),
        request(order, &proof, &long_key, &lines, 0),
        request(order, &proof, "bad key", &lines, 0),
        RefundRequest {
            confirmed: false,
            ..request(order, &proof, "key", &lines, 0)
        },
        RefundRequest {
            reason: "",
            ..request(order, &proof, "key", &lines, 0)
        },
        RefundRequest {
            reason: &long_reason,
            ..request(order, &proof, "key", &lines, 0)
        },
        RefundRequest {
            reason: " leading",
            ..request(order, &proof, "key", &lines, 0)
        },
        RefundRequest {
            reason: "private\ntext",
            ..request(order, &proof, "key", &lines, 0)
        },
        request(order, &proof, "key", &[], 0),
        request(order, &proof, "key", &too_many, 0),
        request(order, &proof, "key", &lines, -1),
        request(order, &proof, "key", &empty_sku, 0),
        request(order, &proof, "key", &oversized_sku, 0),
        request(order, &proof, "key", &negative_line, 0),
    ];
    for input in invalid {
        assert_eq!(
            reserve(&pool, &token, f.time.as_ref(), f.ids.as_ref(), input)
                .await
                .err(),
            Some(RefundError::InvalidRequest)
        );
    }
    assert_eq!(counts(&pool).await, (0, 0));
    reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "valid-control", &lines, 0),
    )
    .await
    .expect("valid confirmed command");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_replay_requires_all_original_intent_and_current_grant(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let reversed = [
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
    ];
    let wrong_sku = [
        RefundLine {
            sku: "OTHER",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let original = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "intent-key", &lines, 0),
    )
    .await
    .expect("initial reservation");
    let reordered = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "intent-key", &reversed, 0),
    )
    .await
    .expect("order-independent exact replay");
    assert!(reordered.replayed);
    assert_eq!(reordered.id, original.id);
    let other_id = "ffffffffffffffffffffffffffffffff";
    assert_eq!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, other_id, "new-key-wrong-proof", &lines, 0),
        )
        .await
        .err(),
        Some(RefundError::Conflict)
    );
    let changed = [
        request(other_id, &proof, "intent-key", &lines, 0),
        request(order, other_id, "intent-key", &lines, 0),
        request(order, &proof, "intent-key", &lines, 1),
        request(order, &proof, "intent-key", &wrong_sku, 0),
        RefundRequest {
            reason: "Another reason",
            ..request(order, &proof, "intent-key", &lines, 0)
        },
    ];
    for input in changed {
        assert_eq!(
            reserve(&pool, &token, f.time.as_ref(), f.ids.as_ref(), input)
                .await
                .err(),
            Some(RefundError::Conflict)
        );
    }
    sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='refund-finance' AND capability='refund.prepare'").execute(&pool).await.expect("revoke finance grant");
    assert_eq!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "intent-key", &lines, 0)
        )
        .await
        .err(),
        Some(RefundError::Denied)
    );
    sqlx::query("INSERT INTO staff_capabilities VALUES('refund-finance','refund.prepare')")
        .execute(&pool)
        .await
        .expect("restore finance grant");
    assert!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "intent-key", &lines, 0)
        )
        .await
        .expect("restored authority and exact intent")
        .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_and_event_identity_failure_roll_back_and_receipt_diagnostics_are_redacted(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let before = footprint(&pool).await;
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    for successful_reads in 0..=3 {
        let clock = FailingClock {
            successful_reads,
            calls: AtomicU64::new(0),
        };
        assert_eq!(
            reserve(
                &pool,
                &token,
                &clock,
                f.ids.as_ref(),
                request(order, &proof, "fault-control", &lines, 0)
            )
            .await
            .err(),
            Some(RefundError::ClockUnavailable)
        );
        assert_eq!(clock.calls.load(Ordering::SeqCst), successful_reads + 1);
        assert_eq!(counts(&pool).await, (0, 0));
        assert_eq!(footprint(&pool).await, before);
    }
    let ids = OneIdentifier(AtomicU64::new(0));
    assert_eq!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            &ids,
            request(order, &proof, "fault-control", &lines, 0)
        )
        .await
        .err(),
        Some(RefundError::EntropyUnavailable)
    );
    assert_eq!(ids.0.load(Ordering::SeqCst), 2);
    assert_eq!(counts(&pool).await, (0, 0));
    assert_eq!(footprint(&pool).await, before);
    let reason = "private-customer-reason-marker";
    let receipt = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        RefundRequest {
            reason,
            ..request(order, &proof, "fault-control", &lines, 0)
        },
    )
    .await
    .expect("healthy retry after fault rollback");
    let diagnostic = format!("{receipt:?}");
    assert!(diagnostic.contains("[redacted]"));
    assert!(!diagnostic.contains(reason));
    assert!(!diagnostic.contains("refund-finance"));
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM commerce_outbox WHERE kind='order.refund_reserved.v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("minimal event");
    let object = payload.as_object().expect("object event");
    assert_eq!(object.len(), 4);
    for key in [
        "schema_version",
        "order_id",
        "reservation_id",
        "reservation_sha256",
    ] {
        assert!(object.contains_key(key));
    }
    assert!(!payload.to_string().contains(reason));
    assert!(!payload.to_string().contains("refund-finance"));
    assert_eq!(counts(&pool).await, (1, 1));
}
