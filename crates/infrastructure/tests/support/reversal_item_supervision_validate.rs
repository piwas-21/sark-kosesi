//! Literal custody checks and isolated stored-operation controls.
use super::*;
use commerce_domain::{
    Currency,
    payment_attempts::{PaymentMode, ProviderPayment, observations::ObservationIntent},
};

fn job() -> Job {
    Job {
        id: "a".repeat(32),
        attempt_id: "b".repeat(32),
        observation_id: "c".repeat(32),
        page_id: "d".repeat(32),
        ordinal: 1,
        child_id: "re_example".into(),
        enrollment_key: "enroll".into(),
        profile_id: "pfl_example".into(),
        payment_id: "tr_example".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "e".repeat(64),
        state: "leased".into(),
        review_reason: None,
        receipt_id: None,
        generation: 1,
        failures: 0,
        created_at: 100,
        updated_at: 120,
        due_at: 100,
        lease_until: Some(180),
    }
}
fn page() -> ReversalPageReceipt {
    ReversalPageReceipt {
        id: "d".repeat(32),
        order_id: "1".repeat(32),
        attempt_id: "b".repeat(32),
        effect_key: "page".into(),
        payment_id: "tr_example".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "e".repeat(64),
        scan_id: "d".repeat(32),
        page_number: 1,
        previous_page_id: None,
        from_cursor: None,
        next_cursor: None,
        candidates: vec!["re_example".into(), "re_second".into()],
        recorded_at: 100,
        event_id: "2".repeat(32),
        replayed: true,
    }
}
fn sample_receipt() -> ReversalReceipt {
    ReversalReceipt {
        id: "f".repeat(32),
        order_id: "1".repeat(32),
        attempt_id: "b".repeat(32),
        effect_key: "3".repeat(32),
        payment_id: "tr_example".into(),
        child_id: "re_example".into(),
        phase: ReversalPhase::RefundPending,
        amount_minor: 1234,
        credential_fingerprint: "e".repeat(64),
        source_created_seconds: 1_710_927_217,
        source_created_nanos: 0,
        source_reversed_seconds: None,
        source_reversed_nanos: None,
        recorded_at: 120,
        event_id: "2".repeat(32),
        replayed: true,
    }
}
fn op() -> Operation {
    Operation {
        id: "3".repeat(32),
        job_id: "a".repeat(32),
        generation: 1,
        started_at: 120,
        lease_until: 180,
        outcome: None,
        finished_at: None,
        receipt_id: None,
    }
}

#[test]
fn source_requires_paid_exact_page_candidate_key_and_times() {
    let frozen = crate::mollie::tests::captured(38245).1;
    let binding = &frozen.snapshot().binding;
    let mut j = job();
    j.profile_id = binding.profile.id.clone();
    let mut observation = ObservationSnapshot {
        schema_version: 1,
        id: "c".repeat(32),
        attempt_id: binding.id.clone(),
        effect_key: "paid".into(),
        revision: 1,
        recorded_at: 100,
        intent: ObservationIntent {
            credential_fingerprint: "e".repeat(64),
            payment: ProviderPayment {
                resource: "payment".into(),
                id: "tr_example".into(),
                profile_id: binding.profile.id.clone(),
                mode: PaymentMode::Test,
                currency: Currency::EUR,
                amount_minor: 38245,
                metadata_schema_version: 1,
                order_id: binding.order_id.clone(),
                attempt_id: binding.id.clone(),
                status: PaymentStatus::Paid,
            },
        },
    };
    assert!(valid_source(&j, &frozen, &observation, &page()));
    observation.intent.payment.status = PaymentStatus::Open;
    assert!(!valid_source(&j, &frozen, &observation, &page()));
    observation.intent.payment.status = PaymentStatus::Paid;
    observation.intent.payment.id = "tr_other".into();
    assert!(!valid_source(&j, &frozen, &observation, &page()));
    observation.intent.payment.id = "tr_example".into();
    observation.recorded_at = 101;
    assert!(!valid_source(&j, &frozen, &observation, &page()));
    observation.recorded_at = 100;
    j.profile_id = "pfl_other".into();
    assert!(!valid_source(&j, &frozen, &observation, &page()));
    j.profile_id = binding.profile.id.clone();
    type Change = (&'static str, fn(&mut ReversalPageReceipt));
    let changes: &[Change] = &[
        ("payment", |p| p.payment_id = "tr_other".into()),
        ("kind", |p| p.kind = ReversalKind::Chargeback),
        ("fingerprint", |p| p.credential_fingerprint = "f".repeat(64)),
        ("candidate", |p| p.candidates[0] = "re_other".into()),
        ("page time", |p| p.recorded_at = 101),
    ];
    for (label, change) in changes {
        let mut p = page();
        change(&mut p);
        assert!(!valid_source(&j, &frozen, &observation, &p), "{label}");
    }
    for ordinal in [0, 2, 3, i32::MIN] {
        j.ordinal = ordinal;
        assert!(
            !valid_source(&j, &frozen, &observation, &page()),
            "{ordinal}"
        );
    }
    j.ordinal = 2;
    j.child_id = "re_second".into();
    assert!(valid_source(&j, &frozen, &observation, &page()));
}

#[test]
fn exact_receipt_requires_kind_scope_fingerprint_and_operation_start() {
    let j = job();
    assert!(matches(&j, &sample_receipt(), 120));
    type Change = (&'static str, fn(&mut ReversalReceipt));
    let changes: &[Change] = &[
        ("attempt", |r| r.attempt_id = "4".repeat(32)),
        ("payment", |r| r.payment_id = "tr_other".into()),
        ("child", |r| r.child_id = "re_other".into()),
        ("kind", |r| r.phase = ReversalPhase::ChargebackReceived),
        ("fingerprint", |r| r.credential_fingerprint = "f".repeat(64)),
        ("time", |r| r.recorded_at = 119),
    ];
    for (label, change) in changes {
        let mut r = sample_receipt();
        change(&mut r);
        assert!(!matches(&j, &r, 120), "{label}");
    }
    let mut j = job();
    j.kind = ReversalKind::Chargeback;
    j.child_id = "chb_example".into();
    for phase in [
        ReversalPhase::ChargebackReversalUnknown,
        ReversalPhase::ChargebackReceived,
        ReversalPhase::ChargebackReversed,
    ] {
        let mut r = sample_receipt();
        r.phase = phase;
        r.child_id = "chb_example".into();
        assert!(matches(&j, &r, 120));
    }
    let mut r = sample_receipt();
    r.child_id = "chb_example".into();
    assert!(!matches(&j, &r, 120));
}

#[test]
fn operation_requires_the_exact_unfinished_current_lease() {
    let j = job();
    assert!(valid_operation(&j, &op()));
    type Change = (&'static str, fn(&mut Operation));
    let changes: &[Change] = &[
        ("id", |o| o.id = "bad".into()),
        ("job", |o| o.job_id = "4".repeat(32)),
        ("generation", |o| o.generation = 2),
        ("start", |o| o.started_at = 119),
        ("lease", |o| o.lease_until = 181),
        ("outcome", |o| o.outcome = Some("observed".into())),
        ("finish", |o| o.finished_at = Some(121)),
        ("receipt", |o| o.receipt_id = Some("f".repeat(32))),
    ];
    for (label, change) in changes {
        let mut o = op();
        change(&mut o);
        assert!(!valid_operation(&j, &o), "{label}");
    }
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn operation_lookup_rejects_absent_and_damaged_rows_with_healthy_repair(pool: sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("isolated transaction");
    sqlx::query("CREATE TEMP TABLE reversal_item_operations(id text,job_id text,generation integer,started_at bigint,lease_until bigint,outcome text,finished_at bigint,receipt_id text) ON COMMIT DROP")
        .execute(&mut *tx).await.expect("isolated operations");
    let j = job();
    assert!(matches!(
        operation(&mut tx, &j).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("INSERT INTO reversal_item_operations VALUES ($1,$2,1,120,180,NULL,NULL,NULL)")
        .bind("3".repeat(32))
        .bind(&j.id)
        .execute(&mut *tx)
        .await
        .expect("started");
    assert!(operation(&mut tx, &j).await.is_ok());
    sqlx::query("UPDATE reversal_item_operations SET lease_until=181")
        .execute(&mut *tx)
        .await
        .expect("damage");
    assert!(matches!(
        operation(&mut tx, &j).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE reversal_item_operations SET lease_until=180")
        .execute(&mut *tx)
        .await
        .expect("repair");
    assert!(operation(&mut tx, &j).await.is_ok());
    tx.rollback().await.expect("discard fixture");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn receipt_and_late_readers_require_the_exact_operation_and_reciprocal_event(
    pool: sqlx::PgPool,
) {
    let frozen = crate::mollie::tests::captured(38245).1;
    let mut tx = pool.begin().await.expect("isolated transaction");
    for sql in [
        "CREATE SCHEMA item_validate_control",
        "CREATE TABLE item_validate_control.reversal_item_operations AS SELECT * FROM public.reversal_item_operations WITH NO DATA",
        "CREATE TABLE item_validate_control.payment_reversal_observations AS SELECT * FROM public.payment_reversal_observations WITH NO DATA",
        "CREATE TABLE item_validate_control.commerce_outbox AS SELECT * FROM public.commerce_outbox WITH NO DATA",
        "CREATE FUNCTION item_validate_control.payment_reversal_observation_event(item_validate_control.payment_reversal_observations) RETURNS jsonb LANGUAGE sql IMMUTABLE AS 'SELECT ''{\"synthetic\":true}''::jsonb'",
        "SET LOCAL search_path = item_validate_control, public",
    ] {
        sqlx::query(sql)
            .execute(&mut *tx)
            .await
            .expect("isolated receipt fixture");
    }
    let mut j = job();
    j.attempt_id = frozen.snapshot().binding.id.clone();
    j.created_at = 1500;
    j.updated_at = 1560;
    j.due_at = 1560;
    j.state = "review".into();
    j.review_reason = Some("uncertain".into());
    j.lease_until = None;
    let effect = "3".repeat(32);
    assert!(matches!(
        late_receipt(&mut tx, &j, &frozen, 1500).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("INSERT INTO reversal_item_operations(id,job_id,generation,started_at,lease_until,outcome,finished_at,receipt_id,start_xid,finish_xid) VALUES($1,$2,1,1500,1560,'uncertain',1560,NULL,'1'::xid8,'2'::xid8)")
        .bind(&effect).bind(&j.id).execute(&mut *tx).await.expect("parked operation");
    assert!(
        late_receipt(&mut tx, &j, &frozen, 1500)
            .await
            .expect("valid empty late receipt")
            .is_none()
    );
    for (column, bad, good) in [
        ("started_at", "1499", "1500"),
        ("lease_until", "1559", "1560"),
        ("finished_at", "1559", "1560"),
        ("outcome", "'invalid'", "'uncertain'"),
        ("start_xid", "NULL", "'1'::xid8"),
        ("finish_xid", "NULL", "'2'::xid8"),
        ("finish_xid", "'1'::xid8", "'2'::xid8"),
        ("receipt_id", "'bad'", "NULL"),
        ("id", "'bad'", "'33333333333333333333333333333333'"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE reversal_item_operations SET {column}={bad}"
        )))
        .execute(&mut *tx)
        .await
        .expect("damage operation");
        assert!(
            matches!(
                late_receipt(&mut tx, &j, &frozen, 1500).await,
                Err(RecoveryError::InvalidStoredData)
            ),
            "{column}={bad}"
        );
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE reversal_item_operations SET {column}={good}"
        )))
        .execute(&mut *tx)
        .await
        .expect("repair operation");
        assert!(
            late_receipt(&mut tx, &j, &frozen, 1500)
                .await
                .expect("healthy operation")
                .is_none()
        );
    }
    let receipt_id = "f".repeat(32);
    let event_id = "2".repeat(32);
    sqlx::query("INSERT INTO payment_reversal_observations(id,order_id,attempt_id,effect_key,payment_id,child_id,phase,amount_minor,credential_fingerprint,source_created_seconds,source_created_nanos,recorded_at,event_id,transaction_id) VALUES($1,$2,$3,$4,'tr_example','re_example','refund_pending',1234,$5,1710927217,0,1561,$6,'1'::xid8)")
        .bind(&receipt_id).bind(&frozen.snapshot().binding.order_id).bind(&j.attempt_id).bind(&effect)
        .bind(&j.credential_fingerprint).bind(&event_id).execute(&mut *tx).await.expect("receipt");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reversal_observed.v1','{\"synthetic\":true}'::jsonb,1561)")
        .bind(&event_id).bind(&frozen.snapshot().binding.order_id).execute(&mut *tx).await.expect("event");
    assert_eq!(
        late_receipt(&mut tx, &j, &frozen, 1500)
            .await
            .expect("late")
            .expect("receipt")
            .id,
        receipt_id
    );
    sqlx::query("UPDATE payment_reversal_observations SET child_id='re_other'")
        .execute(&mut *tx)
        .await
        .expect("damage scope");
    assert!(matches!(
        late_receipt(&mut tx, &j, &frozen, 1500).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_reversal_observations SET child_id='re_example'")
        .execute(&mut *tx)
        .await
        .expect("repair scope");
    assert!(
        late_receipt(&mut tx, &j, &frozen, 1500)
            .await
            .expect("repaired")
            .is_some()
    );
    sqlx::query("UPDATE commerce_outbox SET kind='wrong'")
        .execute(&mut *tx)
        .await
        .expect("damage event");
    assert!(late_receipt(&mut tx, &j, &frozen, 1500).await.is_err());
    sqlx::query("UPDATE commerce_outbox SET kind='payment.reversal_observed.v1'")
        .execute(&mut *tx)
        .await
        .expect("repair event");
    assert!(
        late_receipt(&mut tx, &j, &frozen, 1500)
            .await
            .expect("repaired event")
            .is_some()
    );
    j.state = "observed".into();
    j.review_reason = None;
    j.receipt_id = Some(receipt_id.clone());
    j.updated_at = 1561;
    j.due_at = 1561;
    assert!(matches!(
        receipt(&mut tx, &j, &frozen, 1500).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query(
        "UPDATE reversal_item_operations SET outcome='observed',finished_at=1561,receipt_id=$1",
    )
    .bind(&receipt_id)
    .execute(&mut *tx)
    .await
    .expect("observed operation");
    assert!(receipt(&mut tx, &j, &frozen, 1500).await.is_ok());
    j.updated_at = 1562;
    assert!(matches!(
        receipt(&mut tx, &j, &frozen, 1500).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    j.updated_at = 1561;
    sqlx::query("UPDATE reversal_item_operations SET finished_at=1560")
        .execute(&mut *tx)
        .await
        .expect("finish before receipt");
    assert!(matches!(
        receipt(&mut tx, &j, &frozen, 1500).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE reversal_item_operations SET finished_at=1561")
        .execute(&mut *tx)
        .await
        .expect("repair finish");
    assert!(receipt(&mut tx, &j, &frozen, 1500).await.is_ok());
    tx.rollback().await.expect("discard fixture");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn actual_paid_source_requires_dispatch_and_exact_page_scope(pool: sqlx::PgPool) {
    let (_, frozen, _, observation_id) =
        crate::mollie::tests::reversal_supervision_cases::fixture(&pool).await;
    let mut tx = pool.begin().await.expect("isolated transaction");
    let started: i64 =
        sqlx::query_scalar("SELECT started_at FROM payment_dispatch_starts WHERE attempt_id=$1")
            .bind(&frozen.snapshot().binding.id)
            .fetch_one(&mut *tx)
            .await
            .expect("dispatch time");
    let observed: i64 =
        sqlx::query_scalar("SELECT recorded_at FROM payment_observations WHERE id=$1")
            .bind(&observation_id)
            .fetch_one(&mut *tx)
            .await
            .expect("observation time");
    for sql in [
        "CREATE SCHEMA item_source_control",
        "CREATE TABLE item_source_control.payment_reversal_pages AS SELECT * FROM public.payment_reversal_pages WITH NO DATA",
        "CREATE TABLE item_source_control.commerce_outbox AS SELECT * FROM public.commerce_outbox",
        "CREATE FUNCTION item_source_control.payment_reversal_page_event(item_source_control.payment_reversal_pages) RETURNS jsonb LANGUAGE sql IMMUTABLE AS 'SELECT ''{\"synthetic\":true}''::jsonb'",
        "SET LOCAL search_path = item_source_control, public",
    ] {
        sqlx::query(sql)
            .execute(&mut *tx)
            .await
            .expect("isolated source fixture");
    }
    let mut j = job();
    j.attempt_id = frozen.snapshot().binding.id.clone();
    j.observation_id = observation_id;
    j.profile_id = frozen.snapshot().binding.profile.id.clone();
    j.payment_id = "tr_SYNTHETIC_NotARealPayment".into();
    j.created_at = observed;
    j.updated_at = observed;
    j.due_at = observed;
    let event_id = "2".repeat(32);
    sqlx::query("INSERT INTO payment_reversal_pages(id,order_id,attempt_id,effect_key,payment_id,reversal_kind,credential_fingerprint,scan_id,page_number,candidates,recorded_at,event_id,transaction_id) VALUES($1,$2,$3,'page',$4,'refund',$5,$1,1,$6,$7,$8,pg_current_xact_id())")
        .bind(&j.page_id).bind(&frozen.snapshot().binding.order_id).bind(&j.attempt_id).bind(&j.payment_id)
        .bind(&j.credential_fingerprint).bind(vec![j.child_id.clone()]).bind(observed).bind(&event_id)
        .execute(&mut *tx).await.expect("page");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reversal_page_recorded.v1','{\"synthetic\":true}'::jsonb,$3)")
        .bind(&event_id).bind(&frozen.snapshot().binding.order_id).bind(observed).execute(&mut *tx).await.expect("page event");
    assert!(source(&mut tx, &j, &frozen, started).await.is_ok());
    j.ordinal = 2;
    assert!(matches!(
        source(&mut tx, &j, &frozen, started).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    j.ordinal = 1;
    assert!(source(&mut tx, &j, &frozen, started).await.is_ok());
    sqlx::query("DELETE FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1' AND aggregate_id=$1")
        .bind(&frozen.snapshot().binding.order_id).execute(&mut *tx).await.expect("remove dispatch event");
    assert!(matches!(
        source(&mut tx, &j, &frozen, started).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("INSERT INTO commerce_outbox SELECT * FROM public.commerce_outbox WHERE kind='payment.reconciliation_required.v1' AND aggregate_id=$1")
        .bind(&frozen.snapshot().binding.order_id).execute(&mut *tx).await.expect("repair dispatch event");
    assert!(source(&mut tx, &j, &frozen, started).await.is_ok());
    tx.rollback().await.expect("discard fixture");
}
