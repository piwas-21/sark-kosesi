use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::Money;
use commerce_infrastructure::{
    MIGRATOR,
    catalogue_prices::{PriceRevisionError, PriceRevisionRequest, record_price_revision},
    staff_access::{SessionToken, StaffError},
};
use sqlx::{Executor, PgPool};

struct FixedClock(u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
fn command(effect: &str) -> PriceRevisionRequest<'_> {
    PriceRevisionRequest {
        sku: "DEMO-COVERS",
        expected_revision: 0,
        effect_key: effect,
        price: Money::eur(12345).expect("independent minor units"),
        evidence_ref: "isolated-fixture-price-evidence",
        reason: "isolated price test",
        confirmed: true,
    }
}
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../tests/fixtures/bom.sql"))
        .await
        .expect("fixture");
    pool.execute("INSERT INTO staff_accounts VALUES ('fixture-catalogue',true),('fixture-warehouse',true),('fixture-other',true);
        INSERT INTO staff_capabilities VALUES ('fixture-catalogue','catalogue.manage'),('fixture-warehouse','warehouse.manage'),('fixture-other','catalogue.manage')")
        .await.expect("fixture grants");
    for (number, actor) in [
        (1, "fixture-catalogue"),
        (2, "fixture-warehouse"),
        (3, "fixture-other"),
    ] {
        sqlx::query("INSERT INTO staff_sessions (token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,1000,4600,1100)")
            .bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(actor).execute(pool).await.expect("fixture session");
    }
}
async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM catalogue_price_revisions")
        .fetch_one(pool)
        .await
        .expect("count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn revisions_preserve_prices_audit_actor_and_exact_replay_without_publishing(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    let before: Vec<(String, i64, bool)> =
        sqlx::query_as("SELECT sku,minor_units,fixture FROM catalogue_items ORDER BY sku")
            .fetch_all(&pool)
            .await
            .expect("preview oracle");
    let first = record_price_revision(&pool, &token, &FixedClock(1100), command("first"))
        .await
        .expect("first price");
    assert_eq!(
        (first.revision, first.price.minor_units(), first.replayed),
        (1, 12345, false)
    );
    let mut second = command("second");
    second.expected_revision = 1;
    second.price = Money::eur(14999).expect("oracle");
    let latest = record_price_revision(&pool, &token, &FixedClock(1100), second)
        .await
        .expect("new version");
    assert_eq!((latest.revision, latest.price.minor_units()), (2, 14999));
    let replay = record_price_revision(&pool, &token, &FixedClock(1100), command("first"))
        .await
        .expect("original receipt");
    assert_eq!(
        (replay.revision, replay.price.minor_units(), replay.replayed),
        (1, 12345, true)
    );
    let history:Vec<(i32,i64,String,String,String,bool)>=sqlx::query_as("SELECT revision,minor_units,actor,reason,evidence_ref,recorded_at IS NOT NULL FROM catalogue_price_revisions ORDER BY revision")
        .fetch_all(&pool).await.expect("history oracle");
    assert_eq!(
        history,
        vec![
            (
                1,
                12345,
                "fixture-catalogue".into(),
                "isolated price test".into(),
                "isolated-fixture-price-evidence".into(),
                true
            ),
            (
                2,
                14999,
                "fixture-catalogue".into(),
                "isolated price test".into(),
                "isolated-fixture-price-evidence".into(),
                true
            )
        ]
    );
    let after: Vec<(String, i64, bool)> =
        sqlx::query_as("SELECT sku,minor_units,fixture FROM catalogue_items ORDER BY sku")
            .fetch_all(&pool)
            .await
            .expect("unchanged preview");
    assert_eq!(after, before);
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn role_confirmation_bounds_and_missing_variants_fail_without_writes(pool: PgPool) {
    seed(&pool).await;
    for number in [2, 99] {
        assert!(matches!(
            record_price_revision(
                &pool,
                &SessionToken::from_bytes([number; 32]),
                &FixedClock(1100),
                command("no-authority")
            )
            .await,
            Err(PriceRevisionError::Staff(StaffError::Denied))
        ));
    }
    let token = SessionToken::from_bytes([1; 32]);
    for case in 0..15 {
        let mut request = command("invalid");
        let long_sku = "A".repeat(66);
        let long_key = "a".repeat(129);
        let long_evidence = "e".repeat(513);
        let long_reason = "r".repeat(501);
        match case {
            0 => request.confirmed = false,
            1 => request.expected_revision = -1,
            2 => request.expected_revision = i32::MAX,
            3 => request.sku = "",
            4 => request.sku = &long_sku,
            5 => request.effect_key = "",
            6 => request.effect_key = &long_key,
            7 => request.effect_key = "a_b",
            8 => request.effect_key = "é",
            9 => request.evidence_ref = "",
            10 => request.evidence_ref = " \t\n",
            11 => request.evidence_ref = &long_evidence,
            12 => request.reason = "",
            13 => request.reason = " \t\n",
            14 => request.reason = &long_reason,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                record_price_revision(&pool, &token, &FixedClock(1100), request).await,
                Err(PriceRevisionError::InvalidRequest)
            ),
            "case {case}"
        );
    }
    let mut missing = command("missing");
    missing.sku = "DEMO-ABSENT";
    assert!(matches!(
        record_price_revision(&pool, &token, &FixedClock(1100), missing).await,
        Err(PriceRevisionError::NotFound)
    ));
    assert_eq!(count(&pool).await, 0);
    // Unicode references count characters, not UTF-8 bytes; zero remains a draft fact.
    let evidence = "é".repeat(512);
    let reason = "é".repeat(500);
    let key = "x".repeat(128);
    let mut boundary = command(&key);
    boundary.evidence_ref = &evidence;
    boundary.reason = &reason;
    boundary.price = Money::eur(0).expect("oracle");
    assert_eq!(
        record_price_revision(&pool, &token, &FixedClock(1100), boundary)
            .await
            .expect("bounded draft")
            .price
            .minor_units(),
        0
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn effect_identity_and_stale_revision_conflicts_are_atomic(pool: PgPool) {
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    record_price_revision(&pool, &token, &FixedClock(1100), command("key"))
        .await
        .expect("seed price");
    for case in 0..7 {
        let mut request = command("key");
        match case {
            0 => request.sku = "DEMO-FILLED",
            1 => request.expected_revision = 1,
            2 => request.price = Money::eur(1).expect("oracle"),
            3 => request.evidence_ref = "different",
            4 => request.reason = "different",
            5 => request.effect_key = "fresh-stale",
            6 => {}
            _ => unreachable!(),
        }
        let other = SessionToken::from_bytes([3; 32]);
        let identity = if case == 6 { &other } else { &token };
        assert!(
            matches!(
                record_price_revision(&pool, identity, &FixedClock(1100), request).await,
                Err(PriceRevisionError::Conflict)
            ),
            "case {case}"
        );
    }
    sqlx::query("UPDATE staff_sessions SET revoked_at=1200 WHERE token_hash=$1")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("revoke");
    assert!(matches!(
        record_price_revision(&pool, &token, &FixedClock(1200), command("key")).await,
        Err(PriceRevisionError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_editors_and_retries_serialize_on_real_postgres(pool: PgPool) {
    seed(&pool).await;
    let left = pool.clone();
    let right = pool.clone();
    let (a, b) = tokio::join!(
        async move {
            record_price_revision(
                &left,
                &SessionToken::from_bytes([1; 32]),
                &FixedClock(1100),
                command("a"),
            )
            .await
        },
        async move {
            record_price_revision(
                &right,
                &SessionToken::from_bytes([1; 32]),
                &FixedClock(1100),
                command("b"),
            )
            .await
        }
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(PriceRevisionError::Conflict))
            || matches!(b, Err(PriceRevisionError::Conflict))
    );
    let mut next = command("same");
    next.expected_revision = 1;
    let mut retry = command("same");
    retry.expected_revision = 1;
    let token = SessionToken::from_bytes([1; 32]);
    let fixed_clock = FixedClock(1100);
    let (a, b) = tokio::join!(
        record_price_revision(&pool, &token, &fixed_clock, next),
        record_price_revision(&pool, &token, &fixed_clock, retry)
    );
    let a = a.expect("winner or replay");
    let b = b.expect("winner or replay");
    assert_eq!((a.revision, b.revision), (2, 2));
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_rejects_rewrites_skips_and_invalid_facts(pool: PgPool) {
    seed(&pool).await;
    record_price_revision(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        command("first"),
    )
    .await
    .expect("first");
    for invalid in [
        "UPDATE catalogue_price_revisions SET minor_units=1",
        "DELETE FROM catalogue_price_revisions",
        "INSERT INTO catalogue_price_revisions (sku,revision,effect_key,minor_units,evidence_ref,actor,reason) VALUES ('DEMO-COVERS',3,'skip',1,'e','fixture-catalogue','r')",
        "INSERT INTO catalogue_price_revisions (sku,revision,effect_key,minor_units,currency,evidence_ref,actor,reason) VALUES ('DEMO-COVERS',2,'currency',1,'USD','e','fixture-catalogue','r')",
        "INSERT INTO catalogue_price_revisions (sku,revision,effect_key,minor_units,evidence_ref,actor,reason) VALUES ('DEMO-COVERS',2,'negative',-1,'e','fixture-catalogue','r')",
        "INSERT INTO catalogue_price_revisions (sku,revision,effect_key,minor_units,evidence_ref,actor,reason) VALUES ('DEMO-COVERS',2,'empty',1,' ','fixture-catalogue','r')",
    ] {
        assert!(pool.execute(invalid).await.is_err(), "accepted {invalid}");
    }
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrations = false)]
async fn prior_schema_upgrade_preserves_catalogue_stock_and_starts_empty_price_history(
    pool: PgPool,
) {
    MIGRATOR.run_to(7, &pool).await.expect("prior schema");
    seed(&pool).await;
    pool.execute("INSERT INTO stock_balances VALUES ('DEMO-COVER',12,2,3)")
        .await
        .expect("prior stock");
    MIGRATOR.run(&pool).await.expect("price migration");
    assert_eq!(count(&pool).await, 0);
    let old: (i64, i64, i64) = sqlx::query_as(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code='DEMO-COVER'",
    )
    .fetch_one(&pool)
    .await
    .expect("stock oracle");
    assert_eq!(old, (12, 2, 3));
    let variants: i64 = sqlx::query_scalar("SELECT count(*) FROM catalogue_variants")
        .fetch_one(&pool)
        .await
        .expect("variant oracle");
    assert_eq!(variants, 2);
    assert_eq!(
        record_price_revision(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            command("after-upgrade")
        )
        .await
        .expect("guard usable after upgrade")
        .revision,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_or_clock_failure_before_commit_rolls_back_new_prices_and_denies_replay(
    pool: PgPool,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct StepClock {
        calls: AtomicUsize,
        last: Result<u64, ContextError>,
    }
    impl Clock for StepClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(1100)
            } else {
                self.last
            }
        }
    }
    seed(&pool).await;
    let token = SessionToken::from_bytes([1; 32]);
    for last in [Ok(2001), Err(ContextError::ClockUnavailable)] {
        let clock = StepClock {
            calls: AtomicUsize::new(0),
            last,
        };
        let result = record_price_revision(&pool, &token, &clock, command("must-rollback")).await;
        assert!(matches!(
            result,
            Err(PriceRevisionError::Staff(
                StaffError::Denied | StaffError::ClockUnavailable
            ))
        ));
        assert_eq!(count(&pool).await, 0);
    }
    record_price_revision(&pool, &token, &FixedClock(1100), command("first"))
        .await
        .expect("seed receipt");
    let clock = StepClock {
        calls: AtomicUsize::new(0),
        last: Ok(2001),
    };
    assert!(matches!(
        record_price_revision(&pool, &token, &clock, command("first")).await,
        Err(PriceRevisionError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}

async fn wait_for_lock(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_millis(800),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)")
                .bind(pattern).fetch_one(pool).await.expect("lock probe");
            if waiting { break; } tokio::task::yield_now().await;
        }
    }).await.expect("operation reached lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_during_sku_or_effect_lock_wait_never_commits_a_price(pool: PgPool) {
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
    for (block_sql, pattern) in [
        (
            "SELECT sku FROM catalogue_variants WHERE sku='DEMO-COVERS' FOR UPDATE",
            "SELECT sku FROM catalogue_variants%",
        ),
        (
            "SELECT pg_advisory_xact_lock(hashtextextended('catalogue-price:wait',0))",
            "SELECT pg_advisory_xact_lock%",
        ),
    ] {
        let mut blocker = pool.begin().await.expect("blocker");
        blocker.execute(block_sql).await.expect("block price");
        let current = Arc::new(MovingClock(AtomicU64::new(1100)));
        let writer_clock = current.clone();
        let writer_pool = pool.clone();
        let task = tokio::spawn(async move {
            record_price_revision(
                &writer_pool,
                &SessionToken::from_bytes([1; 32]),
                writer_clock.as_ref(),
                command("wait"),
            )
            .await
        });
        wait_for_lock(&pool, pattern).await;
        current.0.store(2001, Ordering::SeqCst);
        blocker.rollback().await.expect("resume after expiry");
        assert!(matches!(
            task.await.expect("writer"),
            Err(PriceRevisionError::Staff(StaffError::Denied))
        ));
        assert_eq!(count(&pool).await, 0);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_revocation_waits_for_price_commit_then_blocks_retry(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("SKU blocker");
    blocker
        .execute("SELECT sku FROM catalogue_variants WHERE sku='DEMO-COVERS' FOR UPDATE")
        .await
        .expect("pause write");
    let writer_pool = pool.clone();
    let write = tokio::spawn(async move {
        record_price_revision(
            &writer_pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            command("before-revoke"),
        )
        .await
    });
    wait_for_lock(&pool, "SELECT sku FROM catalogue_variants%").await;
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='fixture-catalogue'")
            .execute(&revoke_pool)
            .await
    });
    wait_for_lock(&pool, "DELETE FROM staff_capabilities%").await;
    assert_eq!(count(&pool).await, 0);
    blocker.rollback().await.expect("allow write");
    assert_eq!(
        write
            .await
            .expect("write")
            .expect("authorized commit")
            .revision,
        1
    );
    revoke
        .await
        .expect("revoke task")
        .expect("grant removal after commit");
    assert!(matches!(
        record_price_revision(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            command("before-revoke")
        )
        .await,
        Err(PriceRevisionError::Staff(StaffError::Denied))
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn database_faults_and_lock_timeouts_leave_no_partial_price_history(pool: PgPool) {
    seed(&pool).await;
    let mut blocker = pool.begin().await.expect("effect blocker");
    blocker
        .execute("SELECT pg_advisory_xact_lock(hashtextextended('catalogue-price:locked',0))")
        .await
        .expect("lock");
    let result = record_price_revision(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        command("locked"),
    )
    .await;
    match result {
        Err(PriceRevisionError::Database(error)) => assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        ),
        other => panic!("unexpected {other:?}"),
    }
    blocker.rollback().await.expect("unlock");
    assert_eq!(count(&pool).await, 0);
    pool.execute("ALTER TABLE catalogue_price_revisions RENAME TO unavailable_price_revisions")
        .await
        .expect("dependency failure");
    assert!(matches!(
        record_price_revision(
            &pool,
            &SessionToken::from_bytes([1; 32]),
            &FixedClock(1100),
            command("missing-table")
        )
        .await,
        Err(PriceRevisionError::Database(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_repeatable_read_import_cannot_advance_price_history(pool: PgPool) {
    seed(&pool).await;
    let mut stale = pool.begin().await.expect("stale importer");
    stale
        .execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("repeatable read");
    let (old,): (i64,) = sqlx::query_as("SELECT count(*) FROM catalogue_price_revisions")
        .fetch_one(&mut *stale)
        .await
        .expect("old snapshot");
    assert_eq!(old, 0);
    record_price_revision(
        &pool,
        &SessionToken::from_bytes([1; 32]),
        &FixedClock(1100),
        command("first"),
    )
    .await
    .expect("intervening writer");
    let error=stale.execute("INSERT INTO catalogue_price_revisions (sku,revision,effect_key,minor_units,evidence_ref,actor,reason) VALUES ('DEMO-COVERS',2,'stale-import',5,'e','fixture-catalogue','r')").await.expect_err("stale importer must retry");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("40001")
    );
    stale.rollback().await.expect("rollback");
    assert_eq!(count(&pool).await, 1);
}
