//! Literal stored-item checkpoint controls; no provider credentials or network.
use super::*;

fn pending() -> Row {
    Row {
        id: "a".repeat(32),
        attempt_id: "b".repeat(32),
        observation_id: "c".repeat(32),
        page_id: "d".repeat(32),
        ordinal: 1,
        child_id: "re_example".into(),
        enrollment_key: "enroll-1".into(),
        profile_id: "pfl_example".into(),
        payment_id: "tr_example".into(),
        reversal_kind: "refund".into(),
        credential_fingerprint: "e".repeat(64),
        state: "pending".into(),
        review_reason: None,
        receipt_id: None,
        generation: 0,
        failures: 0,
        created_at: 100,
        updated_at: 100,
        due_at: 100,
        lease_until: None,
    }
}

#[test]
fn accepts_boundaries_and_complete_checkpoint_shapes() {
    assert!(decode(pending()).is_ok());
    let mut chargeback = pending();
    chargeback.reversal_kind = "chargeback".into();
    chargeback.child_id = "chb_example".into();
    chargeback.ordinal = 10;
    assert!(decode(chargeback).is_ok());
    let mut leased = pending();
    leased.state = "leased".into();
    leased.generation = 3;
    leased.failures = 2;
    leased.lease_until = Some(160);
    assert!(decode(leased).is_ok());
    for reason in [
        "uncertain",
        "invalid",
        "retry_exhausted",
        "observation_limit",
    ] {
        let mut review = pending();
        review.state = "review".into();
        review.review_reason = Some(reason.into());
        review.generation = 3;
        review.failures = 3;
        assert!(decode(review).is_ok(), "{reason}");
    }
    let mut observed = pending();
    observed.state = "observed".into();
    observed.receipt_id = Some("f".repeat(32));
    observed.generation = 3;
    assert!(decode(observed).is_ok());
    let mut last_time = pending();
    last_time.created_at = 9_007_199_254_740_991;
    last_time.updated_at = last_time.created_at;
    last_time.due_at = last_time.created_at;
    assert!(decode(last_time).is_ok());
}

#[test]
fn rejects_identity_scope_time_and_checkpoint_corruption() {
    type Change = (&'static str, fn(&mut Row));
    let changes: &[Change] = &[
        ("id", |r| r.id = "invalid".into()),
        ("attempt", |r| r.attempt_id = "invalid".into()),
        ("observation", |r| r.observation_id = "invalid".into()),
        ("page", |r| r.page_id = "invalid".into()),
        ("ordinal zero", |r| r.ordinal = 0),
        ("ordinal eleven", |r| r.ordinal = 11),
        ("kind", |r| r.reversal_kind = "Refund".into()),
        ("child kind", |r| r.child_id = "chb_example".into()),
        ("child empty", |r| r.child_id = "re_".into()),
        ("child long", |r| {
            r.child_id = format!("re_{}", "x".repeat(254))
        }),
        ("child whitespace", |r| r.child_id = "re_example ".into()),
        ("child control", |r| r.child_id = "re_a\nb".into()),
        ("key", |r| r.enrollment_key = "bad key".into()),
        ("profile", |r| r.profile_id = "bad".into()),
        ("payment", |r| r.payment_id = "tr_".into()),
        ("fingerprint", |r| r.credential_fingerprint = "z".repeat(64)),
        ("generation negative", |r| r.generation = -1),
        ("generation overflow", |r| r.generation = 4),
        ("failures negative", |r| r.failures = -1),
        ("failures overflow", |r| r.failures = 4),
        ("created negative", |r| r.created_at = -1),
        ("created overflow", |r| r.created_at = 9_007_199_254_740_992),
        ("updated before created", |r| r.updated_at = 99),
        ("updated overflow", |r| r.updated_at = 9_007_199_254_740_992),
        ("due before created", |r| r.due_at = 99),
        ("due overflow", |r| r.due_at = 9_007_199_254_740_992),
        ("receipt identifier", |r| r.receipt_id = Some("bad".into())),
        ("state", |r| r.state = "other".into()),
        ("missing lease", |r| r.state = "leased".into()),
        ("unexpected lease", |r| r.lease_until = Some(160)),
        ("missing reason", |r| r.state = "review".into()),
        ("unexpected reason", |r| {
            r.review_reason = Some("invalid".into())
        }),
        ("missing receipt", |r| r.state = "observed".into()),
        ("unexpected receipt", |r| {
            r.receipt_id = Some("f".repeat(32))
        }),
        ("unknown reason", |r| {
            r.state = "review".into();
            r.review_reason = Some("unknown".into());
        }),
        ("lease zero generation", |r| {
            r.state = "leased".into();
            r.lease_until = Some(160);
        }),
        ("short lease", |r| {
            r.state = "leased".into();
            r.generation = 1;
            r.lease_until = Some(159);
        }),
        ("long lease", |r| {
            r.state = "leased".into();
            r.generation = 1;
            r.lease_until = Some(161);
        }),
        ("lease overflow", |r| {
            r.state = "leased".into();
            r.generation = 1;
            r.updated_at = 9_007_199_254_740_932;
            r.lease_until = Some(9_007_199_254_740_992);
        }),
        ("pending due before update", |r| r.updated_at = 101),
        ("pending final generation", |r| r.generation = 3),
        ("pending final failure", |r| r.failures = 3),
        ("review due mismatch", |r| {
            r.state = "review".into();
            r.review_reason = Some("invalid".into());
            r.due_at = 101;
        }),
        ("observed due mismatch", |r| {
            r.state = "observed".into();
            r.receipt_id = Some("f".repeat(32));
            r.due_at = 101;
        }),
        ("leased final failure", |r| {
            r.state = "leased".into();
            r.generation = 1;
            r.lease_until = Some(160);
            r.failures = 3;
        }),
        ("observed final failure", |r| {
            r.state = "observed".into();
            r.receipt_id = Some("f".repeat(32));
            r.failures = 3;
        }),
    ];
    for (label, change) in changes {
        let mut row = pending();
        change(&mut row);
        assert!(
            matches!(decode(row), Err(RecoveryError::InvalidStoredData)),
            "{label}"
        );
    }
}

#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn lookup_rechecks_the_stored_row_by_id_and_by_key(pool: sqlx::PgPool) {
    let mut tx = pool.begin().await.expect("isolated transaction");
    sqlx::query("CREATE TEMP TABLE reversal_item_jobs(id text,attempt_id text,observation_id text,page_id text,ordinal integer,child_id text,enrollment_key text,profile_id text,payment_id text,reversal_kind text,credential_fingerprint text,state text,review_reason text,receipt_id text,generation integer,failures integer,created_at bigint,updated_at bigint,due_at bigint,lease_until bigint) ON COMMIT DROP")
        .execute(&mut *tx).await.expect("isolated job table");
    let id = "a".repeat(32);
    let attempt = "b".repeat(32);
    assert!(load(&mut tx, &id).await.expect("empty read").is_none());
    assert!(
        by_key(&mut tx, &attempt, "enroll-1")
            .await
            .expect("empty key")
            .is_none()
    );
    sqlx::query("INSERT INTO reversal_item_jobs VALUES ($1,$2,$3,$4,1,'re_example','enroll-1','pfl_example','tr_example','refund',$5,'pending',NULL,NULL,0,0,100,100,100,NULL)")
        .bind(&id).bind(&attempt).bind("c".repeat(32)).bind("d".repeat(32)).bind("e".repeat(64))
        .execute(&mut *tx).await.expect("healthy row");
    assert!(load(&mut tx, &id).await.expect("read").is_some());
    assert!(
        by_key(&mut tx, &attempt, "enroll-1")
            .await
            .expect("key")
            .is_some()
    );
    sqlx::query("UPDATE reversal_item_jobs SET ordinal=0")
        .execute(&mut *tx)
        .await
        .expect("damage");
    assert!(matches!(
        load(&mut tx, &id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    assert!(matches!(
        by_key(&mut tx, &attempt, "enroll-1").await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE reversal_item_jobs SET ordinal=1")
        .execute(&mut *tx)
        .await
        .expect("repair");
    assert!(load(&mut tx, &id).await.expect("repaired").is_some());
    tx.rollback().await.expect("discard fixture");
}
