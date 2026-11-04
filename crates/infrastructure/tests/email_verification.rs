use commerce_application::request_context::{Clock, ContextError};
use commerce_domain::email_identity::normalize_email;
use commerce_infrastructure::{
    MIGRATOR,
    email_verification::{VerificationError, VerificationToken, consume, issue},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn clock(at: u64) -> TestClock {
    TestClock(AtomicU64::new(at))
}

#[test]
fn exact_mailbox_policy_has_explicit_positive_and_negative_cases() {
    assert_eq!(
        normalize_email("Case.Name+tag@EXAMPLE.COM").as_deref(),
        Some("Case.Name+tag@example.com")
    );
    assert_ne!(
        normalize_email("Case@example.com"),
        normalize_email("case@example.com")
    );
    assert_ne!(
        normalize_email("a.b@example.com"),
        normalize_email("ab@example.com")
    );
    assert_ne!(
        normalize_email("a+tag@example.com"),
        normalize_email("a@example.com")
    );
    for invalid in [
        "",
        "x@localhost",
        "x@@example.com",
        "x..y@example.com",
        ".x@example.com",
        "x.@example.com",
        "x@-example.com",
        "x@example..com",
        "x@example.c",
        "x@example.123",
        "x@[127.0.0.1]",
        "é@example.com",
        "x@éxample.com",
        " x@example.com",
        "\"x\"@example.com",
    ] {
        assert!(
            normalize_email(invalid).is_none(),
            "invalid identity: {invalid}"
        );
    }
    assert!(normalize_email(&format!("{}@example.com", "a".repeat(65))).is_none());

    let maximal = format!(
        "{}@{}.{}.{}",
        "a".repeat(64),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    assert_eq!(maximal.len(), 254);
    assert_eq!(normalize_email(&maximal).as_deref(), Some(maximal.as_str()));
    assert!(normalize_email(&format!("{maximal}d")).is_none());
    assert!(normalize_email(&format!("a@{}.com", "a".repeat(64))).is_none());
    for invalid in ["@example.com", "a@", "a@example-.com", "a@exam_ple.com"] {
        assert!(
            normalize_email(invalid).is_none(),
            "invalid identity: {invalid}"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issued_verifier_is_domain_separated_and_never_stores_the_bearer(pool: PgPool) {
    let clock = clock(1000);
    let token = issue(&pool, "Case.Name+tag@EXAMPLE.COM", &clock)
        .await
        .expect("issue");
    let (stored, purpose, email, created, expiry): (Vec<u8>,String,String,i64,i64) = sqlx::query_as("SELECT verifier,purpose,email_identity,created_at,expires_at FROM email_verification_challenges")
        .fetch_one(&pool).await.expect("custody");
    let expected = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(token.expose_bytes())
        .finalize()
        .to_vec();
    assert_eq!(stored, expected);
    assert_ne!(stored, token.expose_bytes());
    assert_eq!(
        (purpose.as_str(), email.as_str(), created, expiry),
        (
            "email-verification-v1",
            "Case.Name+tag@example.com",
            1000,
            1900
        )
    );
    let names: Vec<String> = sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_name='email_verification_challenges' ORDER BY ordinal_position")
        .fetch_all(&pool).await.expect("columns");
    assert_eq!(
        names,
        [
            "verifier",
            "purpose",
            "email_identity",
            "created_at",
            "expires_at",
            "consumed_at",
            "consumed_xid"
        ]
    );
    assert!(
        sqlx::query("UPDATE email_verification_challenges SET email_identity='other@example.com'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        consume(&pool, "Case.Name+tag@example.com", &token, &clock)
            .await
            .is_ok()
    );
    assert_eq!(
        consume(&pool, "Case.Name+tag@example.com", &token, &clock).await,
        Err(VerificationError::Denied)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn wrong_identity_token_and_expiry_have_the_same_denial(pool: PgPool) {
    let clock = clock(1000);
    let token = issue(&pool, "A+b@example.com", &clock)
        .await
        .expect("issue");
    let wrong = VerificationToken::from_bytes([42; 32]);
    for (identity, bearer) in [
        ("A+b@example.com", &wrong),
        ("a+b@example.com", &token),
        ("A@example.com", &token),
        ("not an address", &token),
    ] {
        assert_eq!(
            consume(&pool, identity, bearer, &clock).await,
            Err(VerificationError::Denied)
        );
    }
    clock.0.store(1900, Ordering::SeqCst);
    assert_eq!(
        consume(&pool, "A+b@example.com", &token, &clock).await,
        Err(VerificationError::Denied)
    );
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(consumed.is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_consumption_has_exactly_one_winner(pool: PgPool) {
    let clock = clock(1100);
    let token = issue(&pool, "a@example.com", &clock).await.expect("issue");
    let (a, b) = tokio::join!(
        consume(&pool, "a@example.com", &token, &clock),
        consume(&pool, "a@example.com", &token, &clock)
    );
    assert!(matches!(
        (a, b),
        (Ok(()), Err(VerificationError::Denied)) | (Err(VerificationError::Denied), Ok(()))
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_verification_challenges WHERE consumed_at=1100",
    )
    .fetch_one(&pool)
    .await
    .expect("row");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_is_checked_after_lock_wait(pool: PgPool) {
    let clock = Arc::new(clock(1000));
    let token = issue(&pool, "a@example.com", clock.as_ref())
        .await
        .expect("issue");
    let mut blocker = pool.begin().await.expect("lock transaction");
    sqlx::query("SELECT verifier FROM email_verification_challenges FOR UPDATE")
        .fetch_one(&mut *blocker)
        .await
        .expect("row lock");
    let other_pool = pool.clone();
    let other_clock = clock.clone();
    let attempt = tokio::spawn(async move {
        consume(&other_pool, "a@example.com", &token, other_clock.as_ref()).await
    });
    let mut waiting = false;
    for _ in 0..50 {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='transactionid' AND NOT granted AND pid<>pg_backend_pid()")
            .fetch_one(&pool).await.expect("lock observation");
        if count > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        waiting && !attempt.is_finished(),
        "attempt must wait for the held row"
    );
    clock.0.store(1900, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert_eq!(attempt.await.expect("task"), Err(VerificationError::Denied));
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(consumed.is_none());
}

struct SequenceClock {
    values: std::sync::Mutex<std::collections::VecDeque<u64>>,
}
impl Clock for SequenceClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.values
            .lock()
            .map_err(|_| ContextError::ClockUnavailable)?
            .pop_front()
            .ok_or(ContextError::ClockUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_update_rolls_back_and_last_valid_second_can_succeed(pool: PgPool) {
    let initial = clock(1000);
    let first = issue(&pool, "first@example.com", &initial)
        .await
        .expect("issue");
    let sequence = SequenceClock {
        values: std::sync::Mutex::new([1899, 1899, 1900].into()),
    };
    assert_eq!(
        consume(&pool, "first@example.com", &first, &sequence).await,
        Err(VerificationError::Denied)
    );
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(consumed.is_none());
    let second = issue(&pool, "second@example.com", &initial)
        .await
        .expect("second issue");
    let last_second = clock(1899);
    assert_eq!(
        consume(&pool, "second@example.com", &second, &last_second).await,
        Ok(())
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_failure_or_reversal_never_creates_or_consumes_a_challenge(pool: PgPool) {
    let reversed = SequenceClock {
        values: std::sync::Mutex::new([1000, 999].into()),
    };
    assert!(matches!(
        issue(&pool, "a@example.com", &reversed).await,
        Err(VerificationError::Unavailable)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_challenges")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
    let token = issue(&pool, "a@example.com", &clock(1000))
        .await
        .expect("issue");
    let failed = SequenceClock {
        values: std::sync::Mutex::new([1001].into()),
    };
    assert_eq!(
        consume(&pool, "a@example.com", &token, &failed).await,
        Err(VerificationError::Unavailable)
    );
    let reversed = SequenceClock {
        values: std::sync::Mutex::new([1001, 1000].into()),
    };
    assert_eq!(
        consume(&pool, "a@example.com", &token, &reversed).await,
        Err(VerificationError::Unavailable)
    );
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(consumed.is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_boundaries_fail_closed_and_leave_valid_proof_usable(pool: PgPool) {
    for at in [i64::MAX as u64, u64::MAX] {
        assert!(matches!(
            issue(&pool, "a@example.com", &clock(at)).await,
            Err(VerificationError::Unavailable)
        ));
    }
    let expired_before_commit = SequenceClock {
        values: std::sync::Mutex::new([1000, 1900].into()),
    };
    assert!(matches!(
        issue(&pool, "a@example.com", &expired_before_commit).await,
        Err(VerificationError::Unavailable)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_challenges")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);

    let token = issue(&pool, "a@example.com", &clock(1000))
        .await
        .expect("issue");
    assert_eq!(
        consume(&pool, "a@example.com", &token, &clock(999)).await,
        Err(VerificationError::Denied)
    );
    let reversed_after_update = SequenceClock {
        values: std::sync::Mutex::new([1000, 1001, 1000].into()),
    };
    assert_eq!(
        consume(&pool, "a@example.com", &token, &reversed_after_update).await,
        Err(VerificationError::Unavailable)
    );
    assert_eq!(
        consume(&pool, "a@example.com", &token, &clock(1001)).await,
        Ok(())
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_commit_grants_no_bearer_and_preserves_unspent_proof(pool: PgPool) {
    let time = clock(1000);
    let token = issue(&pool, "owner@example.com", &time)
        .await
        .expect("healthy issue");
    sqlx::query(
        "CREATE FUNCTION reject_verification_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'isolated commit fault'; END $$",
    )
    .execute(&pool)
    .await
    .expect("fault function");
    sqlx::query(
        "CREATE CONSTRAINT TRIGGER reject_verification_commit
         AFTER INSERT OR UPDATE ON email_verification_challenges
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_verification_commit()",
    )
    .execute(&pool)
    .await
    .expect("deferred commit fault");
    assert!(matches!(
        issue(&pool, "other@example.com", &time).await,
        Err(VerificationError::Unavailable)
    ));
    assert_eq!(
        consume(&pool, "owner@example.com", &token, &time).await,
        Err(VerificationError::Unavailable)
    );
    let counts: (i64, i64) =
        sqlx::query_as("SELECT count(*), count(consumed_at) FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("rollback counts");
    assert_eq!(counts, (1, 0));
    sqlx::query("DROP TRIGGER reject_verification_commit ON email_verification_challenges")
        .execute(&pool)
        .await
        .expect("remove fault");
    assert_eq!(
        consume(&pool, "owner@example.com", &token, &time).await,
        Ok(())
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_writes_fail_closed_and_healthy_operations_recover(pool: PgPool) {
    sqlx::query("CREATE FUNCTION suppress_verification_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("fault function");
    sqlx::query("CREATE TRIGGER suppress_verification_insert BEFORE INSERT ON email_verification_challenges FOR EACH ROW EXECUTE FUNCTION suppress_verification_write()")
        .execute(&pool).await.expect("insert fault");
    let clock = clock(1000);
    assert!(matches!(
        issue(&pool, "a@example.com", &clock).await,
        Err(VerificationError::Unavailable)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_challenges")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
    sqlx::query("DROP TRIGGER suppress_verification_insert ON email_verification_challenges")
        .execute(&pool)
        .await
        .expect("remove insert fault");
    let token = issue(&pool, "a@example.com", &clock)
        .await
        .expect("healthy issue");
    sqlx::query("CREATE TRIGGER suppress_verification_update BEFORE UPDATE ON email_verification_challenges FOR EACH ROW EXECUTE FUNCTION suppress_verification_write()")
        .execute(&pool).await.expect("update fault");
    assert_eq!(
        consume(&pool, "a@example.com", &token, &clock).await,
        Err(VerificationError::Unavailable)
    );
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(consumed.is_none());
    sqlx::query("DROP TRIGGER suppress_verification_update ON email_verification_challenges")
        .execute(&pool)
        .await
        .expect("remove update fault");
    assert_eq!(
        consume(&pool, "a@example.com", &token, &clock).await,
        Ok(())
    );
    assert_eq!(
        consume(&pool, "a@example.com", &token, &clock).await,
        Err(VerificationError::Denied)
    );
}
