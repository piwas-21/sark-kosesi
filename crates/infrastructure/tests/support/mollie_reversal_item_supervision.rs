//! Isolated PostgreSQL and authenticated loopback item-supervision controls.
use super::*;
use crate::payment_attempts::{
    reversal_item_supervision::{EnrollIntent, enroll, read_job, run_batch},
    reversal_pages,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[path = "mollie_reversal_item_enrollment.rs"]
mod enrollment_cases;
#[path = "mollie_reversal_item_worker_entry.rs"]
mod worker_entry;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";
fn collection(kind: ReversalKind) -> &'static str {
    match kind {
        ReversalKind::Refund => "refunds",
        ReversalKind::Chargeback => "chargebacks",
    }
}
fn child_id(kind: ReversalKind) -> &'static str {
    match kind {
        ReversalKind::Refund => "re_ITEM",
        ReversalKind::Chargeback => "chb_ITEM",
    }
}
fn child(kind: ReversalKind) -> Value {
    match kind {
        ReversalKind::Refund => {
            json!({"resource":"refund","id":child_id(kind),"paymentId":PAYMENT,"mode":"test","amount":{"currency":"EUR","value":"12.34"},"status":"pending","createdAt":"2024-03-20T09:13:37Z"})
        }
        ReversalKind::Chargeback => {
            json!({"resource":"chargeback","id":child_id(kind),"paymentId":PAYMENT,"amount":{"currency":"EUR","value":"12.34"},"createdAt":"2024-03-20T09:13:37Z"})
        }
    }
}
pub(crate) async fn verified_test_item(
    attempt: &AttemptSnapshot,
    kind: ReversalKind,
) -> VerifiedReversal {
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(attempt)),
        Reply::Json(200, child(kind)),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    match kind {
        ReversalKind::Refund => client.fetch_refund(attempt, PAYMENT, child_id(kind)).await,
        ReversalKind::Chargeback => {
            client
                .fetch_chargeback(attempt, PAYMENT, child_id(kind))
                .await
        }
    }
    .expect("verified item")
}
pub(crate) async fn page_fixture(
    pool: &PgPool,
    time: &dyn commerce_application::request_context::Clock,
    ids: &dyn commerce_application::request_context::Identifiers,
    attempt: &AttemptSnapshot,
    kind: ReversalKind,
    effect: &str,
) -> String {
    let item = match kind {
        ReversalKind::Refund => {
            json!({"resource":"refund","id":child_id(kind),"paymentId":PAYMENT,"mode":"test"})
        }
        ReversalKind::Chargeback => {
            json!({"resource":"chargeback","id":child_id(kind),"paymentId":PAYMENT})
        }
    };
    let page = json!({"count":1,"_embedded":{(collection(kind)):[item]},"_links":{"next":null}});
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(attempt)),
        Reply::Json(200, page),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let verified = client
        .reversal_page(attempt, PAYMENT, kind, None)
        .await
        .expect("page");
    reversal_pages::record(pool, time, ids, effect, None, &verified)
        .await
        .expect("page receipt")
        .id
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_page_candidates_of_both_kinds_produce_separate_observations(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let before = crate::payment_test_fixture::counts(&pool).await;
    for kind in [ReversalKind::Refund, ReversalKind::Chargeback] {
        let page = page_fixture(
            &pool,
            time.as_ref(),
            &ids,
            frozen.snapshot(),
            kind,
            collection(kind),
        )
        .await;
        let server = Server::start(vec![
            profile(),
            Reply::Json(200, payment(frozen.snapshot())),
            Reply::Json(200, child(kind)),
        ]);
        let client = local(&server)
            .verify_profile()
            .await
            .expect("profile")
            .into_reconciliation();
        let intent = |effect| EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: effect,
        };
        let job = enroll(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            intent(collection(kind)),
        )
        .await
        .expect("enrollment");
        assert!(
            enroll(
                &pool,
                time.as_ref(),
                &ids,
                &client,
                EnrollIntent {
                    ordinal: 2,
                    ..intent("bad-ordinal")
                }
            )
            .await
            .is_err()
        );
        assert!(
            run_batch(&pool, time.as_ref(), &ids, &client, 0)
                .await
                .is_err()
        );
        assert!(
            run_batch(&pool, time.as_ref(), &ids, &client, 26)
                .await
                .is_err()
        );
        assert_eq!(server.count(), 1);
        assert_eq!(
            run_batch(&pool, time.as_ref(), &ids, &client, 25)
                .await
                .expect("batch")
                .processed,
            1
        );
        let saved = read_job(&pool, &job.id)
            .await
            .expect("valid job")
            .expect("present");
        assert_eq!(saved.child_id, child_id(kind));
        let receipt: (String, String) = sqlx::query_as("SELECT child_id,payment_id FROM payment_reversal_observations WHERE attempt_id=$1 AND child_id=$2")
            .bind(&job.attempt_id).bind(child_id(kind)).fetch_one(&pool).await.expect("exact receipt");
        assert_eq!(receipt, (child_id(kind).into(), PAYMENT.into()));
        server.inspect(|requests| {
            assert_eq!(requests.len(), 3);
            for request in requests {
                assert_eq!(request.method, "GET");
                assert!(request.body.is_empty());
                assert_eq!(
                    request.header("Authorization"),
                    Some(format!("Bearer {}", raw_key()).as_str())
                );
            }
            assert_eq!(requests[1].path, format!("/v2/payments/{PAYMENT}"));
            assert_eq!(
                requests[2].path,
                format!(
                    "/v2/payments/{PAYMENT}/{}/{}",
                    collection(kind),
                    child_id(kind)
                )
            );
        });
    }
    let after = crate::payment_test_fixture::counts(&pool).await;
    assert_eq!(
        (after.0, after.2, after.3, after.4),
        (before.0, before.2, before.3, before.4)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelled_get_parks_uncertain_without_repeating_io(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
        Reply::Json(200, child(ReversalKind::Refund)),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    {
        let mut running = Box::pin(run_batch(&pool, time.as_ref(), &ids, &client, 1));
        tokio::select! {
            result = &mut running => panic!("unexpected completion: {}", result.is_ok()),
            () = async { tokio::time::timeout(Duration::from_secs(2), async { while server.count() < 2 { tokio::time::sleep(Duration::from_millis(2)).await; } }).await.expect("GET entered"); } => {}
        }
    }
    gate.store(true, Ordering::SeqCst);
    time.0.store(1561, Ordering::SeqCst);
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("recover");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("job")
        .expect("present");
    assert_eq!(stopped.review_reason.as_deref(), Some("uncertain"));
    assert_eq!(server.count(), 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_receipt_is_recovered_without_another_request(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::GateJson(gate.clone(), child(ReversalKind::Refund)),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    let first = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let expire = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 3 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("child GET entered");
        time.0.store(1561, Ordering::SeqCst);
        gate.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(first, expire);
    assert!(result.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reversal_observations")
        .fetch_one(&pool)
        .await
        .expect("receipt");
    assert_eq!(count, 1);
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("receipt recovery");
    let saved = read_job(&pool, &job.id)
        .await
        .expect("job")
        .expect("present");
    assert_ne!(saved.review_reason.as_deref(), Some("uncertain"));
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unavailable_retries_are_bounded_and_malformed_response_is_reviewed(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    for expected in 1..=3 {
        run_batch(&pool, time.as_ref(), &ids, &client, 25)
            .await
            .expect("bounded pass");
        let saved = read_job(&pool, &job.id)
            .await
            .expect("job")
            .expect("present");
        assert_eq!(saved.failures, expected);
        if expected < 3 {
            assert_eq!(saved.state, "pending");
            time.0.store(saved.due_at as u64, Ordering::SeqCst);
        } else {
            assert_eq!(saved.review_reason.as_deref(), Some("retry_exhausted"));
        }
    }
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("stopped pass");
    assert_eq!(server.count(), 4);

    let second = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page-two",
    )
    .await;
    let malformed = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, json!({})),
    ]);
    let client = local(&malformed)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &second,
            ordinal: 1,
            enrollment_key: "item-two",
        },
    )
    .await
    .expect("distinct page");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("malformed outcome");
    let saved = read_job(&pool, &job.id)
        .await
        .expect("job")
        .expect("present");
    assert_eq!(saved.review_reason.as_deref(), Some("invalid"));
    assert_eq!(malformed.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exhausted_observation_capacity_stops_before_provider_io(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, child(ReversalKind::Refund)),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let verified = client
        .fetch_refund(frozen.snapshot(), PAYMENT, child_id(ReversalKind::Refund))
        .await
        .expect("fixture");
    for i in 0..128 {
        crate::payment_attempts::reversals::record(
            &pool,
            time.as_ref(),
            &ids,
            &format!("capacity-{i}"),
            &verified,
        )
        .await
        .expect("receipt");
    }
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("capacity review");
    let saved = read_job(&pool, &job.id)
        .await
        .expect("job")
        .expect("present");
    assert_eq!(saved.review_reason.as_deref(), Some("observation_limit"));
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn receipt_arriving_after_uncertain_review_remains_visible_without_advancing_job(
    pool: PgPool,
) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::GateJson(gate.clone(), child(ReversalKind::Refund)),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    let first = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let park = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 3 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("child GET entered");
        time.0.store(1561, Ordering::SeqCst);
        run_batch(&pool, time.as_ref(), &ids, &client, 1)
            .await
            .expect("park uncertain");
        let saved = read_job(&pool, &job.id)
            .await
            .expect("job")
            .expect("present");
        assert_eq!(saved.review_reason.as_deref(), Some("uncertain"));
        assert!(
            crate::payment_attempts::reversal_item_supervision::read_late_receipt(&pool, &job.id)
                .await
                .expect("inspect")
                .is_none()
        );
        gate.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(first, park);
    assert!(result.is_err());
    let receipt =
        crate::payment_attempts::reversal_item_supervision::read_late_receipt(&pool, &job.id)
            .await
            .expect("valid late receipt")
            .expect("present");
    assert_eq!(receipt.payment_id, PAYMENT);
    assert_eq!(receipt.child_id, child_id(ReversalKind::Refund));
    assert_eq!(receipt.attempt_id, job.attempt_id);
    assert_eq!(receipt.credential_fingerprint, job.credential_fingerprint);
    let saved = read_job(&pool, &job.id)
        .await
        .expect("job")
        .expect("present");
    assert_eq!(saved.review_reason.as_deref(), Some("uncertain"));
    assert!(saved.receipt_id.is_none());
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_enrollment_replays_and_concurrent_workers_do_not_duplicate_gets(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
        Reply::Json(200, child(ReversalKind::Refund)),
    ]);
    // This fixture deliberately holds the parent GET while another worker runs.
    // Give that hold its own deadline without changing the normal test client.
    let key = credential();
    let configuration = config(&key);
    let client = TestClient::local(
        key,
        configuration,
        server.base.clone(),
        Duration::from_secs(60),
    )
    .expect("loopback only client")
    .verify_profile()
    .await
    .expect("profile")
    .into_reconciliation();
    let intent = || EnrollIntent {
        attempt_id: &frozen.snapshot().binding.id,
        observation_id: &paid,
        page_id: &page,
        ordinal: 1,
        enrollment_key: "item",
    };
    let job = enroll(&pool, time.as_ref(), &ids, &client, intent())
        .await
        .expect("enroll");
    let replay = enroll(&pool, time.as_ref(), &ids, &client, intent())
        .await
        .expect("exact replay");
    assert_eq!(replay.id, job.id);
    let second = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page-two",
    )
    .await;
    assert!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            EnrollIntent {
                page_id: &second,
                ..intent()
            }
        )
        .await
        .is_err()
    );
    let mut first = Box::pin(run_batch(&pool, time.as_ref(), &ids, &client, 1));
    tokio::select! {
        result = &mut first => match result {
            Ok(batch) => panic!("first worker completed before held GET: {} processed", batch.processed),
            Err(error) => panic!("first worker failed before held GET: {error:?}"),
        },
        () = async {
            tokio::time::timeout(Duration::from_secs(20), async {
                while server.count() < 2 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .expect("parent GET entered");
        } => {}
    }
    // Keep polling the held request past the normal five-second read deadline.
    tokio::select! {
        result = &mut first => match result {
            Ok(batch) => panic!("first worker completed during held GET: {} processed", batch.processed),
            Err(error) => panic!("first worker failed during held GET: {error:?}"),
        },
        () = tokio::time::sleep(Duration::from_secs(6)) => {}
    }
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids, &client, 1)
            .await
            .expect("second worker")
            .processed,
        0
    );
    assert_eq!(server.count(), 2);
    gate.store(true, Ordering::SeqCst);
    assert_eq!(first.await.expect("first worker").processed, 1);
    assert_eq!(server.count(), 3);
    assert_eq!(
        read_job(&pool, &job.id)
            .await
            .expect("job")
            .expect("present")
            .state,
        "observed"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotated_key_cannot_enroll_or_claim_page_bound_to_original_key(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let intent = || EnrollIntent {
        attempt_id: &frozen.snapshot().binding.id,
        observation_id: &paid,
        page_id: &page,
        ordinal: 1,
        enrollment_key: "item",
    };
    enroll(&pool, time.as_ref(), &ids, &client, intent())
        .await
        .expect("enroll");
    let key = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(5)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("rotated");
    let configuration = config(&key);
    let rotated_server = Server::start(vec![profile()]);
    let rotated = TestClient::local(
        key,
        configuration,
        rotated_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("same profile")
    .into_reconciliation();
    assert!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &rotated,
            EnrollIntent {
                enrollment_key: "other",
                ..intent()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids, &rotated, 25)
            .await
            .expect("no matching jobs")
            .processed,
        0
    );
    assert_eq!(rotated_server.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lease_expiring_before_io_sends_no_request(pool: PgPool) {
    use commerce_application::request_context::{Clock, ContextError};
    use std::sync::atomic::AtomicU64;
    struct ExpiringClock(AtomicU64);
    impl Clock for ExpiringClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(if self.0.fetch_add(1, Ordering::SeqCst) < 3 {
                1500
            } else {
                1560
            })
        }
    }
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    assert!(
        run_batch(&pool, &observation_cases::BadPorts, &ids, &client, 1)
            .await
            .is_err()
    );
    assert!(
        run_batch(&pool, &ExpiringClock(AtomicU64::new(0)), &ids, &client, 1)
            .await
            .is_err()
    );
    assert_eq!(server.count(), 1);
    time.0.store(1560, Ordering::SeqCst);
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("park");
    assert_eq!(
        read_job(&pool, &job.id)
            .await
            .expect("job")
            .expect("present")
            .review_reason
            .as_deref(),
        Some("uncertain")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_observation_write_below_capacity_stops_as_invalid(pool: PgPool) {
    use sqlx::Executor;
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, child(ReversalKind::Refund)),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    pool.execute("CREATE FUNCTION suppress_item_observation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").await.expect("function");
    pool.execute("CREATE TRIGGER suppress_item_observation BEFORE INSERT ON payment_reversal_observations FOR EACH ROW EXECUTE FUNCTION suppress_item_observation()").await.expect("trigger");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("review");
    pool.execute("DROP TRIGGER suppress_item_observation ON payment_reversal_observations")
        .await
        .expect("restore");
    assert_eq!(
        read_job(&pool, &job.id)
            .await
            .expect("job")
            .expect("present")
            .review_reason
            .as_deref(),
        Some("invalid")
    );
    assert_eq!(server.count(), 3);
}
