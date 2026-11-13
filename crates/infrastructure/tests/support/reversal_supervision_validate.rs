//! Literal controls for a frozen job's operation and authenticated page scope.
use super::*;
fn job() -> Job {
    Job {
        id: "a".repeat(32),
        attempt_id: "b".repeat(32),
        observation_id: "c".repeat(32),
        enrollment_key: "enroll".into(),
        profile_id: "pfl_sample".into(),
        payment_id: "tr_sample".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "d".repeat(64),
        state: "leased".into(),
        review_reason: None,
        tip_page_id: Some("e".repeat(32)),
        generation: 2,
        failures: 0,
        created_at: 100,
        updated_at: 120,
        due_at: 100,
        lease_until: Some(180),
    }
}
fn page() -> ReversalPageReceipt {
    ReversalPageReceipt {
        id: "f".repeat(32),
        order_id: "1".repeat(32),
        attempt_id: "b".repeat(32),
        effect_key: "effect".into(),
        payment_id: "tr_sample".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "d".repeat(64),
        scan_id: "e".repeat(32),
        page_number: 2,
        previous_page_id: Some("e".repeat(32)),
        from_cursor: Some("re_prev".into()),
        next_cursor: Some("re_next".into()),
        candidates: vec![],
        recorded_at: 120,
        event_id: "2".repeat(32),
        replayed: true,
    }
}
fn op() -> Operation {
    Operation {
        id: "3".repeat(32),
        job_id: "a".repeat(32),
        generation: 2,
        predecessor_id: Some("e".repeat(32)),
        started_at: 120,
        lease_until: 180,
        outcome: None,
        finished_at: None,
        page_id: None,
    }
}
#[test]
fn tip_scope_requires_exact_payment_kind_fingerprint_and_recording_window() {
    let j = job();
    assert!(valid_tip(&j, &page()));
    type Mutation = (&'static str, fn(&mut ReversalPageReceipt));
    let changes: &[Mutation] = &[
        ("payment", |p| p.payment_id = "tr_other".into()),
        ("kind", |p| p.kind = ReversalKind::Chargeback),
        ("key", |p| p.credential_fingerprint = "f".repeat(64)),
        ("before admission", |p| p.recorded_at = 99),
        ("after checkpoint", |p| p.recorded_at = 121),
    ];
    for (label, change) in changes {
        let mut p = page();
        change(&mut p);
        assert!(!valid_tip(&j, &p), "{label}")
    }
}
#[test]
fn effect_page_requires_exact_predecessor_scope_and_start_time() {
    let j = job();
    assert!(valid_page(&j, &page()));
    type Mutation = (&'static str, fn(&mut ReversalPageReceipt));
    let changes: &[Mutation] = &[
        ("predecessor", |p| p.previous_page_id = None),
        ("payment", |p| p.payment_id = "tr_other".into()),
        ("kind", |p| p.kind = ReversalKind::Chargeback),
        ("key", |p| p.credential_fingerprint = "f".repeat(64)),
        ("before start", |p| p.recorded_at = 119),
    ];
    for (label, change) in changes {
        let mut p = page();
        change(&mut p);
        assert!(!valid_page(&j, &p), "{label}")
    }
}
#[test]
fn late_effect_page_requires_exact_scope_and_original_operation_start() {
    let j = job();
    let mut p = page();
    p.recorded_at = 110;
    assert!(valid_late_page(&j, &p, 110));
    assert!(
        !valid_page(&j, &p),
        "a late page need not follow the stop time"
    );
    type Mutation = (&'static str, fn(&mut ReversalPageReceipt));
    for (label, change) in [
        (
            "predecessor",
            (|p: &mut ReversalPageReceipt| p.previous_page_id = None)
                as fn(&mut ReversalPageReceipt),
        ),
        ("payment", |p| p.payment_id = "tr_other".into()),
        ("kind", |p| p.kind = ReversalKind::Chargeback),
        ("key", |p| p.credential_fingerprint = "f".repeat(64)),
        ("before original start", |p| p.recorded_at = 109),
    ] as [Mutation; 5]
    {
        let mut changed = p.clone();
        change(&mut changed);
        assert!(!valid_late_page(&j, &changed, 110), "{label}");
    }
}
#[test]
fn started_operation_requires_exact_unfinished_lease() {
    let j = job();
    assert!(valid_operation(&j, &op()));
    type Mutation = (&'static str, fn(&mut Operation));
    let changes: &[Mutation] = &[
        ("id", |o| o.id = "bad".into()),
        ("job", |o| o.job_id = "4".repeat(32)),
        ("generation", |o| o.generation = 3),
        ("predecessor", |o| o.predecessor_id = None),
        ("start", |o| o.started_at = 119),
        ("lease", |o| o.lease_until = 181),
        ("outcome", |o| o.outcome = Some("page".into())),
        ("finish", |o| o.finished_at = Some(121)),
        ("page", |o| o.page_id = Some("5".repeat(32))),
    ];
    for (label, change) in changes {
        let mut o = op();
        change(&mut o);
        assert!(!valid_operation(&j, &o), "{label}")
    }
}

#[test]
fn source_requires_the_paid_observation_and_frozen_profile() {
    use commerce_domain::{
        Currency,
        payment_attempts::{
            PaymentMode, PaymentStatus, ProviderPayment, observations::ObservationIntent,
        },
    };
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
            credential_fingerprint: "d".repeat(64),
            payment: ProviderPayment {
                resource: "payment".into(),
                id: "tr_sample".into(),
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
    assert!(valid_source(&j, &frozen, &observation));
    observation.intent.payment.status = PaymentStatus::Open;
    assert!(!valid_source(&j, &frozen, &observation));
    observation.intent.payment.status = PaymentStatus::Paid;
    observation.intent.payment.id = "tr_other".into();
    assert!(!valid_source(&j, &frozen, &observation));
    observation.intent.payment.id = "tr_sample".into();
    j.profile_id = "pfl_other".into();
    assert!(!valid_source(&j, &frozen, &observation));
    j.profile_id = binding.profile.id.clone();
    observation.recorded_at = 101;
    assert!(!valid_source(&j, &frozen, &observation));
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn operation_reader_rejects_missing_and_damaged_started_rows(pool: sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("isolated transaction");
    sqlx::query("CREATE TEMP TABLE reversal_scan_operations(id text,job_id text,generation integer,predecessor_id text,started_at bigint,lease_until bigint,outcome text,finished_at bigint,page_id text) ON COMMIT DROP")
        .execute(&mut *tx).await.expect("isolated operations");
    let j = job();
    assert!(matches!(
        operation(&mut tx, &j).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("INSERT INTO reversal_scan_operations VALUES ($1,$2,2,$3,120,180,NULL,NULL,NULL)")
        .bind("3".repeat(32))
        .bind(&j.id)
        .bind(&j.tip_page_id)
        .execute(&mut *tx)
        .await
        .expect("started operation");
    assert!(operation(&mut tx, &j).await.is_ok());
    sqlx::query("UPDATE reversal_scan_operations SET outcome='page'")
        .execute(&mut *tx)
        .await
        .expect("damaged outcome");
    assert!(matches!(
        operation(&mut tx, &j).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    tx.rollback().await.expect("discard isolated fixture");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn real_paid_source_rejects_scope_drift_and_missing_dispatch_event(pool: sqlx::PgPool) {
    let (_, frozen, _, observation_id) =
        crate::mollie::tests::reversal_supervision_cases::fixture(&pool).await;
    let mut tx = pool.begin().await.expect("isolated transaction");
    let started: i64 =
        sqlx::query_scalar("SELECT started_at FROM payment_dispatch_starts WHERE attempt_id=$1")
            .bind(&frozen.snapshot().binding.id)
            .fetch_one(&mut *tx)
            .await
            .expect("dispatch");
    let observed: i64 =
        sqlx::query_scalar("SELECT recorded_at FROM payment_observations WHERE id=$1")
            .bind(&observation_id)
            .fetch_one(&mut *tx)
            .await
            .expect("observation");
    let mut j = job();
    j.attempt_id = frozen.snapshot().binding.id.clone();
    j.observation_id = observation_id;
    j.profile_id = frozen.snapshot().binding.profile.id.clone();
    j.payment_id = "tr_SYNTHETIC_NotARealPayment".into();
    j.created_at = observed;
    assert!(source(&mut tx, &j, &frozen, started).await.is_ok());
    j.payment_id = "tr_wrong".into();
    assert!(matches!(
        source(&mut tx, &j, &frozen, started).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    j.payment_id = "tr_SYNTHETIC_NotARealPayment".into();
    sqlx::query("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .execute(&mut *tx)
        .await
        .expect("isolate damaged event");
    sqlx::query("DELETE FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1' AND aggregate_id=$1")
        .bind(&frozen.snapshot().binding.order_id).execute(&mut *tx).await.expect("remove dispatch event");
    assert!(matches!(
        source(&mut tx, &j, &frozen, started).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    tx.rollback().await.expect("discard isolated damage");
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn receipt_readers_reject_scope_drift_on_authenticated_page(pool: sqlx::PgPool) {
    let frozen = crate::mollie::tests::captured(38245).1;
    let mut tx = pool.begin().await.expect("isolated transaction");
    for sql in [
        "CREATE SCHEMA reversal_validate_control",
        "CREATE TABLE reversal_validate_control.payment_reversal_pages AS SELECT * FROM public.payment_reversal_pages WITH NO DATA",
        "CREATE TABLE reversal_validate_control.commerce_outbox AS SELECT * FROM public.commerce_outbox WITH NO DATA",
        "CREATE FUNCTION reversal_validate_control.payment_reversal_page_event(reversal_validate_control.payment_reversal_pages) RETURNS jsonb LANGUAGE sql IMMUTABLE AS 'SELECT ''{\"synthetic\":true}''::jsonb'",
        "SET LOCAL search_path = reversal_validate_control, public",
    ] {
        sqlx::query(sql)
            .execute(&mut *tx)
            .await
            .expect("isolated receipt fixture");
    }
    let mut p = page();
    p.order_id = frozen.snapshot().binding.order_id.clone();
    p.attempt_id = frozen.snapshot().binding.id.clone();
    p.scan_id = p.id.clone();
    p.page_number = 1;
    p.previous_page_id = None;
    p.from_cursor = None;
    p.recorded_at = 1500;
    sqlx::query("INSERT INTO payment_reversal_pages(id,order_id,attempt_id,effect_key,payment_id,reversal_kind,credential_fingerprint,scan_id,page_number,previous_page_id,from_cursor,next_cursor,candidates,recorded_at,event_id,transaction_id) VALUES($1,$2,$3,$4,$5,'refund',$6,$7,1,NULL,NULL,$8,$9,$10,$11,pg_current_xact_id())")
        .bind(&p.id).bind(&p.order_id).bind(&p.attempt_id).bind(&p.effect_key).bind(&p.payment_id).bind(&p.credential_fingerprint)
        .bind(&p.scan_id).bind(&p.next_cursor).bind(&p.candidates).bind(p.recorded_at).bind(&p.event_id).execute(&mut *tx).await.expect("authenticated page fixture");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reversal_page_recorded.v1','{\"synthetic\":true}'::jsonb,$3)")
        .bind(&p.event_id).bind(&p.order_id).bind(p.recorded_at).execute(&mut *tx).await.expect("reciprocal event");
    let mut j = job();
    j.attempt_id = p.attempt_id.clone();
    j.tip_page_id = Some(p.id.clone());
    j.created_at = 1500;
    j.updated_at = 1500;
    assert!(tip(&mut tx, &j, &frozen, 1500).await.is_ok());
    j.payment_id = "tr_wrong".into();
    assert!(matches!(
        tip(&mut tx, &j, &frozen, 1500).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    j.payment_id = p.payment_id.clone();
    j.tip_page_id = None;
    assert!(matches!(
        exact_page(&mut tx, &j, &frozen, 1500, &p.effect_key).await,
        Ok(Some(_))
    ));
    j.payment_id = "tr_wrong".into();
    assert!(matches!(
        exact_page(&mut tx, &j, &frozen, 1500, &p.effect_key).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    tx.rollback().await.expect("discard isolated fixture");
}
