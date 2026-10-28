use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::inventory::WarehouseChange;
use commerce_infrastructure::{
    MIGRATOR,
    staff_access::{SessionToken, StaffError},
    staff_operations::{StaffWarehouseRequest, catalogue_bom, warehouse_change},
};
use sqlx::{Executor, PgPool};

struct FixedClock(Result<u64, ContextError>);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
    }
}
fn clock(now: u64) -> FixedClock {
    FixedClock(Ok(now))
}
fn command(effect: &str) -> StaffWarehouseRequest<'_> {
    StaffWarehouseRequest {
        effect_key: effect,
        component_code: "DEMO-COVER",
        change: WarehouseChange::Receive(4),
        reason: "isolated staff test",
    }
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("fixture");
    pool.execute("INSERT INTO staff_accounts VALUES ('fixture-catalogue',true),('fixture-warehouse',true),('fixture-unprivileged',true);
        INSERT INTO staff_capabilities VALUES ('fixture-catalogue','catalogue.manage'),('fixture-warehouse','warehouse.manage')")
        .await.expect("isolated staff grants");
    for (number, actor) in [
        (1, "fixture-catalogue"),
        (2, "fixture-warehouse"),
        (3, "fixture-unprivileged"),
    ] {
        sqlx::query("INSERT INTO staff_sessions (token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,1000,4600,1100)")
            .bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("test-only session attestation");
    }
}
async fn effect_count(pool: &PgPool) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_movements")
        .fetch_one(pool)
        .await
        .expect("effect count");
    count
}

#[test]
fn session_digest_matches_an_independent_sha256_fixture() {
    let expected: [u8; 32] = [
        0x66, 0x68, 0x7a, 0xad, 0xf8, 0x62, 0xbd, 0x77, 0x6c, 0x8f, 0xc1, 0x8b, 0x8e, 0x9f, 0x8e,
        0x20, 0x08, 0x97, 0x14, 0x85, 0x6e, 0xe2, 0x33, 0xb3, 0x90, 0x2a, 0x59, 0x1d, 0x0d, 0x5f,
        0x29, 0x25,
    ];
    assert_eq!(SessionToken::from_bytes([0; 32]).digest(), expected);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn catalogue_and_warehouse_grants_are_separate_and_actor_is_server_owned(pool: PgPool) {
    seed(&pool).await;
    let catalogue = SessionToken::from_bytes([1; 32]);
    let warehouse = SessionToken::from_bytes([2; 32]);
    for (sku, revision) in [
        ("", 1),
        ("DEMO-COVERS", 0),
        (
            "long-long-long-long-long-long-long-long-long-long-long-long-long-long",
            1,
        ),
    ] {
        assert!(matches!(
            catalogue_bom(&pool, &catalogue, &clock(1100), sku, revision).await,
            Err(StaffError::InvalidRequest)
        ));
    }
    assert_eq!(
        catalogue_bom(&pool, &catalogue, &clock(1100), "DEMO-FILLED", 1)
            .await
            .expect("catalogue role")
            .expect("BOM")
            .components
            .len(),
        2
    );
    assert_eq!(
        catalogue_bom(&pool, &catalogue, &clock(1100), "DEMO-MISSING", 1)
            .await
            .expect("bounded missing"),
        None
    );
    assert!(matches!(
        catalogue_bom(&pool, &warehouse, &clock(1100), "DEMO-FILLED", 1).await,
        Err(StaffError::Denied)
    ));
    assert!(matches!(
        warehouse_change(&pool, &catalogue, &clock(1100), command("catalogue-denied")).await,
        Err(StaffError::Denied)
    ));
    let received = warehouse_change(
        &pool,
        &warehouse,
        &clock(1100),
        command("warehouse-receive"),
    )
    .await
    .expect("warehouse grant");
    assert_eq!(received.balance.on_hand(), 4);
    assert!(
        warehouse_change(
            &pool,
            &warehouse,
            &clock(1100),
            command("warehouse-receive")
        )
        .await
        .expect("exact retry")
        .replayed
    );
    let (actor,): (String,) =
        sqlx::query_as("SELECT actor FROM stock_movements WHERE effect_key='warehouse-receive'")
            .fetch_one(&pool)
            .await
            .expect("audit actor");
    assert_eq!(actor, "fixture-warehouse");
    assert_eq!(effect_count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn absent_wrong_expired_revoked_disabled_and_unverified_sessions_cannot_write(pool: PgPool) {
    seed(&pool).await;
    for secret in [3, 99] {
        assert!(matches!(
            warehouse_change(
                &pool,
                &SessionToken::from_bytes([secret; 32]),
                &clock(1100),
                command("denied")
            )
            .await,
            Err(StaffError::Denied)
        ));
    }
    let token = SessionToken::from_bytes([2; 32]);
    for now in [999, 1099, 2001, 4600] {
        assert!(matches!(
            warehouse_change(&pool, &token, &clock(now), command("denied")).await,
            Err(StaffError::Denied)
        ));
    }
    pool.execute(
        "UPDATE staff_sessions SET mfa_verified_at=NULL WHERE staff_id='fixture-warehouse'",
    )
    .await
    .expect("not verified");
    assert!(matches!(
        warehouse_change(&pool, &token, &clock(1100), command("denied")).await,
        Err(StaffError::Denied)
    ));
    pool.execute("UPDATE staff_sessions SET mfa_verified_at=1100,revoked_at=1100 WHERE staff_id='fixture-warehouse'").await.expect("revoked");
    assert!(matches!(
        warehouse_change(&pool, &token, &clock(1100), command("denied")).await,
        Err(StaffError::Denied)
    ));
    pool.execute("UPDATE staff_sessions SET revoked_at=NULL WHERE staff_id='fixture-warehouse'; UPDATE staff_accounts SET enabled=false WHERE id='fixture-warehouse'").await.expect("disabled");
    assert!(matches!(
        warehouse_change(&pool, &token, &clock(1100), command("denied")).await,
        Err(StaffError::Denied)
    ));
    assert_eq!(effect_count(&pool).await, 0);
    pool.execute("UPDATE staff_accounts SET enabled=true WHERE id='fixture-warehouse'")
        .await
        .expect("activate fixture");
    assert_eq!(
        warehouse_change(&pool, &token, &clock(2000), command("boundary"))
            .await
            .expect("exact MFA limit")
            .balance
            .on_hand(),
        4
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_and_database_failures_are_explicit_and_failed_stock_work_rolls_back(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([2; 32]);
    for clock in [
        FixedClock(Err(ContextError::ClockUnavailable)),
        clock(u64::MAX),
    ] {
        assert!(matches!(
            warehouse_change(&pool, &token, &clock, command("clock-error")).await,
            Err(StaffError::ClockUnavailable)
        ));
    }
    let invalid = StaffWarehouseRequest {
        change: WarehouseChange::Receive(0),
        ..command("invalid")
    };
    assert!(matches!(
        warehouse_change(&pool, &token, &clock(1100), invalid).await,
        Err(StaffError::Stock(_))
    ));
    assert_eq!(effect_count(&pool).await, 0);
    pool.execute("ALTER TABLE catalogue_boms RENAME TO unavailable_boms")
        .await
        .expect("database failure control");
    assert!(matches!(
        catalogue_bom(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &clock(1100),
            "DEMO-COVERS",
            1
        )
        .await,
        Err(StaffError::Database(_))
    ));
    pool.close().await;
    assert!(matches!(
        warehouse_change(&pool, &token, &clock(1100), command("closed")).await,
        Err(StaffError::Database(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn persisted_session_identity_and_grant_constraints_reject_tampering(pool: PgPool) {
    seed(&pool).await;
    for invalid in [
        "UPDATE staff_sessions SET staff_id='fixture-unprivileged' WHERE staff_id='fixture-warehouse'",
        "UPDATE staff_sessions SET token_hash=decode(repeat('01',32),'hex') WHERE staff_id='fixture-warehouse'",
        "UPDATE staff_sessions SET created_at=999",
        "UPDATE staff_sessions SET expires_at=1000",
        "UPDATE staff_sessions SET expires_at=44201",
        "UPDATE staff_sessions SET mfa_verified_at=999",
        "UPDATE staff_sessions SET mfa_verified_at=4600",
        "INSERT INTO staff_capabilities VALUES ('fixture-unprivileged','owner-all')",
        "INSERT INTO staff_sessions VALUES (decode('00','hex'),'fixture-warehouse',1000,4600,1100,NULL)",
    ] {
        assert!(
            pool.execute(invalid).await.is_err(),
            "accepted invalid SQL {invalid}"
        );
    }
    let (raw_matches,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM staff_sessions WHERE token_hash=decode(repeat('02',32),'hex')",
    )
    .fetch_one(&pool)
    .await
    .expect("hash-only storage");
    assert_eq!(raw_matches, 0);
    assert_eq!(effect_count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn security_revocation_waits_for_the_authorized_stock_commit(pool: PgPool) {
    seed(&pool).await;
    pool.execute("CREATE FUNCTION pause_staff_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(812347); RETURN NEW; END; $$; CREATE TRIGGER pause_staff_write BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION pause_staff_write()")
        .await.expect("write timing control");
    for (effect, revoke_sql, expected) in [
        (
            "session-change",
            "UPDATE staff_sessions SET revoked_at=1101 WHERE staff_id='fixture-warehouse'",
            4,
        ),
        (
            "account-change",
            "UPDATE staff_accounts SET enabled=false WHERE id='fixture-warehouse'",
            8,
        ),
        (
            "grant-change",
            "DELETE FROM staff_capabilities WHERE staff_id='fixture-warehouse'",
            12,
        ),
    ] {
        pool.execute("UPDATE staff_sessions SET revoked_at=NULL; UPDATE staff_accounts SET enabled=true; INSERT INTO staff_capabilities VALUES ('fixture-warehouse','warehouse.manage') ON CONFLICT DO NOTHING").await.expect("reset controlled fixture");
        let mut blocker = pool.begin().await.expect("blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(812347)")
            .execute(&mut *blocker)
            .await
            .expect("pause effect");
        let write_pool = pool.clone();
        let write = tokio::spawn(async move {
            warehouse_change(
                &write_pool,
                &SessionToken::from_bytes([2; 32]),
                &clock(1100),
                command(effect),
            )
            .await
        });
        wait_for_lock(&pool,"SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory')").await;
        let revoke_pool = pool.clone();
        let revoke = tokio::spawn(async move { revoke_pool.execute(revoke_sql).await });
        wait_for_lock(&pool,"SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND (query LIKE 'UPDATE staff_sessions%' OR query LIKE 'UPDATE staff_accounts%' OR query LIKE 'DELETE FROM staff_capabilities%'))").await;
        blocker.rollback().await.expect("resume write");
        assert_eq!(
            write
                .await
                .expect("write task")
                .expect("authorized before revoke")
                .balance
                .on_hand(),
            expected
        );
        revoke
            .await
            .expect("revoke task")
            .expect("revoke after commit");
        for key in [effect, "new-denied-effect"] {
            assert!(matches!(
                warehouse_change(
                    &pool,
                    &SessionToken::from_bytes([2; 32]),
                    &clock(1101),
                    command(key)
                )
                .await,
                Err(StaffError::Denied)
            ));
        }
    }
    assert_eq!(effect_count(&pool).await, 3);
}

async fn wait_for_lock(pool: &PgPool, query: &'static str) {
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let (waiting,): (bool,) = sqlx::query_as(query)
                .fetch_one(pool)
                .await
                .expect("lock probe");
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("contender reached database lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn revocation_committed_while_authorization_waits_is_observed_before_effects(pool: PgPool) {
    seed(&pool).await;
    for revoke_sql in [
        "UPDATE staff_sessions SET revoked_at=1100 WHERE staff_id='fixture-warehouse'",
        "UPDATE staff_accounts SET enabled=false WHERE id='fixture-warehouse'",
        "DELETE FROM staff_capabilities WHERE staff_id='fixture-warehouse'",
    ] {
        pool.execute("UPDATE staff_sessions SET revoked_at=NULL; UPDATE staff_accounts SET enabled=true; INSERT INTO staff_capabilities VALUES ('fixture-warehouse','warehouse.manage') ON CONFLICT DO NOTHING").await.expect("reset fixture");
        let mut revoke = pool.begin().await.expect("revoker");
        revoke
            .execute(revoke_sql)
            .await
            .expect("pending revocation");
        let writer_pool = pool.clone();
        let writer = tokio::spawn(async move {
            warehouse_change(
                &writer_pool,
                &SessionToken::from_bytes([2; 32]),
                &clock(1100),
                command("must-not-apply"),
            )
            .await
        });
        wait_for_lock(&pool,"SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT s.staff_id%')").await;
        revoke
            .commit()
            .await
            .expect("commit revocation before authorization");
        assert!(matches!(
            writer.await.expect("writer"),
            Err(StaffError::Denied)
        ));
        assert_eq!(effect_count(&pool).await, 0);
    }
}

#[sqlx::test(migrations = false)]
async fn access_upgrade_preserves_existing_inventory_and_starts_without_staff_authority(
    pool: PgPool,
) {
    MIGRATOR.run_to(6, &pool).await.expect("stock schema");
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("old catalogue");
    MIGRATOR.run(&pool).await.expect("expand staff tables");
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM staff_accounts),(SELECT count(*) FROM staff_sessions),(SELECT count(*) FROM staff_capabilities)").fetch_one(&pool).await.expect("no invented grants");
    assert_eq!(counts, (0, 0, 0));
    assert!(matches!(
        warehouse_change(
            &pool,
            &SessionToken::from_bytes([2; 32]),
            &clock(1100),
            command("no-session")
        )
        .await,
        Err(StaffError::Denied)
    ));
    assert_eq!(
        commerce_infrastructure::bom::bill_of_materials(&pool, "DEMO-FILLED", 1)
            .await
            .expect("old reader")
            .expect("BOM")
            .components
            .len(),
        2
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_while_waiting_for_authority_is_checked_before_stock_effects(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct MovingClock(AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    seed(&pool).await;
    for now in [2001, 4600] {
        let current = Arc::new(MovingClock(AtomicU64::new(1100)));
        let mut blocker = pool.begin().await.expect("account blocker");
        sqlx::query("UPDATE staff_accounts SET enabled=enabled WHERE id='fixture-warehouse'")
            .execute(&mut *blocker)
            .await
            .expect("pause authority lookup without revoking");
        let writer_pool = pool.clone();
        let writer_clock = current.clone();
        let writer = tokio::spawn(async move {
            warehouse_change(
                &writer_pool,
                &SessionToken::from_bytes([2; 32]),
                writer_clock.as_ref(),
                command("expired-during-lookup"),
            )
            .await
        });
        wait_for_lock(&pool,"SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT s.staff_id%')").await;
        current.0.store(now, Ordering::SeqCst);
        blocker
            .commit()
            .await
            .expect("resume lookup after clock advanced");
        assert!(
            matches!(writer.await.expect("writer"), Err(StaffError::Denied)),
            "expiry during an authorization wait cannot use an older clock capture"
        );
        assert_eq!(effect_count(&pool).await, 0);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn locked_authority_storage_fails_closed_with_a_bounded_database_error(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("authority blocker");
    sqlx::query("LOCK TABLE staff_sessions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("hold dependency lock");
    let result = warehouse_change(
        &pool,
        &SessionToken::from_bytes([2; 32]),
        &clock(1100),
        command("locked-authority"),
    )
    .await;
    match result {
        Err(StaffError::Database(error)) => assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        ),
        other => panic!("expected bounded lock error, got {other:?}"),
    }
    blocker.rollback().await.expect("release");
    assert_eq!(effect_count(&pool).await, 0);
    assert_eq!(
        warehouse_change(
            &pool,
            &SessionToken::from_bytes([2; 32]),
            &clock(1100),
            command("recovered-authority")
        )
        .await
        .expect("recovery")
        .balance
        .on_hand(),
        4
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_grant_storage_never_falls_back_to_an_implicit_role(pool: PgPool) {
    seed(&pool).await;
    pool.execute("ALTER TABLE staff_capabilities RENAME TO unavailable_staff_capabilities")
        .await
        .expect("missing dependency control");
    let result = warehouse_change(
        &pool,
        &SessionToken::from_bytes([2; 32]),
        &clock(1100),
        command("missing-grant-storage"),
    )
    .await;
    match result {
        Err(StaffError::Database(error)) => assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42P01")
        ),
        other => panic!("expected unavailable grant storage, got {other:?}"),
    }
    assert_eq!(effect_count(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_during_an_inventory_wait_rolls_back_pending_effects(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct MovingClock(AtomicU64);
    impl Clock for MovingClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }
    seed(&pool).await;
    pool.execute("CREATE FUNCTION pause_staff_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(812348); RETURN NEW; END; $$;
        CREATE TRIGGER pause_staff_commit BEFORE INSERT ON stock_movements FOR EACH ROW EXECUTE FUNCTION pause_staff_commit()")
        .await.expect("stock commit timing control");
    for now in [2001, 4600] {
        let current = Arc::new(MovingClock(AtomicU64::new(1100)));
        let mut blocker = pool.begin().await.expect("blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(812348)")
            .execute(&mut *blocker)
            .await
            .expect("pause effect");
        let writer_pool = pool.clone();
        let writer_clock = current.clone();
        let writer = tokio::spawn(async move {
            warehouse_change(
                &writer_pool,
                &SessionToken::from_bytes([2; 32]),
                writer_clock.as_ref(),
                command("expired-before-commit"),
            )
            .await
        });
        wait_for_lock(&pool,"SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory')").await;
        current.0.store(now, Ordering::SeqCst);
        blocker.rollback().await.expect("resume after expiry");
        assert!(matches!(
            writer.await.expect("writer"),
            Err(StaffError::Denied)
        ));
        assert_eq!(effect_count(&pool).await, 0);
        let (balances,): (i64,) = sqlx::query_as("SELECT count(*) FROM stock_balances")
            .fetch_one(&pool)
            .await
            .expect("no partial balance");
        assert_eq!(balances, 0);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_failure_before_commit_or_private_read_return_never_exposes_a_result(pool: PgPool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct FailingSecondClock(AtomicUsize);
    impl Clock for FailingSecondClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(1100)
            } else {
                Err(ContextError::ClockUnavailable)
            }
        }
    }
    seed(&pool).await;
    assert!(matches!(
        warehouse_change(
            &pool,
            &SessionToken::from_bytes([2; 32]),
            &FailingSecondClock(AtomicUsize::new(0)),
            command("no-clock-at-commit")
        )
        .await,
        Err(StaffError::ClockUnavailable)
    ));
    assert_eq!(effect_count(&pool).await, 0);
    assert!(matches!(
        catalogue_bom(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FailingSecondClock(AtomicUsize::new(0)),
            "DEMO-COVERS",
            1
        )
        .await,
        Err(StaffError::ClockUnavailable)
    ));
}
