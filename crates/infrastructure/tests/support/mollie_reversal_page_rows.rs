//! Literal stored-row and predecessor controls; all identities are synthetic.
use super::*;

fn frozen() -> FrozenAttempt {
    crate::mollie::tests::captured(38245).1
}
fn receipt(frozen: &FrozenAttempt) -> ReversalPageReceipt {
    ReversalPageReceipt {
        id: "a".repeat(32),
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: frozen.snapshot().binding.id.clone(),
        effect_key: "page-effect".into(),
        payment_id: "tr_EXAMPLE".into(),
        kind: ReversalKind::Refund,
        credential_fingerprint: "d".repeat(64),
        scan_id: "a".repeat(32),
        page_number: 1,
        previous_page_id: None,
        from_cursor: None,
        next_cursor: Some("re_NEXT".into()),
        candidates: vec!["re_ONE".into(), "re_TWO".into()],
        recorded_at: 1500,
        event_id: "b".repeat(32),
        replayed: true,
    }
}
fn child(parent: &ReversalPageReceipt) -> ReversalPageReceipt {
    let mut r = parent.clone();
    r.id = "c".repeat(32);
    r.page_number = 2;
    r.previous_page_id = Some(parent.id.clone());
    r.from_cursor = parent.next_cursor.clone();
    r.next_cursor = Some("re_LATER".into());
    r
}
fn row(f: &FrozenAttempt) -> Row {
    let r = receipt(f);
    Row {
        id: r.id,
        order_id: r.order_id,
        attempt_id: r.attempt_id,
        effect_key: r.effect_key,
        payment_id: r.payment_id,
        reversal_kind: "refund".into(),
        credential_fingerprint: r.credential_fingerprint,
        scan_id: r.scan_id,
        page_number: r.page_number,
        previous_page_id: r.previous_page_id,
        from_cursor: r.from_cursor,
        next_cursor: r.next_cursor,
        candidates: Some(r.candidates),
        recorded_at: r.recorded_at,
        event_id: r.event_id,
        transaction_present: true,
        event_present: true,
        child_count: 0,
    }
}
#[test]
fn provider_id_and_kind_parsers_are_closed() {
    for (name, kind) in [
        ("refund", ReversalKind::Refund),
        ("chargeback", ReversalKind::Chargeback),
    ] {
        assert_eq!(parse_kind(name), Some(kind));
        assert_eq!(kind_name(kind), name);
    }
    assert_eq!(parse_kind("Refund"), None);
    assert!(opaque("re_valid", "re_"));
    for value in ["other", "re_", "re_value ", "re_\nvalue"] {
        assert!(!opaque(value, "re_"), "{value:?}");
    }
    assert!(!opaque(&format!("re_{}", "x".repeat(254)), "re_"));
}
#[test]
fn every_stored_scope_and_page_invariant_is_checked() {
    let f = frozen();
    type Mutation = (&'static str, fn(&mut ReversalPageReceipt));
    let mutations: [Mutation; 21] = [
        ("id", |r| r.id = "bad".into()),
        ("event", |r| r.event_id = "bad".into()),
        ("order shape", |r| r.order_id = "bad".into()),
        ("order binding", |r| r.order_id = "8".repeat(32)),
        ("attempt binding", |r| r.attempt_id = "8".repeat(32)),
        ("effect", |r| r.effect_key = "bad key".into()),
        ("scan", |r| r.scan_id = "bad".into()),
        ("payment", |r| r.payment_id = "bad".into()),
        ("page bound", |r| r.page_number = 33),
        ("key length", |r| r.credential_fingerprint = "d".repeat(63)),
        ("key alphabet", |r| {
            r.credential_fingerprint = "z".repeat(64)
        }),
        ("time", |r| r.recorded_at = 1499),
        ("candidate count", |r| {
            r.candidates = (0..11).map(|n| format!("re_{n}")).collect()
        }),
        ("candidate format", |r| r.candidates = vec!["bad".into()]),
        ("candidate duplicate", |r| {
            r.candidates = vec!["re_A".into(), "re_A".into()]
        }),
        ("from format", |r| r.from_cursor = Some("bad".into())),
        ("next format", |r| r.next_cursor = Some("bad".into())),
        ("cursor repeats", |r| {
            r.from_cursor = Some("re_A".into());
            r.next_cursor = Some("re_A".into());
        }),
        ("root predecessor", |r| {
            r.previous_page_id = Some("c".repeat(32))
        }),
        ("root cursor", |r| r.from_cursor = Some("re_A".into())),
        ("root scan", |r| r.scan_id = "c".repeat(32)),
    ];
    assert!(validate(&receipt(&f), &f, 1500).is_ok());
    for (label, change) in mutations {
        let mut r = receipt(&f);
        change(&mut r);
        assert_eq!(
            validate(&r, &f, 1500),
            Err(PaymentStoreError::InvalidStoredData),
            "{label}"
        );
    }
    let valid_child = child(&receipt(&f));
    assert!(validate(&valid_child, &f, 1500).is_ok());
    for change in [
        (|r: &mut ReversalPageReceipt| r.previous_page_id = None) as fn(&mut ReversalPageReceipt),
        |r| r.previous_page_id = Some("bad".into()),
        |r| r.from_cursor = None,
        |r| r.scan_id = r.id.clone(),
    ] {
        let mut r = valid_child.clone();
        change(&mut r);
        assert_eq!(
            validate(&r, &f, 1500),
            Err(PaymentStoreError::InvalidStoredData)
        );
    }
    let mut chargeback = receipt(&f);
    chargeback.kind = ReversalKind::Chargeback;
    chargeback.candidates = vec!["chb_ONE".into()];
    chargeback.next_cursor = Some("chb_NEXT".into());
    assert!(validate(&chargeback, &f, 1500).is_ok());
}
#[test]
fn predecessor_relation_requires_every_exact_binding() {
    let f = frozen();
    let parent = receipt(&f);
    let valid = child(&parent);
    assert!(follows(&valid, &parent));
    type Mutation = fn(&mut ReversalPageReceipt);
    let changes: [Mutation; 8] = [
        |r| r.previous_page_id = None,
        |r| r.scan_id = "f".repeat(32),
        |r| r.page_number = 3,
        |r| r.payment_id = "tr_OTHER".into(),
        |r| r.kind = ReversalKind::Chargeback,
        |r| r.credential_fingerprint = "e".repeat(64),
        |r| r.from_cursor = Some("re_WRONG".into()),
        |r| r.recorded_at = 1499,
    ];
    for change in changes {
        let mut r = valid.clone();
        change(&mut r);
        assert!(!follows(&r, &parent));
    }
    let mut terminal = parent.clone();
    terminal.next_cursor = None;
    assert!(!follows(&valid, &terminal));
}
#[test]
fn row_decode_rejects_missing_event_transaction_fork_and_bad_fields() {
    let f = frozen();
    assert!(row(&f).decode(&f, 1500).is_ok());
    type Mutation = fn(&mut Row);
    let changes: [Mutation; 6] = [
        |r| r.event_present = false,
        |r| r.transaction_present = false,
        |r| r.child_count = 2,
        |r| r.reversal_kind = "unexpected".into(),
        |r| r.candidates = None,
        |r| r.id = "invalid".into(),
    ];
    for change in changes {
        let mut r = row(&f);
        change(&mut r);
        assert!(matches!(
            r.decode(&f, 1500),
            Err(PaymentStoreError::InvalidStoredData)
        ));
    }
}

// An isolated schema exercises the real reader and its ancestry checks
// without mutating a shared or deployed database.
#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn malformed_ancestry_and_repeated_offsets_fail_closed(pool: sqlx::PgPool) {
    let f = frozen();
    let mut tx = pool.begin().await.expect("transaction");
    for sql in [
        "CREATE SCHEMA reversal_chain_control",
        "CREATE TABLE reversal_chain_control.payment_reversal_pages AS SELECT * FROM public.payment_reversal_pages WITH NO DATA",
        "CREATE TABLE reversal_chain_control.commerce_outbox AS SELECT * FROM public.commerce_outbox WITH NO DATA",
        "CREATE FUNCTION reversal_chain_control.payment_reversal_page_event(reversal_chain_control.payment_reversal_pages) RETURNS jsonb LANGUAGE sql IMMUTABLE AS 'SELECT ''{\"synthetic\":true}''::jsonb'",
        "SET LOCAL search_path = reversal_chain_control, public",
    ] {
        sqlx::query(sql)
            .execute(&mut *tx)
            .await
            .expect("isolated fixture");
    }
    let root = receipt(&f);
    let next = child(&root);
    for r in [&root, &next] {
        sqlx::query("INSERT INTO payment_reversal_pages(id,order_id,attempt_id,effect_key,payment_id,reversal_kind,credential_fingerprint,scan_id,page_number,previous_page_id,from_cursor,next_cursor,candidates,recorded_at,event_id,transaction_id) VALUES($1,$2,$3,$4,$5,'refund',$6,$7,$8,$9,$10,$11,$12,$13,$14,pg_current_xact_id())")
            .bind(&r.id).bind(&r.order_id).bind(&r.attempt_id).bind(&r.effect_key)
            .bind(&r.payment_id).bind(&r.credential_fingerprint).bind(&r.scan_id).bind(r.page_number)
            .bind(&r.previous_page_id).bind(&r.from_cursor).bind(&r.next_cursor).bind(&r.candidates)
            .bind(r.recorded_at).bind(&r.event_id).execute(&mut *tx).await.expect("page fixture");
        sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reversal_page_recorded.v1','{\"synthetic\":true}'::jsonb,$3)")
            .bind(&r.event_id).bind(&r.order_id).bind(r.recorded_at).execute(&mut *tx).await.expect("event fixture");
    }
    assert!(chain(&mut tx, &f, 1500, &next).await.is_ok());
    let mut cyclic = next.clone();
    cyclic.next_cursor = cyclic.from_cursor.clone();
    assert_eq!(
        chain(&mut tx, &f, 1500, &cyclic).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    let mut missing = next.clone();
    missing.previous_page_id = Some("f".repeat(32));
    assert_eq!(
        chain(&mut tx, &f, 1500, &missing).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    let mut mismatched = next.clone();
    mismatched.payment_id = "tr_OTHER".into();
    assert_eq!(
        chain(&mut tx, &f, 1500, &mismatched).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    tx.rollback().await.expect("discard fixture");
}
