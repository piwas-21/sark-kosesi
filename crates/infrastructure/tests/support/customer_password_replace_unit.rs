use super::*;
use crate::{MIGRATOR, customer_passwords, email_verification, verified_customer_accounts};
use commerce_application::request_context::ContextError;

fn proof() -> Proof {
    Proof {
        account_id: 7,
        expected_revision: 2,
        created_at: 1_000,
        expires_at: 1_900,
        consumed_at: None,
        xid_empty: true,
    }
}

fn stored() -> Stored {
    Stored {
        revision: 3,
        password_hash: "hash".into(),
        changed_at: 1_100,
        challenge_verifier: vec![1, 2, 3],
        credential_xid_matches: true,
        account_id: 7,
        expected_revision: 2,
        created_at: 1_000,
        expires_at: 1_900,
        consumed_at: Some(1_100),
        proof_xid_matches: true,
    }
}

#[test]
fn proof_boundaries_and_each_source_field_are_checked() {
    let original = proof();
    assert!(original.valid(1_000));
    assert!(original.valid(1_899));
    assert!(!original.valid(999));
    assert!(!original.valid(1_900));
    let invalid: [fn(&mut Proof); 6] = [
        |p| p.expected_revision = -1,
        |p| p.created_at = -1,
        |p| p.expires_at += 1,
        |p| {
            p.created_at = i64::MAX;
            p.expires_at = i64::MAX;
        },
        |p| p.consumed_at = Some(1_000),
        |p| p.xid_empty = false,
    ];
    for change in invalid {
        let mut altered = proof();
        change(&mut altered);
        assert!(!altered.valid(1_100));
    }
    let changed_source: [fn(&mut Proof); 4] = [
        |p| p.account_id += 1,
        |p| p.expected_revision += 1,
        |p| p.created_at += 1,
        |p| p.expires_at += 1,
    ];
    assert!(original.same_source(&proof()));
    for change in changed_source {
        let mut altered = proof();
        change(&mut altered);
        assert!(!original.same_source(&altered));
    }
}

#[test]
fn final_tuple_requires_each_exact_proof_and_credential_field() {
    let valid = |row: &Stored| row.matches(&proof(), 3, "hash", &[1, 2, 3], 1_100);
    assert!(valid(&stored()));
    let corruptions: [fn(&mut Stored); 11] = [
        |s| s.revision += 1,
        |s| s.password_hash.push('x'),
        |s| s.changed_at += 1,
        |s| s.challenge_verifier.push(4),
        |s| s.credential_xid_matches = false,
        |s| s.account_id += 1,
        |s| s.expected_revision += 1,
        |s| s.created_at += 1,
        |s| s.expires_at += 1,
        |s| s.consumed_at = None,
        |s| s.proof_xid_matches = false,
    ];
    for corrupt in corruptions {
        let mut row = stored();
        corrupt(&mut row);
        assert!(!valid(&row));
    }
}

struct TestClock;
impl Clock for TestClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(1_000)
    }
}

async fn setup_account(pool: &PgPool) -> (i64, PasswordChangeToken) {
    let email = "replacement-control@example.com";
    let token = email_verification::issue(pool, email, &TestClock)
        .await
        .expect("verification");
    let id = verified_customer_accounts::establish(pool, email, &token, &TestClock)
        .await
        .expect("account")
        .value();
    let token = customer_passwords::issue(pool, email, &TestClock)
        .await
        .expect("challenge");
    (id, token)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_proof_consumption_leaves_it_usable(pool: PgPool) {
    let (id, token) = setup_account(&pool).await;
    sqlx::query("CREATE FUNCTION suppress_password_consumption() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("fault function");
    sqlx::query("CREATE TRIGGER suppress_password_consumption BEFORE UPDATE ON customer_password_challenges FOR EACH ROW EXECUTE FUNCTION suppress_password_consumption()")
        .execute(&pool).await.expect("fault trigger");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &token,
            "long enough password",
            &TestClock
        )
        .await,
        Err(PasswordError::Unavailable)
    );
    let state: (i64, Option<i64>) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_password_credentials WHERE account_id=$1), consumed_at FROM customer_password_challenges WHERE verifier=$2")
        .bind(id).bind(token.verifier().as_slice()).fetch_one(&pool).await.expect("unchanged state");
    assert_eq!(state, (0, None));
    sqlx::query("DROP TRIGGER suppress_password_consumption ON customer_password_challenges")
        .execute(&pool)
        .await
        .expect("remove fault");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &token,
            "long enough password",
            &TestClock
        )
        .await,
        Ok(1)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_reset_rolls_back_consumption_and_preserves_exact_old_credential(pool: PgPool) {
    let (id, first) = setup_account(&pool).await;
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &first,
            "first password long",
            &TestClock
        )
        .await,
        Ok(1)
    );
    let reset = customer_passwords::issue(&pool, "replacement-control@example.com", &TestClock)
        .await
        .expect("reset proof");
    let before: (i32, String, i64, Vec<u8>) = sqlx::query_as("SELECT revision,password_hash,changed_at,challenge_verifier FROM customer_password_credentials WHERE account_id=$1")
        .bind(id).fetch_one(&pool).await.expect("old credential");
    sqlx::query("CREATE FUNCTION suppress_password_reset() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("fault function");
    sqlx::query("CREATE TRIGGER suppress_password_reset BEFORE UPDATE ON customer_password_credentials FOR EACH ROW EXECUTE FUNCTION suppress_password_reset()")
        .execute(&pool).await.expect("fault trigger");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &reset,
            "second password long",
            &TestClock
        )
        .await,
        Err(PasswordError::Unavailable)
    );
    let after: (i32, String, i64, Vec<u8>) = sqlx::query_as("SELECT revision,password_hash,changed_at,challenge_verifier FROM customer_password_credentials WHERE account_id=$1")
        .bind(id).fetch_one(&pool).await.expect("preserved credential");
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_at FROM customer_password_challenges WHERE verifier=$1",
    )
    .bind(reset.verifier().as_slice())
    .fetch_one(&pool)
    .await
    .expect("proof state");
    assert_eq!(after, before);
    assert_eq!(consumed, None);
    sqlx::query("DROP TRIGGER suppress_password_reset ON customer_password_credentials")
        .execute(&pool)
        .await
        .expect("remove fault");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &reset,
            "second password long",
            &TestClock
        )
        .await,
        Ok(2)
    );
}

async fn wait_for_account_lock(pool: &PgPool) {
    for _ in 0..80 {
        let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query='SELECT id FROM verified_customer_accounts WHERE id=$1 AND email_identity=$2 FOR UPDATE')")
            .fetch_one(pool).await.expect("lock status");
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("replacement did not reach the account lock");
}

const CANONICAL_TEST_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_distinct_proof_cannot_replace_a_newer_revision_after_hashing(pool: PgPool) {
    let (id, losing) = setup_account(&pool).await;
    let winning = customer_passwords::issue(&pool, "replacement-control@example.com", &TestClock)
        .await
        .expect("competing proof");
    let mut blocker = pool.begin().await.expect("transaction");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .expect("hold account");
    let worker_pool = pool.clone();
    let worker = tokio::spawn(async move {
        customer_passwords::replace(
            &worker_pool,
            "replacement-control@example.com",
            &losing,
            "losing password long",
            &TestClock,
        )
        .await
    });
    wait_for_account_lock(&pool).await;
    sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(winning.verifier().as_slice())
        .execute(&mut *blocker)
        .await
        .expect("consume winning proof");
    sqlx::query("INSERT INTO customer_password_credentials(account_id,revision,password_hash,changed_at,challenge_verifier) VALUES ($1,1,$2,1000,$3)")
        .bind(id).bind(CANONICAL_TEST_HASH).bind(winning.verifier().as_slice()).execute(&mut *blocker).await.expect("commit winning credential");
    blocker.commit().await.expect("release account");
    assert_eq!(worker.await.expect("worker"), Err(PasswordError::Denied));
    let state: (i32, String, Vec<u8>, i64) = sqlx::query_as("SELECT revision,password_hash,challenge_verifier,(SELECT count(*) FROM customer_password_challenges WHERE account_id=$1 AND consumed_at IS NOT NULL) FROM customer_password_credentials WHERE account_id=$1")
        .bind(id).fetch_one(&pool).await.expect("winning state");
    assert_eq!(
        state,
        (
            1,
            CANONICAL_TEST_HASH.to_owned(),
            winning.verifier().to_vec(),
            1
        )
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn substituted_canonical_hash_fails_final_check_and_rolls_back(pool: PgPool) {
    let (id, token) = setup_account(&pool).await;
    // The trigger sorts after the native guard and changes only a valid PHC value.
    sqlx::query("CREATE FUNCTION substitute_password_hash() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.password_hash := '$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'; RETURN NEW; END $$")
        .execute(&pool).await.expect("substitution function");
    sqlx::query("CREATE TRIGGER zz_substitute_password_hash BEFORE INSERT ON customer_password_credentials FOR EACH ROW EXECUTE FUNCTION substitute_password_hash()")
        .execute(&pool).await.expect("substitution trigger");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &token,
            "long enough password",
            &TestClock
        )
        .await,
        Err(PasswordError::Unavailable)
    );
    let state: (i64, Option<i64>) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_password_credentials WHERE account_id=$1),consumed_at FROM customer_password_challenges WHERE verifier=$2")
        .bind(id).bind(token.verifier().as_slice()).fetch_one(&pool).await.expect("rolled back state");
    assert_eq!(state, (0, None));
    sqlx::query("DROP TRIGGER zz_substitute_password_hash ON customer_password_credentials")
        .execute(&pool)
        .await
        .expect("remove substitution");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "replacement-control@example.com",
            &token,
            "long enough password",
            &TestClock
        )
        .await,
        Ok(1)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn source_drift_after_precheck_is_denied_without_consuming_proof(pool: PgPool) {
    let (id, token) = setup_account(&pool).await;
    let digest = token.verifier();
    let mut blocker = pool.begin().await.expect("transaction");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .expect("hold account");
    let worker_pool = pool.clone();
    let worker = tokio::spawn(async move {
        customer_passwords::replace(
            &worker_pool,
            "replacement-control@example.com",
            &token,
            "long enough password",
            &TestClock,
        )
        .await
    });
    wait_for_account_lock(&pool).await;
    // Privileged fault injection simulates changed source despite the native immutability guard.
    sqlx::query("ALTER TABLE customer_password_challenges DISABLE TRIGGER guard_customer_password_challenge").execute(&mut *blocker).await.expect("inject source drift");
    sqlx::query(
        "UPDATE customer_password_challenges SET created_at=999,expires_at=1899 WHERE verifier=$1",
    )
    .bind(digest.as_slice())
    .execute(&mut *blocker)
    .await
    .expect("alter source");
    sqlx::query("SET CONSTRAINTS require_customer_password_change IMMEDIATE")
        .execute(&mut *blocker)
        .await
        .expect("flush native constraint events before restoring the guard");
    sqlx::query(
        "ALTER TABLE customer_password_challenges ENABLE TRIGGER guard_customer_password_challenge",
    )
    .execute(&mut *blocker)
    .await
    .expect("restore guard");
    blocker.commit().await.expect("release account");
    assert_eq!(worker.await.expect("worker"), Err(PasswordError::Denied));
    let state: (i64, i64, Option<i64>, i64) = sqlx::query_as("SELECT created_at,expires_at,consumed_at,(SELECT count(*) FROM customer_password_credentials WHERE account_id=$1) FROM customer_password_challenges WHERE verifier=$2")
        .bind(id).bind(digest.as_slice()).fetch_one(&pool).await.expect("unconsumed source");
    assert_eq!(state, (999, 1899, None, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn account_identity_drift_after_precheck_is_denied(pool: PgPool) {
    let (id, token) = setup_account(&pool).await;
    let digest = token.verifier();
    // Privileged fault injection simulates identity corruption despite native immutability.
    sqlx::query(
        "ALTER TABLE verified_customer_accounts DISABLE TRIGGER guard_verified_customer_account",
    )
    .execute(&pool)
    .await
    .expect("inject identity drift");
    let mut blocker = pool.begin().await.expect("transaction");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .expect("hold account");
    let worker_pool = pool.clone();
    let worker = tokio::spawn(async move {
        customer_passwords::replace(
            &worker_pool,
            "replacement-control@example.com",
            &token,
            "long enough password",
            &TestClock,
        )
        .await
    });
    wait_for_account_lock(&pool).await;
    sqlx::query("UPDATE verified_customer_accounts SET email_identity='changed-control@example.com' WHERE id=$1").bind(id).execute(&mut *blocker).await.expect("alter identity");
    blocker.commit().await.expect("release account");
    assert_eq!(worker.await.expect("worker"), Err(PasswordError::Denied));
    sqlx::query(
        "ALTER TABLE verified_customer_accounts ENABLE TRIGGER guard_verified_customer_account",
    )
    .execute(&pool)
    .await
    .expect("restore identity guard");
    let state: (Option<i64>, i64) = sqlx::query_as("SELECT consumed_at,(SELECT count(*) FROM customer_password_credentials WHERE account_id=$1) FROM customer_password_challenges WHERE verifier=$2")
        .bind(id).bind(digest.as_slice()).fetch_one(&pool).await.expect("untouched proof");
    assert_eq!(state, (None, 0));
}

struct ExpiresBeforeCommit(std::sync::atomic::AtomicUsize);
impl Clock for ExpiresBeforeCommit {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        let sample = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(if sample < 3 { 1000 } else { 1900 })
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replacement_requires_the_native_fence_and_fresh_time_at_commit(pool: PgPool) {
    let (id, token) = setup_account(&pool).await;
    let email = "replacement-control@example.com";
    let password = "long enough password";
    assert_eq!(
        customer_passwords::replace(
            &pool,
            email,
            &token,
            password,
            &ExpiresBeforeCommit(0.into()),
        )
        .await,
        Err(PasswordError::Denied)
    );
    assert_unspent(&pool, id, &token).await;

    sqlx::query("DROP TRIGGER require_customer_password_change ON customer_password_challenges")
        .execute(&pool)
        .await
        .expect("remove the native fence in this isolated database");
    assert_eq!(
        customer_passwords::replace(&pool, email, &token, password, &TestClock).await,
        Err(PasswordError::Unavailable)
    );
    assert_unspent(&pool, id, &token).await;

    sqlx::query(
        "CREATE CONSTRAINT TRIGGER require_customer_password_change
         AFTER UPDATE ON customer_password_challenges DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION require_customer_password_change()",
    )
    .execute(&pool)
    .await
    .expect("restore the native fence");
    assert_eq!(
        customer_passwords::replace(&pool, email, &token, password, &TestClock).await,
        Ok(1)
    );
}

async fn assert_unspent(pool: &PgPool, account: i64, token: &PasswordChangeToken) {
    let state: (i64, Option<i64>, bool) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM customer_password_credentials WHERE account_id=$1),
         consumed_at,consumed_xid IS NULL FROM customer_password_challenges WHERE verifier=$2",
    )
    .bind(account)
    .bind(token.verifier().as_slice())
    .fetch_one(pool)
    .await
    .expect("rolled back proof and credential");
    assert_eq!(state, (0, None, true));
}
