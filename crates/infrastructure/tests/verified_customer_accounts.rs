use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    email_verification::{VerificationError, VerificationToken, issue},
    verified_customer_accounts::establish,
};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};

struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn clock() -> TestClock {
    TestClock(AtomicU64::new(1000))
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_proof_creates_immutable_unique_account_and_denials_preserve_proof(pool: PgPool) {
    let time = clock();
    let token = issue(&pool, "Case+tag@EXAMPLE.COM", &time)
        .await
        .expect("issue");
    let wrong = VerificationToken::from_bytes([8; 32]);
    assert!(matches!(
        establish(&pool, "Case+tag@example.com", &wrong, &time).await,
        Err(VerificationError::Denied)
    ));
    assert!(matches!(
        establish(&pool, "case+tag@example.com", &token, &time).await,
        Err(VerificationError::Denied)
    ));
    let id = establish(&pool, "Case+tag@example.com", &token, &time)
        .await
        .expect("establish");
    let row: (i64, String, i64) =
        sqlx::query_as("SELECT id,email_identity,verified_at FROM verified_customer_accounts")
            .fetch_one(&pool)
            .await
            .expect("account");
    assert_eq!(row, (id.value(), "Case+tag@example.com".to_string(), 1000));
    assert!(
        sqlx::query("UPDATE verified_customer_accounts SET email_identity='new@example.com'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM verified_customer_accounts")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(matches!(
        establish(&pool, "Case+tag@example.com", &token, &time).await,
        Err(VerificationError::Denied)
    ));
    let duplicate = issue(&pool, "Case+tag@example.com", &time)
        .await
        .expect("duplicate issue");
    assert!(matches!(
        establish(&pool, "Case+tag@example.com", &duplicate, &time).await,
        Err(VerificationError::Denied)
    ));
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(remaining, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_challenges_for_same_mailbox_have_one_account_and_one_consumed_proof(
    pool: PgPool,
) {
    let time = clock();
    let a = issue(&pool, "a@example.com", &time).await.expect("issue");
    let b = issue(&pool, "a@example.com", &time).await.expect("issue");
    let (a, b) = tokio::join!(
        establish(&pool, "a@example.com", &a, &time),
        establish(&pool, "a@example.com", &b, &time)
    );
    assert!(matches!(
        (a, b),
        (Ok(_), Err(VerificationError::Denied)) | (Err(VerificationError::Denied), Ok(_))
    ));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)").fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (1, 1));
}

struct SequenceClock(std::sync::Mutex<std::collections::VecDeque<u64>>);
impl Clock for SequenceClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
            .lock()
            .map_err(|_| ContextError::ClockUnavailable)?
            .pop_front()
            .ok_or(ContextError::ClockUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_account_insert_and_suppressed_writes_roll_back_both_effects(pool: PgPool) {
    let time = clock();
    let token = issue(&pool, "a@example.com", &time).await.expect("issue");
    let expires = SequenceClock(std::sync::Mutex::new([1899, 1899, 1900].into()));
    assert!(matches!(
        establish(&pool, "a@example.com", &token, &expires).await,
        Err(VerificationError::Denied)
    ));
    sqlx::query("CREATE FUNCTION suppress_account_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").execute(&pool).await.expect("fault");
    sqlx::query("CREATE TRIGGER suppress_account_insert BEFORE INSERT ON verified_customer_accounts FOR EACH ROW EXECUTE FUNCTION suppress_account_write()").execute(&pool).await.expect("fault");
    assert!(matches!(
        establish(&pool, "a@example.com", &token, &time).await,
        Err(VerificationError::Unavailable)
    ));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)").fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (0, 0));
    sqlx::query("DROP TRIGGER suppress_account_insert ON verified_customer_accounts")
        .execute(&pool)
        .await
        .expect("remove fault");
    assert!(
        establish(&pool, "a@example.com", &token, &time)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_or_failed_account_insert_rolls_back_and_preserves_proof(pool: PgPool) {
    let time = clock();
    let token = issue(&pool, "a@example.com", &time).await.expect("issue");
    let other_token = issue(&pool, "other@example.com", &time)
        .await
        .expect("second valid proof");
    // PostgreSQL fires same-kind triggers alphabetically, so this runs after the
    // production guard and simulates an altered RETURNING row.
    sqlx::query("CREATE FUNCTION alter_account_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.verified_at := NEW.verified_at + 1; RETURN NEW; END $$")
        .execute(&pool).await.expect("fault function");
    sqlx::query("CREATE TRIGGER z_alter_account_insert BEFORE INSERT ON verified_customer_accounts FOR EACH ROW EXECUTE FUNCTION alter_account_insert()")
        .execute(&pool).await.expect("alter insert");
    assert!(matches!(
        establish(&pool, "a@example.com", &token, &time).await,
        Err(VerificationError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER z_alter_account_insert ON verified_customer_accounts")
        .execute(&pool)
        .await
        .expect("remove alteration");
    for function in [
        "CREATE OR REPLACE FUNCTION alter_account_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.email_identity := 'altered@example.com'; RETURN NEW; END $$",
        "CREATE OR REPLACE FUNCTION alter_account_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN SELECT verifier INTO NEW.verification_verifier FROM email_verification_challenges WHERE email_identity='other@example.com'; RETURN NEW; END $$",
    ] {
        sqlx::query(function)
            .execute(&pool)
            .await
            .expect("alter function");
        sqlx::query("CREATE TRIGGER z_alter_account_insert BEFORE INSERT ON verified_customer_accounts FOR EACH ROW EXECUTE FUNCTION alter_account_insert()")
            .execute(&pool).await.expect("alter insert");
        assert!(matches!(
            establish(&pool, "a@example.com", &token, &time).await,
            Err(VerificationError::Unavailable)
        ));
        sqlx::query("DROP TRIGGER z_alter_account_insert ON verified_customer_accounts")
            .execute(&pool)
            .await
            .expect("remove alteration");
    }
    sqlx::query("CREATE FUNCTION fail_account_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected failure'; END $$")
        .execute(&pool).await.expect("failure function");
    sqlx::query("CREATE TRIGGER z_fail_account_insert BEFORE INSERT ON verified_customer_accounts FOR EACH ROW EXECUTE FUNCTION fail_account_insert()")
        .execute(&pool).await.expect("fail insert");
    assert!(matches!(
        establish(&pool, "a@example.com", &token, &time).await,
        Err(VerificationError::Unavailable)
    ));
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)")
        .fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (0, 0));
    sqlx::query("DROP TRIGGER z_fail_account_insert ON verified_customer_accounts")
        .execute(&pool)
        .await
        .expect("remove failure");
    assert!(
        establish(&pool, "a@example.com", &token, &time)
            .await
            .is_ok()
    );
    assert!(
        establish(&pool, "other@example.com", &other_token, &time)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_same_proof_and_suppressed_consumption_cannot_create_detached_accounts(
    pool: PgPool,
) {
    let time = clock();
    let token = issue(&pool, "first@example.com", &time)
        .await
        .expect("issue");
    sqlx::query("CREATE FUNCTION suppress_challenge_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").execute(&pool).await.expect("fault");
    sqlx::query("CREATE TRIGGER suppress_challenge_update BEFORE UPDATE ON email_verification_challenges FOR EACH ROW EXECUTE FUNCTION suppress_challenge_update()").execute(&pool).await.expect("fault");
    assert!(matches!(
        establish(&pool, "first@example.com", &token, &time).await,
        Err(VerificationError::Unavailable)
    ));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)").fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (0, 0));
    sqlx::query("DROP TRIGGER suppress_challenge_update ON email_verification_challenges")
        .execute(&pool)
        .await
        .expect("remove fault");
    let (a, b) = tokio::join!(
        establish(&pool, "first@example.com", &token, &time),
        establish(&pool, "first@example.com", &token, &time)
    );
    assert!(matches!(
        (a, b),
        (Ok(_), Err(VerificationError::Denied)) | (Err(VerificationError::Denied), Ok(_))
    ));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)").fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn standalone_consumption_cannot_be_replayed_to_establish_account(pool: PgPool) {
    let time = clock();
    let token = issue(&pool, "a@example.com", &time).await.expect("issue");
    commerce_infrastructure::email_verification::consume(&pool, "a@example.com", &token, &time)
        .await
        .expect("consume");
    assert!(matches!(
        establish(&pool, "a@example.com", &token, &time).await,
        Err(VerificationError::Denied)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM verified_customer_accounts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn waiting_for_challenge_lock_does_not_extend_expiry(pool: PgPool) {
    use std::{sync::Arc, time::Duration};
    let time = Arc::new(clock());
    let token = issue(&pool, "a@example.com", time.as_ref())
        .await
        .expect("issue");
    let mut blocker = pool.begin().await.expect("transaction");
    sqlx::query("SELECT verifier FROM email_verification_challenges FOR UPDATE")
        .fetch_one(&mut *blocker)
        .await
        .expect("lock");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let task = tokio::spawn(async move {
        establish(&worker_pool, "a@example.com", &token, worker_time.as_ref()).await
    });
    let mut waiting = false;
    for _ in 0..50 {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='transactionid' AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("locks");
        if count > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(waiting && !task.is_finished());
    time.0.store(1900, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert!(matches!(
        task.await.expect("task"),
        Err(VerificationError::Denied)
    ));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)").fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn waiting_for_unique_email_does_not_extend_expiry(pool: PgPool) {
    use sha2::{Digest, Sha256};
    use std::{sync::Arc, time::Duration};
    let time = Arc::new(clock());
    let blocker_token = issue(&pool, "a@example.com", time.as_ref())
        .await
        .expect("first issue");
    let token = issue(&pool, "a@example.com", time.as_ref())
        .await
        .expect("second issue");
    let verifier = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(blocker_token.expose_bytes())
        .finalize()
        .to_vec();
    let mut blocker = pool.begin().await.expect("transaction");
    sqlx::query("UPDATE email_verification_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(&verifier)
        .execute(&mut *blocker)
        .await
        .expect("consume");
    sqlx::query("INSERT INTO verified_customer_accounts(email_identity,verification_verifier,verified_at) VALUES ('a@example.com',$1,1000)").bind(&verifier).execute(&mut *blocker).await.expect("block unique email");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let task = tokio::spawn(async move {
        establish(&worker_pool, "a@example.com", &token, worker_time.as_ref()).await
    });
    let mut waiting = false;
    for _ in 0..50 {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='transactionid' AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("locks");
        if count > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(waiting && !task.is_finished());
    time.0.store(1900, Ordering::SeqCst);
    blocker.rollback().await.expect("release without account");
    assert!(matches!(
        task.await.expect("task"),
        Err(VerificationError::Denied)
    ));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM verified_customer_accounts), (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL)").fetch_one(&pool).await.expect("counts");
    assert_eq!(counts, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn database_requires_native_same_transaction_consumption_and_rejects_prefilled_custody(
    pool: PgPool,
) {
    use sqlx::Executor;
    let time = clock();
    let standalone = issue(&pool, "standalone@example.com", &time)
        .await
        .expect("issue");
    commerce_infrastructure::email_verification::consume(
        &pool,
        "standalone@example.com",
        &standalone,
        &time,
    )
    .await
    .expect("consume and commit");
    assert!(pool.execute("INSERT INTO verified_customer_accounts(email_identity,verification_verifier,verified_at) SELECT email_identity,verifier,consumed_at FROM email_verification_challenges WHERE email_identity='standalone@example.com'").await.is_err(), "committed consumption is not adoptable");
    assert!(pool.execute("INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at,consumed_at,consumed_xid) VALUES (decode(repeat('ab',32),'hex'),'email-verification-v1','prefill@example.com',1000,1900,1000,pg_current_xact_id())").await.is_err(), "caller cannot prefill consumption");
    assert!(pool.execute("INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at,consumed_xid) VALUES (decode(repeat('ac',32),'hex'),'email-verification-v1','prefill@example.com',1000,1900,pg_current_xact_id())").await.is_err(), "caller cannot prefill xid alone");
    let _token = issue(&pool, "native@example.com", &time)
        .await
        .expect("issue");
    assert!(pool.execute("UPDATE email_verification_challenges SET consumed_at=1000,consumed_xid=pg_current_xact_id() WHERE email_identity='native@example.com'").await.is_err(), "caller cannot choose consumption xid");
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query("UPDATE email_verification_challenges SET consumed_at=1000 WHERE email_identity='native@example.com'").execute(&mut *tx).await.expect("native consumption");
    let inserted = sqlx::query("INSERT INTO verified_customer_accounts(email_identity,verification_verifier,verified_at) SELECT email_identity,verifier,consumed_at FROM email_verification_challenges WHERE email_identity='native@example.com'").execute(&mut *tx).await.expect("same transaction evidence");
    assert_eq!(inserted.rows_affected(), 1);
    tx.commit().await.expect("commit");
    assert!(pool.execute("UPDATE email_verification_challenges SET consumed_xid=pg_current_xact_id() WHERE email_identity='native@example.com'").await.is_err(), "consumed custody is immutable");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM verified_customer_accounts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = false)]
async fn upgrade_preserves_but_cannot_adopt_preexisting_consumption(pool: PgPool) {
    use sqlx::Executor;
    MIGRATOR.run_to(34, &pool).await.expect("challenge schema");
    let time = clock();
    let used = issue(&pool, "legacy@example.com", &time)
        .await
        .expect("legacy issue");
    commerce_infrastructure::email_verification::consume(&pool, "legacy@example.com", &used, &time)
        .await
        .expect("legacy committed consumption");
    let fresh = issue(&pool, "fresh@example.com", &time)
        .await
        .expect("unconsumed legacy challenge");
    MIGRATOR.run(&pool).await.expect("upgrade account schema");
    let legacy: (Option<i64>, bool) = sqlx::query_as("SELECT consumed_at, consumed_xid IS NULL FROM email_verification_challenges WHERE email_identity='legacy@example.com'").fetch_one(&pool).await.expect("preserved row");
    assert_eq!(legacy, (Some(1000), true));
    assert!(pool.execute("INSERT INTO verified_customer_accounts(email_identity,verification_verifier,verified_at) SELECT email_identity,verifier,consumed_at FROM email_verification_challenges WHERE email_identity='legacy@example.com'").await.is_err(), "legacy consumed proof cannot be adopted");
    assert!(
        establish(&pool, "fresh@example.com", &fresh, &time)
            .await
            .is_ok(),
        "unconsumed legacy proof remains usable"
    );
}
