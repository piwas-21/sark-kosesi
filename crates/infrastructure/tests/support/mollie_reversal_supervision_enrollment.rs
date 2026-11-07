//! Enrollment and read controls over isolated payment custody.
use super::*;
use crate::payment_attempts::reversal_supervision::RecoveryError;
use commerce_application::request_context::{Clock, ContextError};
use std::sync::atomic::AtomicUsize;

fn request<'a>(
    attempt: &'a str,
    observation_id: &'a str,
    kind: ReversalKind,
    enrollment_key: &'a str,
) -> EnrollIntent<'a> {
    EnrollIntent {
        attempt_id: attempt,
        observation_id,
        kind,
        enrollment_key,
    }
}

struct RegressingClock(AtomicUsize);
impl Clock for RegressingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            1500
        } else {
            1499
        })
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn enrollment_validates_sources_replay_and_conflicts(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let ids = observation_cases::ids();
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let open = observation_cases::fetched(frozen.snapshot(), PAYMENT, "open", false).await;
    let paid_fact = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let open = record_verified(&pool, time.as_ref(), &ids, attempt, "open", &open)
        .await
        .expect("open");
    let paid = record_verified(&pool, time.as_ref(), &ids, attempt, "paid", &paid_fact)
        .await
        .expect("paid");
    let paid_again = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        attempt,
        "paid-again",
        &paid_fact,
    )
    .await
    .expect("second fact");
    for (bad_attempt, bad_observation, bad_key) in [
        ("invalid", paid.snapshot.id.as_str(), "valid"),
        (attempt.as_str(), "invalid", "valid"),
        (attempt.as_str(), paid.snapshot.id.as_str(), "bad key"),
    ] {
        assert!(matches!(
            enroll(
                &pool,
                time.as_ref(),
                &ids,
                &client,
                EnrollIntent {
                    attempt_id: bad_attempt,
                    observation_id: bad_observation,
                    kind: ReversalKind::Refund,
                    enrollment_key: bad_key
                }
            )
            .await,
            Err(RecoveryError::InvalidRequest)
        ));
    }
    assert!(matches!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            request(
                &"e".repeat(32),
                &paid.snapshot.id,
                ReversalKind::Refund,
                "missing-attempt"
            )
        )
        .await,
        Err(RecoveryError::InvalidStoredData)
    ));
    assert!(matches!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            request(attempt, &"f".repeat(32), ReversalKind::Refund, "missing")
        )
        .await,
        Err(RecoveryError::InvalidStoredData)
    ));
    assert!(matches!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            request(attempt, &open.snapshot.id, ReversalKind::Refund, "open")
        )
        .await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(matches!(
        read_job(&pool, "invalid").await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(
        read_job(&pool, &"f".repeat(32))
            .await
            .expect("missing read")
            .is_none()
    );

    time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &client,
            request(attempt, &paid.snapshot.id, ReversalKind::Refund, "clock")
        )
        .await,
        Err(RecoveryError::ClockUnavailable)
    ));
    time.0.store(1500, Ordering::SeqCst);
    let clock = RegressingClock(AtomicUsize::new(0));
    assert!(matches!(
        enroll(
            &pool,
            &clock,
            &ids,
            &client,
            request(attempt, &paid.snapshot.id, ReversalKind::Refund, "regress")
        )
        .await,
        Err(RecoveryError::ClockUnavailable)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM reversal_scan_jobs")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0, "failed clock checks roll back admission");
    let first = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        request(attempt, &paid.snapshot.id, ReversalKind::Refund, "stable"),
    )
    .await
    .expect("enroll");
    assert_eq!(first.due_at, 1500);
    let replay = enroll(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        &client,
        request(attempt, &paid.snapshot.id, ReversalKind::Refund, "stable"),
    )
    .await
    .expect("replay requires neither clock nor entropy");
    assert!(first == replay);
    for (source, kind) in [
        (&paid.snapshot.id, ReversalKind::Chargeback),
        (&paid_again.snapshot.id, ReversalKind::Refund),
    ] {
        assert!(matches!(
            enroll(
                &pool,
                time.as_ref(),
                &ids,
                &client,
                request(attempt, source, kind, "stable")
            )
            .await,
            Err(RecoveryError::Conflict)
        ));
    }
    let rotated_key = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(5)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("key");
    let rotated_server = Server::start(vec![profile()]);
    let rotated_config = config(&rotated_key);
    let rotated = TestClient::local(
        rotated_key,
        rotated_config,
        rotated_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("local")
    .verify_profile()
    .await
    .expect("profile")
    .into_reconciliation();
    assert!(matches!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &rotated,
            request(attempt, &paid.snapshot.id, ReversalKind::Refund, "stable")
        )
        .await,
        Err(RecoveryError::Conflict)
    ));
    let other_key = TestCredential::capture(
        &format!("test_{}", "another".repeat(5)),
        "pfl_OTHER_PROFILE",
    )
    .expect("key");
    let other_server = Server::start(vec![Reply::Json(
        200,
        json!({"resource":"profile","id":"pfl_OTHER_PROFILE"}),
    )]);
    let other_config = config(&other_key);
    let other = TestClient::local(
        other_key,
        other_config,
        other_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("local")
    .verify_profile()
    .await
    .expect("other profile")
    .into_reconciliation();
    assert!(matches!(
        enroll(
            &pool,
            time.as_ref(),
            &ids,
            &other,
            request(
                attempt,
                &paid.snapshot.id,
                ReversalKind::Refund,
                "other-profile"
            )
        )
        .await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(
        read_job(&pool, &first.id)
            .await
            .expect("valid read")
            .is_some_and(|job| job == first)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_and_altered_enrollments_roll_back_before_positive_retry(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let attempt = &frozen.snapshot().binding.id;
    let paid = record_verified(&pool, time.as_ref(), &ids, attempt, "source", &paid)
        .await
        .expect("paid");
    let intent = || EnrollIntent {
        attempt_id: attempt,
        observation_id: &paid.snapshot.id,
        kind: ReversalKind::Refund,
        enrollment_key: "injected",
    };
    for (name, body) in [
        ("suppress", "RETURN NULL;"),
        ("alter", "NEW.enrollment_key := 'different'; RETURN NEW;"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE FUNCTION test_enrollment_{name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN {body} END $$"))).execute(&pool).await.expect("function");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER aaa_{name} BEFORE INSERT ON reversal_scan_jobs FOR EACH ROW EXECUTE FUNCTION test_enrollment_{name}()"))).execute(&pool).await.expect("trigger");
        assert!(matches!(
            enroll(&pool, time.as_ref(), &ids, &client, intent()).await,
            Err(RecoveryError::InvalidStoredData)
        ));
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP TRIGGER aaa_{name} ON reversal_scan_jobs"
        )))
        .execute(&pool)
        .await
        .expect("remove trigger");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM reversal_scan_jobs")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 0, "injected write rolled back");
    }
    let job = enroll(&pool, time.as_ref(), &ids, &client, intent())
        .await
        .expect("healthy retry");
    assert!(
        read_job(&pool, &job.id)
            .await
            .expect("healthy read")
            .is_some_and(|v| v == job)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_paid_source_or_event_denies_read_and_replay(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let attempt = &frozen.snapshot().binding.id;
    let paid = record_verified(&pool, time.as_ref(), &ids, attempt, "source", &paid)
        .await
        .expect("paid");
    let intent = || EnrollIntent {
        attempt_id: attempt,
        observation_id: &paid.snapshot.id,
        kind: ReversalKind::Refund,
        enrollment_key: "source-check",
    };
    let job = enroll(&pool, time.as_ref(), &ids, &client, intent())
        .await
        .expect("healthy enrollment");
    for (table, damage, restore) in [
        (
            "payment_observations",
            "UPDATE payment_observations SET status='open'",
            "UPDATE payment_observations SET status='paid'",
        ),
        (
            "commerce_outbox",
            "UPDATE commerce_outbox SET payload='{}'::jsonb WHERE kind='payment.observation_recorded.v1'",
            "UPDATE commerce_outbox SET payload=jsonb_build_object('schema_version',1,'order_id',(SELECT order_id FROM payment_attempts LIMIT 1),'attempt_id',(SELECT id FROM payment_attempts LIMIT 1),'observation_id',(SELECT id FROM payment_observations LIMIT 1)) WHERE kind='payment.observation_recorded.v1'",
        ),
    ] {
        let mut tx = pool.begin().await.expect("damage transaction");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} DISABLE TRIGGER USER"
        )))
        .execute(&mut *tx)
        .await
        .expect("disable guards");
        tx.execute(damage).await.expect("damage");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} ENABLE TRIGGER USER"
        )))
        .execute(&mut *tx)
        .await
        .expect("enable guards");
        tx.commit().await.expect("commit damage");
        assert!(matches!(
            read_job(&pool, &job.id).await,
            Err(RecoveryError::InvalidStoredData)
        ));
        assert!(
            enroll(
                &pool,
                &observation_cases::BadPorts,
                &observation_cases::BadPorts,
                &client,
                intent()
            )
            .await
            .is_err()
        );
        let mut tx = pool.begin().await.expect("restore transaction");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} DISABLE TRIGGER USER"
        )))
        .execute(&mut *tx)
        .await
        .expect("disable guards");
        tx.execute(restore).await.expect("restore");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} ENABLE TRIGGER USER"
        )))
        .execute(&mut *tx)
        .await
        .expect("enable guards");
        tx.commit().await.expect("commit repair");
        assert!(
            read_job(&pool, &job.id)
                .await
                .expect("restored custody")
                .is_some_and(|v| v == job)
        );
    }
}
