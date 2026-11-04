//! Admission is committed in PostgreSQL before credential work can start.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{
        AdmissionError, AdmissionKey, AdmissionLimits, CredentialPurpose, admit,
    },
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn clock(at: u64) -> TestClock {
    TestClock(AtomicU64::new(at))
}
fn key(version: i32, byte: u8) -> AdmissionKey {
    AdmissionKey::new(version, [byte; 32]).expect("valid key")
}
fn limits(global: i32, target: i32) -> AdmissionLimits {
    AdmissionLimits::new(global, target).expect("valid limits")
}
async fn count(pool: &PgPool, purpose: i16) -> (i32, i64, i64) {
    sqlx::query_as("SELECT s.admitted, count(t.target_digest), coalesce(sum(t.admitted),0)::bigint FROM customer_credential_admission_scopes s LEFT JOIN customer_credential_admission_targets t ON t.purpose=s.purpose WHERE s.purpose=$1 GROUP BY s.purpose,s.admitted")
        .bind(purpose).fetch_one(pool).await.expect("admission counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_requests_share_target_and_global_capacity_across_pools(pool: PgPool) {
    let other = PgPoolOptions::new()
        .max_connections(12)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .expect("second isolated pool");
    let time = Arc::new(clock(1000));
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..24 {
        let p = if n % 2 == 0 {
            pool.clone()
        } else {
            other.clone()
        };
        let t = time.clone();
        tasks.spawn(async move {
            admit(
                &p,
                CredentialPurpose::Login,
                "Same@EXAMPLE.com",
                &key(1, 7),
                limits(20, 4),
                t.as_ref(),
            )
            .await
        });
    }
    let mut accepted = 0;
    let mut limited = 0;
    while let Some(result) = tasks.join_next().await {
        match result.expect("task") {
            Ok(()) => accepted += 1,
            Err(AdmissionError::Limited) => limited += 1,
            _ => panic!("unexpected admission outcome"),
        }
    }
    assert_eq!((accepted, limited), (4, 20));
    assert_eq!(count(&pool, 1).await, (4, 1, 4));

    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..32 {
        let p = if n % 2 == 0 {
            pool.clone()
        } else {
            other.clone()
        };
        let t = time.clone();
        tasks.spawn(async move {
            let email = format!("distinct{n}@example.com");
            admit(
                &p,
                CredentialPurpose::Login,
                &email,
                &key(1, 7),
                limits(20, 4),
                t.as_ref(),
            )
            .await
        });
    }
    let mut accepted = 0;
    let mut limited = 0;
    while let Some(result) = tasks.join_next().await {
        match result.expect("task") {
            Ok(()) => accepted += 1,
            Err(AdmissionError::Limited) => limited += 1,
            _ => panic!("unexpected admission outcome"),
        }
    }
    assert_eq!((accepted, limited), (16, 16));
    assert_eq!(count(&pool, 1).await, (20, 17, 20));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn normalized_targets_invalid_bucket_and_purposes_are_independent(pool: PgPool) {
    let time = clock(1000);
    let k = key(1, 7);
    let cap = limits(10, 1);
    admit(
        &pool,
        CredentialPurpose::Login,
        "Case@EXAMPLE.COM",
        &k,
        cap,
        &time,
    )
    .await
    .expect("first");
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "Case@example.com",
            &k,
            cap,
            &time
        )
        .await,
        Err(AdmissionError::Limited)
    ));
    admit(
        &pool,
        CredentialPurpose::Login,
        "case@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("local part distinct");
    admit(
        &pool,
        CredentialPurpose::Login,
        "not an email",
        &k,
        cap,
        &time,
    )
    .await
    .expect("invalid bucket");
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "also-invalid",
            &k,
            cap,
            &time
        )
        .await,
        Err(AdmissionError::Limited)
    ));
    for purpose in [
        CredentialPurpose::EmailVerificationIssue,
        CredentialPurpose::EmailVerificationConsume,
        CredentialPurpose::PasswordProofIssue,
        CredentialPurpose::PasswordReplace,
    ] {
        admit(&pool, purpose, "Case@example.com", &k, cap, &time)
            .await
            .expect("purpose isolated");
    }
    assert_eq!(count(&pool, 1).await, (3, 3, 3));
    for purpose in 2..=5 {
        assert_eq!(count(&pool, purpose).await, (1, 1, 1));
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn window_pins_key_and_limits_retires_targets_and_rejects_stale_time_or_version(
    pool: PgPool,
) {
    let time = clock(1000);
    let k1 = key(1, 7);
    let k2 = key(2, 8);
    let cap = limits(4, 2);
    admit(
        &pool,
        CredentialPurpose::Login,
        "old@example.com",
        &k1,
        cap,
        &time,
    )
    .await
    .expect("initial");
    for (candidate, config) in [
        (&key(1, 8), cap),
        (&k2, cap),
        (&k1, limits(5, 2)),
        (&k1, limits(4, 1)),
    ] {
        assert!(matches!(
            admit(
                &pool,
                CredentialPurpose::Login,
                "other@example.com",
                candidate,
                config,
                &time
            )
            .await,
            Err(AdmissionError::Unavailable)
        ));
    }
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    time.0.store(999, Ordering::SeqCst);
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "other@example.com",
            &k1,
            cap,
            &time
        )
        .await,
        Err(AdmissionError::Unavailable)
    ));
    time.0.store(1020, Ordering::SeqCst);
    admit(
        &pool,
        CredentialPurpose::Login,
        "new@example.com",
        &k2,
        limits(5, 3),
        &time,
    )
    .await
    .expect("new window and key");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    time.0.store(1080, Ordering::SeqCst);
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "stale@example.com",
            &k1,
            cap,
            &time
        )
        .await,
        Err(AdmissionError::Unavailable)
    ));
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "same-version-wrong-key@example.com",
            &key(2, 9),
            limits(3, 1),
            &time,
        )
        .await,
        Err(AdmissionError::Unavailable)
    ));
    admit(
        &pool,
        CredentialPurpose::Login,
        "newer@example.com",
        &k2,
        limits(3, 1),
        &time,
    )
    .await
    .expect("same key, new limits");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn held_scope_lock_uses_time_after_wait_and_retires_old_targets(pool: PgPool) {
    let time = Arc::new(clock(1000));
    admit(
        &pool,
        CredentialPurpose::Login,
        "same@example.com",
        &key(1, 7),
        limits(1, 1),
        time.as_ref(),
    )
    .await
    .expect("old window full");
    let digest_query =
        "SELECT target_digest FROM customer_credential_admission_targets WHERE purpose=1";
    let old_digest: Vec<u8> = sqlx::query_scalar(digest_query)
        .fetch_one(&pool)
        .await
        .expect("old window digest");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query(
        "SELECT purpose FROM customer_credential_admission_scopes WHERE purpose=1 FOR UPDATE",
    )
    .fetch_one(&mut *blocker)
    .await
    .expect("hold scope");
    let p = pool.clone();
    let t = time.clone();
    let task = tokio::spawn(async move {
        admit(
            &p,
            CredentialPurpose::Login,
            "same@example.com",
            &key(1, 7),
            limits(1, 1),
            t.as_ref(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query ILIKE '%customer_credential_admission_scopes%')").fetch_one(&pool).await.expect("lock observation");
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("actual scope lock wait");
    assert!(!task.is_finished());
    time.0.store(1020, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    task.await.expect("join").expect("new window admitted");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    let new_digest: Vec<u8> = sqlx::query_scalar(digest_query)
        .fetch_one(&pool)
        .await
        .expect("new window digest");
    assert_ne!(old_digest, new_digest, "digest rotates with the window");
    let stored: (i64, i64) = sqlx::query_as("SELECT window_start,last_seen_at FROM customer_credential_admission_scopes WHERE purpose=1").fetch_one(&pool).await.expect("window");
    assert_eq!(stored, (1020, 1020));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn database_constraints_and_private_storage_have_positive_controls(pool: PgPool) {
    assert!(AdmissionKey::new(0, [1; 32]).is_none());
    assert!(AdmissionKey::new(1, [0; 32]).is_none());
    assert!(AdmissionLimits::new(0, 1).is_none());
    assert!(AdmissionLimits::new(1, 2).is_none());
    assert!(AdmissionLimits::new(1001, 1).is_none());
    assert!(AdmissionLimits::new(101, 101).is_none());
    let time = clock(1000);
    admit(
        &pool,
        CredentialPurpose::Login,
        "private@example.com",
        &key(1, b'K'),
        limits(3, 2),
        &time,
    )
    .await
    .expect("valid positive control");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    let stored: String = sqlx::query_scalar("SELECT coalesce(string_agg(row_to_json(t)::text,''),'') FROM (SELECT * FROM customer_credential_admission_scopes UNION ALL SELECT purpose,NULL,NULL,NULL,target_digest,NULL,NULL,admitted FROM customer_credential_admission_targets) t").fetch_one(&pool).await.expect("serialized rows");
    assert!(!stored.contains("private@example.com"));
    assert!(!stored.contains(&"4b".repeat(32)), "raw key absent");
    assert!(sqlx::query("UPDATE customer_credential_admission_scopes SET admitted=global_limit+1 WHERE purpose=1").execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO customer_credential_admission_targets(purpose,target_digest,admitted) VALUES (1,decode('01','hex'),1)").execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO customer_credential_admission_targets(purpose,target_digest,admitted) VALUES (6,decode(repeat('01',32),'hex'),1)").execute(&pool).await.is_err());
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
}

struct UnavailableClock;
impl Clock for UnavailableClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Err(ContextError::ClockUnavailable)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_or_database_failure_never_grants_admission(pool: PgPool) {
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "a@example.com",
            &key(1, 7),
            limits(3, 2),
            &UnavailableClock
        )
        .await,
        Err(AdmissionError::Unavailable)
    ));
    assert_eq!(count(&pool, 1).await, (0, 0, 0));
    sqlx::query("DROP TABLE customer_credential_admission_targets")
        .execute(&pool)
        .await
        .expect("simulate unavailable target store");
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "a@example.com",
            &key(1, 7),
            limits(3, 2),
            &clock(1000)
        )
        .await,
        Err(AdmissionError::Unavailable)
    ));
    let total: i32 = sqlx::query_scalar(
        "SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=1",
    )
    .fetch_one(&pool)
    .await
    .expect("scope after database failure");
    assert_eq!(total, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn final_write_wait_cannot_commit_a_claim_in_an_expired_window(pool: PgPool) {
    let time = Arc::new(clock(1000));
    admit(
        &pool,
        CredentialPurpose::Login,
        "first@example.com",
        &key(1, 7),
        limits(3, 2),
        time.as_ref(),
    )
    .await
    .expect("initial claim");
    sqlx::query("CREATE FUNCTION hold_admission_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(274910); RETURN NEW; END $$")
        .execute(&pool).await.expect("test fence function");
    sqlx::query("CREATE TRIGGER hold_admission_write AFTER UPDATE ON customer_credential_admission_scopes FOR EACH ROW EXECUTE FUNCTION hold_admission_write()")
        .execute(&pool).await.expect("test fence trigger");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(274910)")
        .execute(&mut *blocker)
        .await
        .expect("hold final write");
    let p = pool.clone();
    let t = time.clone();
    let task = tokio::spawn(async move {
        admit(
            &p,
            CredentialPurpose::Login,
            "second@example.com",
            &key(1, 7),
            limits(3, 2),
            t.as_ref(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted)",
            )
            .fetch_one(&pool)
            .await
            .expect("observe final write wait");
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual final write wait");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    time.0.store(1020, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert!(matches!(
        task.await.expect("join"),
        Err(AdmissionError::Unavailable)
    ));
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
    admit(
        &pool,
        CredentialPurpose::Login,
        "second@example.com",
        &key(1, 7),
        limits(3, 2),
        time.as_ref(),
    )
    .await
    .expect("fresh retry");
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
}

struct ScriptedClock(std::sync::Mutex<std::collections::VecDeque<u64>>);
impl Clock for ScriptedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        let mut samples = self.0.lock().map_err(|_| ContextError::ClockUnavailable)?;
        if samples.len() > 1 {
            samples.pop_front().ok_or(ContextError::ClockUnavailable)
        } else {
            samples
                .front()
                .copied()
                .ok_or(ContextError::ClockUnavailable)
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn successful_final_observation_is_persisted_and_fences_clock_rewind(pool: PgPool) {
    let samples = ScriptedClock(std::sync::Mutex::new([1000, 1000, 1019].into()));
    admit(
        &pool,
        CredentialPurpose::Login,
        "first@example.com",
        &key(1, 7),
        limits(3, 2),
        &samples,
    )
    .await
    .expect("advanced clock admission");
    let last: i64 = sqlx::query_scalar(
        "SELECT last_seen_at FROM customer_credential_admission_scopes WHERE purpose=1",
    )
    .fetch_one(&pool)
    .await
    .expect("persisted last observation");
    assert_eq!(last, 1019);
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "second@example.com",
            &key(1, 7),
            limits(3, 2),
            &clock(1001)
        )
        .await,
        Err(AdmissionError::Unavailable)
    ));
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn partially_null_scope_is_rejected_by_database(pool: PgPool) {
    admit(
        &pool,
        CredentialPurpose::Login,
        "valid@example.com",
        &key(1, 7),
        limits(3, 2),
        &clock(1000),
    )
    .await
    .expect("valid control");
    assert!(
        sqlx::query(
            "UPDATE customer_credential_admission_scopes SET last_seen_at=NULL WHERE purpose=1"
        )
        .execute(&pool)
        .await
        .is_err(),
        "partial scope must not satisfy the constraint"
    );
    let last: i64 = sqlx::query_scalar(
        "SELECT last_seen_at FROM customer_credential_admission_scopes WHERE purpose=1",
    )
    .fetch_one(&pool)
    .await
    .expect("valid scope preserved");
    assert_eq!(last, 1000);
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_after_admission_does_not_restore_capacity(pool: PgPool) {
    let p = pool.clone();
    let (admitted, observed) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        admit(
            &p,
            CredentialPurpose::Login,
            "cancel@example.com",
            &key(1, 7),
            limits(1, 1),
            &clock(1000),
        )
        .await
        .expect("committed admission");
        admitted.send(()).expect("test receives admission");
        std::future::pending::<()>().await;
    });
    observed
        .await
        .expect("admission committed before cancellation");
    task.abort();
    assert!(task.await.expect_err("cancelled").is_cancelled());
    assert!(matches!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "cancel@example.com",
            &key(1, 7),
            limits(1, 1),
            &clock(1000)
        )
        .await,
        Err(AdmissionError::Limited)
    ));
    assert_eq!(count(&pool, 1).await, (1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stalled_commit_crossing_window_denies_but_retains_debit(pool: PgPool) {
    sqlx::query("CREATE FUNCTION hold_admission_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(274911); RETURN NEW; END $$")
        .execute(&pool).await.expect("deferred fence function");
    sqlx::query("CREATE CONSTRAINT TRIGGER hold_admission_commit AFTER UPDATE ON customer_credential_admission_scopes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hold_admission_commit()")
        .execute(&pool).await.expect("deferred commit fence");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(274911)")
        .execute(&mut *blocker)
        .await
        .expect("hold commit");
    let time = Arc::new(clock(1019));
    let p = pool.clone();
    let t = time.clone();
    let task = tokio::spawn(async move {
        admit(
            &p,
            CredentialPurpose::Login,
            "commit@example.com",
            &key(1, 7),
            limits(1, 1),
            t.as_ref(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted)",
            )
            .fetch_one(&pool)
            .await
            .expect("observe deferred commit wait");
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual commit wait");
    assert!(!task.is_finished());
    assert_eq!(count(&pool, 1).await, (0, 0, 0));
    time.0.store(1020, Ordering::SeqCst);
    blocker.commit().await.expect("release commit");
    assert!(matches!(
        task.await.expect("join"),
        Err(AdmissionError::Unavailable)
    ));
    assert_eq!(count(&pool, 1).await, (1, 1, 1), "committed debit retained");
    let stored: (i64, i64) = sqlx::query_as("SELECT window_start,last_seen_at FROM customer_credential_admission_scopes WHERE purpose=1")
        .fetch_one(&pool).await.expect("committed window");
    assert_eq!(stored, (960, 1019));
}

// Each sample describes one observation by the command; None simulates clock failure.
struct FailingClock(std::sync::Mutex<std::collections::VecDeque<Option<u64>>>);
impl Clock for FailingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        let mut samples = self.0.lock().map_err(|_| ContextError::ClockUnavailable)?;
        let sample = if samples.len() > 1 {
            samples.pop_front()
        } else {
            samples.front().copied()
        };
        sample.flatten().ok_or(ContextError::ClockUnavailable)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_observations_fail_closed_and_only_post_commit_failures_keep_the_debit(pool: PgPool) {
    // started, locked, written, optional high-water recheck, committed
    type ClockCase<'a> = (&'a str, &'a [Option<u64>], (i32, i64, i64), bool);
    let cases: &[ClockCase<'_>] = &[
        ("invalid initial time", &[Some(u64::MAX)], (0, 0, 0), false),
        (
            "rewind after lock",
            &[Some(1001), Some(1000)],
            (0, 0, 0),
            false,
        ),
        (
            "rewind after write",
            &[Some(1000), Some(1000), Some(999)],
            (0, 0, 0),
            false,
        ),
        (
            "window after write",
            &[Some(1000), Some(1000), Some(1020)],
            (0, 0, 0),
            false,
        ),
        (
            "rewind after high water",
            &[Some(1000), Some(1000), Some(1001), Some(1000)],
            (0, 0, 0),
            false,
        ),
        (
            "window after high water",
            &[Some(1000), Some(1000), Some(1001), Some(1020)],
            (0, 0, 0),
            false,
        ),
        (
            "rewind after commit",
            &[Some(1000), Some(1000), Some(1001), Some(1002), Some(1001)],
            (1, 1, 1),
            false,
        ),
        (
            "window after commit",
            &[Some(1000), Some(1000), Some(1000), Some(1020)],
            (1, 1, 1),
            false,
        ),
        (
            "clock unavailable after commit",
            &[Some(1000), Some(1000), Some(1000), None],
            (1, 1, 1),
            false,
        ),
        (
            "advancing time succeeds",
            &[Some(1000), Some(1000), Some(1001), Some(1002), Some(1003)],
            (1, 1, 1),
            true,
        ),
    ];
    for (label, samples, expected, succeeds) in cases {
        sqlx::query("DELETE FROM customer_credential_admission_targets WHERE purpose=1")
            .execute(&pool)
            .await
            .expect("reset targets");
        sqlx::query("UPDATE customer_credential_admission_scopes SET window_start=NULL,last_seen_at=NULL,key_version=NULL,key_check=NULL,global_limit=NULL,target_limit=NULL,admitted=0 WHERE purpose=1")
            .execute(&pool).await.expect("reset scope");
        let clock = FailingClock(std::sync::Mutex::new(samples.iter().copied().collect()));
        let outcome = admit(
            &pool,
            CredentialPurpose::Login,
            "clock@example.com",
            &key(1, 7),
            limits(2, 1),
            &clock,
        )
        .await;
        if *succeeds {
            assert_eq!(outcome, Ok(()), "{label}");
        } else {
            assert_eq!(outcome, Err(AdmissionError::Unavailable), "{label}");
        }
        assert_eq!(count(&pool, 1).await, *expected, "{label}");
        if expected.0 == 1 {
            assert_eq!(
                admit(
                    &pool,
                    CredentialPurpose::Login,
                    "clock@example.com",
                    &key(1, 7),
                    limits(2, 1),
                    &clock_for_retry()
                )
                .await,
                Err(AdmissionError::Limited),
                "{label}: committed debit must constrain retry"
            );
        }
    }
}

fn clock_for_retry() -> TestClock {
    clock(1010)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_persisted_target_counts_are_denied_without_changes(pool: PgPool) {
    let k = key(1, 7);
    let cap = limits(10, 3);
    let time = clock(1000);
    admit(
        &pool,
        CredentialPurpose::Login,
        "target@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("healthy target");
    admit(
        &pool,
        CredentialPurpose::Login,
        "other@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("healthy independent target");
    // Allow the test to model imported or otherwise damaged data; the normal
    // constraint is separately covered by the positive/negative SQL controls.
    sqlx::query("ALTER TABLE customer_credential_admission_targets DROP CONSTRAINT customer_credential_admission_targets_admitted_check")
        .execute(&pool).await.expect("permit damaged fixture");
    // Identify the actual target deterministically from its known healthy increment.
    admit(
        &pool,
        CredentialPurpose::Login,
        "target@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("healthy increment");
    let digest: Vec<u8> = sqlx::query_scalar("SELECT target_digest FROM customer_credential_admission_targets WHERE purpose=1 AND admitted=2")
        .fetch_one(&pool).await.expect("target digest");
    for broken in [0, 4] {
        sqlx::query("UPDATE customer_credential_admission_targets SET admitted=$1 WHERE purpose=1 AND target_digest=$2")
            .bind(broken).bind(&digest).execute(&pool).await.expect("damage target");
        let before = count(&pool, 1).await;
        assert_eq!(
            admit(
                &pool,
                CredentialPurpose::Login,
                "target@example.com",
                &k,
                cap,
                &time
            )
            .await,
            Err(AdmissionError::Unavailable),
            "damaged target {broken}"
        );
        assert_eq!(
            count(&pool, 1).await,
            before,
            "damaged target is not changed"
        );
    }
    // A count within the target limit can still exceed the scope total.
    sqlx::query("UPDATE customer_credential_admission_targets SET admitted=2 WHERE purpose=1 AND target_digest=$1")
        .bind(&digest).execute(&pool).await.expect("restore target");
    sqlx::query("UPDATE customer_credential_admission_scopes SET admitted=1 WHERE purpose=1")
        .execute(&pool)
        .await
        .expect("damage total");
    let before = count(&pool, 1).await;
    assert_eq!(
        admit(
            &pool,
            CredentialPurpose::Login,
            "target@example.com",
            &k,
            cap,
            &time
        )
        .await,
        Err(AdmissionError::Unavailable)
    );
    assert_eq!(count(&pool, 1).await, before);
    sqlx::query("UPDATE customer_credential_admission_scopes SET admitted=3 WHERE purpose=1")
        .execute(&pool)
        .await
        .expect("restore total");
    admit(
        &pool,
        CredentialPurpose::Login,
        "target@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("repaired target can be admitted");
    assert_eq!(count(&pool, 1).await, (4, 2, 4));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn terminal_storage_errors_roll_back_both_scope_and_target_writes(pool: PgPool) {
    let k = key(1, 7);
    let cap = limits(3, 2);
    let time = clock(1000);
    admit(
        &pool,
        CredentialPurpose::Login,
        "first@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("healthy initial admission");
    sqlx::query("CREATE FUNCTION fail_admission_test() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test failure'; END $$")
        .execute(&pool).await.expect("failure injector");
    for (create, drop_trigger) in [
        (
            "CREATE TRIGGER fail_admission_test BEFORE INSERT OR UPDATE ON customer_credential_admission_targets FOR EACH ROW EXECUTE FUNCTION fail_admission_test()",
            "DROP TRIGGER fail_admission_test ON customer_credential_admission_targets",
        ),
        (
            "CREATE TRIGGER fail_admission_test BEFORE UPDATE ON customer_credential_admission_scopes FOR EACH ROW EXECUTE FUNCTION fail_admission_test()",
            "DROP TRIGGER fail_admission_test ON customer_credential_admission_scopes",
        ),
        (
            "CREATE CONSTRAINT TRIGGER fail_admission_test AFTER UPDATE ON customer_credential_admission_scopes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_admission_test()",
            "DROP TRIGGER fail_admission_test ON customer_credential_admission_scopes",
        ),
    ] {
        sqlx::query(create)
            .execute(&pool)
            .await
            .expect("install failure injector");
        assert_eq!(
            admit(
                &pool,
                CredentialPurpose::Login,
                "second@example.com",
                &k,
                cap,
                &time
            )
            .await,
            Err(AdmissionError::Unavailable)
        );
        assert_eq!(
            count(&pool, 1).await,
            (1, 1, 1),
            "failed transaction leaves original claim only"
        );
        sqlx::query(drop_trigger)
            .execute(&pool)
            .await
            .expect("remove failure injector");
    }
    admit(
        &pool,
        CredentialPurpose::Login,
        "second@example.com",
        &k,
        cap,
        &time,
    )
    .await
    .expect("admission succeeds after storage recovers");
    assert_eq!(count(&pool, 1).await, (2, 2, 2));
}
