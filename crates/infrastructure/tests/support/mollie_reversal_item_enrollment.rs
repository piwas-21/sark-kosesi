use super::*;
use crate::payment_attempts::reversal_item_supervision::{RecoveryError, read_late_receipt};
use commerce_application::request_context::{Clock, ContextError};
use sqlx::Executor;
use std::sync::atomic::AtomicU64;

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
struct Rewind(AtomicU64);
impl Clock for Rewind {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            1501
        } else {
            1500
        })
    }
}

async fn refund_candidates(attempt: &AttemptSnapshot, count: usize) -> crate::mollie::ReversalPage {
    let candidates: Vec<_> = (0..count).map(|n| json!({"resource":"refund","id":format!("re_TEST{n}"),"paymentId":PAYMENT,"mode":"test"})).collect();
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(attempt)),
        Reply::Json(
            200,
            json!({"count":count,"_embedded":{"refunds":candidates},"_links":{"next":null}}),
        ),
    ]);
    local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation()
        .reversal_page(attempt, PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("authenticated candidates")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn item_enrollment_validates_locators_scope_replay_and_clock(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let attempt = &frozen.snapshot().binding.id;
    let page = refund_candidates(frozen.snapshot(), 2).await;
    let page = reversal_pages::record(&pool, time.as_ref(), &ids, "enroll-page", None, &page)
        .await
        .expect("ordered page")
        .id;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let missing = "f".repeat(32);
    let intent = || EnrollIntent {
        attempt_id: attempt,
        observation_id: &paid,
        page_id: &page,
        ordinal: 1,
        enrollment_key: "item",
    };
    for bad in [
        EnrollIntent {
            attempt_id: "bad",
            ..intent()
        },
        EnrollIntent {
            observation_id: "bad",
            ..intent()
        },
        EnrollIntent {
            page_id: "bad",
            ..intent()
        },
        EnrollIntent {
            page_id: &missing,
            ..intent()
        },
        EnrollIntent {
            ordinal: 0,
            ..intent()
        },
        EnrollIntent {
            ordinal: 11,
            ..intent()
        },
        EnrollIntent {
            enrollment_key: "bad key",
            ..intent()
        },
    ] {
        assert!(matches!(
            enroll(&pool, time.as_ref(), &ids, &client, bad).await,
            Err(RecoveryError::InvalidRequest)
        ));
    }
    assert!(matches!(
        read_job(&pool, "bad").await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(matches!(
        read_late_receipt(&pool, "bad").await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(
        read_job(&pool, &missing)
            .await
            .expect("valid absent ID")
            .is_none()
    );
    assert!(
        read_late_receipt(&pool, &missing)
            .await
            .expect("absent late result")
            .is_none()
    );

    let profile_id = "pfl_ALTERNATE_PROFILE";
    let other_key =
        TestCredential::capture(&raw_key(), profile_id).expect("synthetic other profile");
    let other_config = config(&other_key);
    let other_server = Server::start(vec![Reply::Json(
        200,
        json!({"resource":"profile","id":profile_id}),
    )]);
    let other = TestClient::local(
        other_key,
        other_config,
        other_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("other verified profile")
    .into_reconciliation();
    assert!(matches!(
        enroll(&pool, time.as_ref(), &ids, &other, intent()).await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert_eq!(other_server.count(), 1);
    assert!(matches!(
        enroll(&pool, &At(1499), &ids, &client, intent()).await,
        Err(RecoveryError::ClockUnavailable)
    ));

    let paid_fact = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let later = crate::payment_attempts::observations::record_verified(
        &pool,
        &At(1502),
        &ids,
        attempt,
        "later-paid-item",
        &paid_fact,
    )
    .await
    .expect("later authenticated paid fact");
    assert!(matches!(
        enroll(
            &pool,
            &At(1501),
            &ids,
            &client,
            EnrollIntent {
                observation_id: &later.snapshot.id,
                ..intent()
            }
        )
        .await,
        Err(RecoveryError::ClockUnavailable)
    ));
    assert!(matches!(
        enroll(&pool, &Rewind(0.into()), &ids, &client, intent()).await,
        Err(RecoveryError::ClockUnavailable)
    ));
    let initial = enroll(&pool, &At(1502), &ids, &client, intent())
        .await
        .expect("exact enrollment");
    let replay = enroll(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        &client,
        intent(),
    )
    .await
    .expect("exact replay without clock/IDs");
    assert_eq!(replay.id, initial.id);
    for conflict in [
        EnrollIntent {
            observation_id: &later.snapshot.id,
            ..intent()
        },
        EnrollIntent {
            ordinal: 2,
            ..intent()
        },
        EnrollIntent {
            enrollment_key: "new-key-same-candidate",
            ..intent()
        },
    ] {
        assert!(matches!(
            enroll(&pool, &At(1502), &ids, &client, conflict).await,
            Err(RecoveryError::Conflict)
        ));
    }
    assert_eq!(
        server.count(),
        1,
        "enrollment does not fetch any provider resource"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn item_enrollment_readback_rejects_suppressed_or_changed_writes(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "item-write-page",
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
        enrollment_key: "fault-enrollment",
    };
    for function in [
        "CREATE OR REPLACE FUNCTION fixture_item_enroll() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$",
        "CREATE OR REPLACE FUNCTION fixture_item_enroll() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.enrollment_key='changed-item'; RETURN NEW; END; $$",
    ] {
        pool.execute(function).await.expect("fault function");
        pool.execute("CREATE TRIGGER zz_fixture_item_enroll BEFORE INSERT ON reversal_item_jobs FOR EACH ROW EXECUTE FUNCTION fixture_item_enroll()").await.expect("fault after native guard");
        assert!(matches!(
            enroll(&pool, time.as_ref(), &ids, &client, intent()).await,
            Err(RecoveryError::InvalidStoredData)
        ));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM reversal_item_jobs")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 0);
        pool.execute("DROP TRIGGER zz_fixture_item_enroll ON reversal_item_jobs")
            .await
            .expect("restore healthy insert");
    }
    enroll(&pool, time.as_ref(), &ids, &client, intent())
        .await
        .expect("healthy exact retry");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn item_jobs_have_a_real_per_attempt_capacity_across_pages(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let attempt = &frozen.snapshot().binding.id;
    let verified = refund_candidates(frozen.snapshot(), 10).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    for page_index in 0..13 {
        let page = reversal_pages::record(
            &pool,
            time.as_ref(),
            &ids,
            &format!("job-cap-page-{page_index}"),
            None,
            &verified,
        )
        .await
        .expect("fresh bounded page");
        for ordinal in 1..=10 {
            let result = enroll(
                &pool,
                time.as_ref(),
                &ids,
                &client,
                EnrollIntent {
                    attempt_id: attempt,
                    observation_id: &paid,
                    page_id: &page.id,
                    ordinal,
                    enrollment_key: &format!("job-cap-{page_index}-{ordinal}"),
                },
            )
            .await;
            if page_index * 10 + ordinal <= 128 {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(RecoveryError::Conflict)));
            }
        }
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reversal_item_jobs WHERE attempt_id=$1")
            .bind(attempt)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 128);
    assert_eq!(server.count(), 1);
}
