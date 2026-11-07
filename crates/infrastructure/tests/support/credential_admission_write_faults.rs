use super::*;

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct ScopeRow {
    purpose: i16,
    window_start: Option<i64>,
    last_seen_at: Option<i64>,
    key_version: Option<i32>,
    key_check: Option<Vec<u8>>,
    global_limit: Option<i32>,
    target_limit: Option<i32>,
    admitted: i32,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TargetRow {
    purpose: i16,
    target_digest: Vec<u8>,
    admitted: i32,
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    scopes: Vec<ScopeRow>,
    targets: Vec<TargetRow>,
}

async fn snapshot(pool: &PgPool) -> Snapshot {
    Snapshot {
        scopes: sqlx::query_as("SELECT purpose,window_start,last_seen_at,key_version,key_check,global_limit,target_limit,admitted FROM customer_credential_admission_scopes ORDER BY purpose")
            .fetch_all(pool).await.expect("all scope rows"),
        targets: sqlx::query_as("SELECT purpose,target_digest,admitted FROM customer_credential_admission_targets ORDER BY purpose,target_digest")
            .fetch_all(pool).await.expect("all target rows"),
    }
}

async fn claim(pool: &PgPool, email: &str, time: &dyn Clock) -> Result<(), AdmissionError> {
    admit(
        pool,
        CredentialPurpose::Login,
        email,
        &key(1, 7),
        limits(5, 3),
        time,
    )
    .await
}

async fn seed(pool: &PgPool) {
    claim(pool, "first@example.com", &clock(1000))
        .await
        .expect("seed admission");
}

async fn install_suppression(pool: &PgPool) {
    sqlx::query("CREATE FUNCTION suppress_admission_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(pool).await.expect("suppression function");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_target_insert_and_update_roll_back_exact_rows(pool: PgPool) {
    seed(&pool).await;
    install_suppression(&pool).await;
    for (create, email, expected) in [
        (
            "CREATE TRIGGER suppress_admission_target BEFORE INSERT ON customer_credential_admission_targets FOR EACH ROW EXECUTE FUNCTION suppress_admission_write()",
            "second@example.com",
            (2, 2, 2),
        ),
        (
            "CREATE TRIGGER suppress_admission_target BEFORE UPDATE ON customer_credential_admission_targets FOR EACH ROW EXECUTE FUNCTION suppress_admission_write()",
            "first@example.com",
            (3, 2, 3),
        ),
    ] {
        let before = snapshot(&pool).await;
        sqlx::query(create)
            .execute(&pool)
            .await
            .expect("install target suppression");
        assert_eq!(
            claim(&pool, email, &clock(1000)).await,
            Err(AdmissionError::Unavailable)
        );
        assert_eq!(
            snapshot(&pool).await,
            before,
            "failed target write rolls back every row"
        );
        sqlx::query(
            "DROP TRIGGER suppress_admission_target ON customer_credential_admission_targets",
        )
        .execute(&pool)
        .await
        .expect("remove target suppression");
        claim(&pool, email, &clock(1000))
            .await
            .expect("healthy target retry");
        assert_eq!(count(&pool, 1).await, expected);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_scope_update_rolls_back_exact_rows(pool: PgPool) {
    seed(&pool).await;
    let before = snapshot(&pool).await;
    install_suppression(&pool).await;
    sqlx::query("CREATE TRIGGER suppress_admission_scope BEFORE UPDATE ON customer_credential_admission_scopes FOR EACH ROW EXECUTE FUNCTION suppress_admission_write()")
        .execute(&pool).await.expect("install scope suppression");
    assert_eq!(
        claim(&pool, "second@example.com", &clock(1000)).await,
        Err(AdmissionError::Unavailable)
    );
    assert_eq!(
        snapshot(&pool).await,
        before,
        "target insert also rolls back"
    );
    sqlx::query("DROP TRIGGER suppress_admission_scope ON customer_credential_admission_scopes")
        .execute(&pool)
        .await
        .expect("remove scope suppression");
    claim(&pool, "second@example.com", &clock(1000))
        .await
        .expect("healthy scope retry");
    assert_eq!(count(&pool, 1).await, (2, 2, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_high_water_update_rolls_back_exact_rows(pool: PgPool) {
    seed(&pool).await;
    let before = snapshot(&pool).await;
    install_suppression(&pool).await;
    sqlx::query("CREATE TRIGGER suppress_admission_high_water BEFORE UPDATE ON customer_credential_admission_scopes FOR EACH ROW WHEN (NEW.admitted=OLD.admitted AND NEW.last_seen_at>OLD.last_seen_at) EXECUTE FUNCTION suppress_admission_write()")
        .execute(&pool).await.expect("install high-water-only suppression");
    let samples = ScriptedClock(std::sync::Mutex::new([1000, 1000, 1001, 1001, 1001].into()));
    assert_eq!(
        claim(&pool, "second@example.com", &samples).await,
        Err(AdmissionError::Unavailable)
    );
    assert_eq!(
        snapshot(&pool).await,
        before,
        "earlier counter writes roll back"
    );
    sqlx::query(
        "DROP TRIGGER suppress_admission_high_water ON customer_credential_admission_scopes",
    )
    .execute(&pool)
    .await
    .expect("remove high-water suppression");
    let samples = ScriptedClock(std::sync::Mutex::new([1000, 1000, 1001, 1001, 1001].into()));
    claim(&pool, "second@example.com", &samples)
        .await
        .expect("healthy high-water retry");
    assert_eq!(count(&pool, 1).await, (2, 2, 2));
    let last: i64 = sqlx::query_scalar(
        "SELECT last_seen_at FROM customer_credential_admission_scopes WHERE purpose=1",
    )
    .fetch_one(&pool)
    .await
    .expect("last observation");
    assert_eq!(last, 1001);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_expired_target_cleanup_rolls_back_exact_rows(pool: PgPool) {
    seed(&pool).await;
    claim(&pool, "second@example.com", &clock(1000))
        .await
        .expect("second old target");
    let before = snapshot(&pool).await;
    install_suppression(&pool).await;
    sqlx::query("CREATE TRIGGER suppress_admission_cleanup BEFORE DELETE ON customer_credential_admission_targets FOR EACH ROW EXECUTE FUNCTION suppress_admission_write()")
        .execute(&pool).await.expect("install cleanup suppression");
    assert_eq!(
        claim(&pool, "next@example.com", &clock(1020)).await,
        Err(AdmissionError::Unavailable)
    );
    assert_eq!(
        snapshot(&pool).await,
        before,
        "old scope and every target remain exact"
    );
    sqlx::query("DROP TRIGGER suppress_admission_cleanup ON customer_credential_admission_targets")
        .execute(&pool)
        .await
        .expect("remove cleanup suppression");
    claim(&pool, "next@example.com", &clock(1020))
        .await
        .expect("healthy rollover retry");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    let after = snapshot(&pool).await;
    assert_eq!(after.scopes[0].window_start, Some(1020));
    assert!(before.targets.iter().all(|old| {
        after
            .targets
            .iter()
            .all(|new| old.target_digest != new.target_digest)
    }));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_target_count_rolls_back_exact_rows(pool: PgPool) {
    seed(&pool).await;
    let before = snapshot(&pool).await;
    sqlx::query("CREATE FUNCTION alter_admission_target() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.admitted := NEW.admitted + 1; RETURN NEW; END $$")
        .execute(&pool).await.expect("alteration function");
    sqlx::query("CREATE TRIGGER alter_admission_target BEFORE INSERT ON customer_credential_admission_targets FOR EACH ROW EXECUTE FUNCTION alter_admission_target()")
        .execute(&pool).await.expect("install target alteration");
    assert_eq!(
        claim(&pool, "second@example.com", &clock(1000)).await,
        Err(AdmissionError::Unavailable)
    );
    assert_eq!(
        snapshot(&pool).await,
        before,
        "altered target never commits"
    );
    sqlx::query("DROP TRIGGER alter_admission_target ON customer_credential_admission_targets")
        .execute(&pool)
        .await
        .expect("remove target alteration");
    claim(&pool, "second@example.com", &clock(1000))
        .await
        .expect("healthy retry");
    assert_eq!(count(&pool, 1).await, (2, 2, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_initial_scope_prevents_protected_proof_issuance(pool: PgPool) {
    let before = snapshot(&pool).await;
    install_suppression(&pool).await;
    sqlx::query("CREATE TRIGGER suppress_admission_scope BEFORE UPDATE ON customer_credential_admission_scopes FOR EACH ROW WHEN (NEW.purpose=2) EXECUTE FUNCTION suppress_admission_write()")
        .execute(&pool).await.expect("install issue admission suppression");
    let time = clock(1000);
    let k = key(1, 7);
    let cap = limits(5, 3);
    assert!(matches!(
        commerce_infrastructure::email_verification::issue(
            &pool,
            "owner@example.com",
            &time,
            &k,
            cap
        )
        .await,
        Err(commerce_infrastructure::email_verification::VerificationError::Unavailable)
    ));
    assert_eq!(snapshot(&pool).await, before, "no quota grant commits");
    let proofs: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_challenges")
        .fetch_one(&pool)
        .await
        .expect("proof count");
    assert_eq!(proofs, 0, "protected proof issuance never starts");
    sqlx::query("DROP TRIGGER suppress_admission_scope ON customer_credential_admission_scopes")
        .execute(&pool)
        .await
        .expect("remove scope suppression");
    commerce_infrastructure::email_verification::issue(&pool, "owner@example.com", &time, &k, cap)
        .await
        .expect("healthy proof retry");
    assert_eq!(count(&pool, 2).await, (1, 1, 1));
    let proofs: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_challenges")
        .fetch_one(&pool)
        .await
        .expect("proof count");
    assert_eq!(proofs, 1);
}
