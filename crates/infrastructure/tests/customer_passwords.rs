#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;

use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use argon2::{
    Argon2,
    password_hash::{PasswordVerifier, phc::PasswordHash},
};
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_passwords::{PasswordChangeToken, PasswordError},
};
use sha2::{Digest, Sha256};
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
async fn account(pool: &PgPool, email: &str, time: &TestClock) -> i64 {
    let token = email_verification::issue(pool, email, time)
        .await
        .expect("verification issue");
    establish(pool, email, &token, time)
        .await
        .expect("account")
        .value()
}
fn verifier(token: &PasswordChangeToken) -> Vec<u8> {
    Sha256::new()
        .chain_update(b"piwas:customer-password-change:verifier:v1\0")
        .chain_update(token.expose_bytes())
        .finalize()
        .to_vec()
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM customer_password_credentials), (SELECT count(*) FROM customer_password_challenges WHERE consumed_at IS NOT NULL)")
        .fetch_one(pool).await.expect("counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocked_precheck_releases_the_connection_without_spending_proof(pool: PgPool) {
    let time = clock();
    let email = "locked-precheck@example.com";
    account(&pool, email, &time).await;
    let proof = customer_passwords::issue(&pool, email, &time)
        .await
        .expect("issue before the blocker");
    let mut blocker = pool.begin().await.expect("blocker transaction");
    sqlx::query("LOCK TABLE customer_password_challenges IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("block the read-only precheck");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        customer_passwords::replace(&pool, email, &proof, "long enough password", &time),
    )
    .await
    .expect("the database enforces a tighter wait bound");
    assert_eq!(result, Err(PasswordError::Unavailable));
    blocker.rollback().await.expect("release the lock");
    assert_eq!(counts(&pool).await, (0, 0));
    assert_eq!(
        customer_passwords::replace(&pool, email, &proof, "long enough password", &time).await,
        Ok(1)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_proof_sets_and_resets_only_current_argon2_credential(pool: PgPool) {
    let time = clock();
    let id = account(&pool, "Case+tag@EXAMPLE.COM", &time).await;
    let token = customer_passwords::issue(&pool, "Case+tag@example.com", &time)
        .await
        .expect("issue");
    let stored: (Vec<u8>, i64, i32, i64, i64) = sqlx::query_as("SELECT verifier,account_id,expected_revision,created_at,expires_at FROM customer_password_challenges")
        .fetch_one(&pool).await.expect("challenge");
    assert_eq!(stored.0, verifier(&token));
    assert_ne!(stored.0, token.expose_bytes().to_vec());
    let registration_verifier = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(token.expose_bytes())
        .finalize()
        .to_vec();
    assert_ne!(stored.0, registration_verifier);
    assert_eq!((stored.1, stored.2, stored.3), (id, 0, 1000));
    assert!(stored.4 > 1000);
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "Case+tag@EXAMPLE.COM",
            &token,
            "first-password-long",
            &time
        )
        .await
        .expect("setup"),
        1
    );
    let (revision, hash, changed_at, challenge): (i32, String, i64, Vec<u8>) = sqlx::query_as("SELECT revision,password_hash,changed_at,challenge_verifier FROM customer_password_credentials WHERE account_id=$1")
        .bind(id).fetch_one(&pool).await.expect("credential");
    assert_eq!((revision, changed_at, challenge), (1, 1000, stored.0));
    assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    let parsed = PasswordHash::new(&hash).expect("valid PHC");
    assert!(
        Argon2::default()
            .verify_password(b"first-password-long", &parsed)
            .is_ok()
    );
    assert!(
        Argon2::default()
            .verify_password(b"wrong-password-long", &parsed)
            .is_err()
    );
    let reset = customer_passwords::issue(&pool, "Case+tag@EXAMPLE.com", &time)
        .await
        .expect("reset issue");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "Case+tag@example.com",
            &reset,
            "second-password-long",
            &time
        )
        .await
        .expect("reset"),
        2
    );
    let (revision, new_hash): (i32, String) = sqlx::query_as(
        "SELECT revision,password_hash FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("reset credential");
    assert_eq!(revision, 2);
    assert_ne!(hash, new_hash);
    let parsed = PasswordHash::new(&new_hash).expect("valid reset PHC");
    assert!(
        Argon2::default()
            .verify_password(b"second-password-long", &parsed)
            .is_ok()
    );
    assert!(
        Argon2::default()
            .verify_password(b"first-password-long", &parsed)
            .is_err()
    );
    assert_eq!(counts(&pool).await, (1, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_proofs_and_identities_leave_valid_proof_usable(pool: PgPool) {
    let time = clock();
    account(&pool, "Case@example.com", &time).await;
    account(&pool, "other@example.com", &time).await;
    let registration = email_verification::issue(&pool, "Case@example.com", &time)
        .await
        .expect("registration issue");
    let token = customer_passwords::issue(&pool, "Case@example.com", &time)
        .await
        .expect("issue");
    let wrong = PasswordChangeToken::from_bytes([8; 32]);
    let registration_as_password = PasswordChangeToken::from_bytes(*registration.expose_bytes());
    for (email, proof) in [
        ("Case@example.com", &wrong),
        ("Case@example.com", &registration_as_password),
        ("other@example.com", &token),
        ("case@example.com", &token),
        ("missing@example.com", &token),
    ] {
        assert!(matches!(
            customer_passwords::replace(&pool, email, proof, "a-password-long-enough", &time).await,
            Err(PasswordError::Denied)
        ));
    }
    assert!(matches!(
        customer_passwords::issue(&pool, "missing@example.com", &time).await,
        Err(PasswordError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    assert!(matches!(
        customer_passwords::replace(&pool, "Case@example.com", &token, "short", &time).await,
        Err(PasswordError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "Case@EXAMPLE.COM",
            &token,
            "a-password-long-enough",
            &time
        )
        .await
        .expect("valid"),
        1
    );
    assert!(matches!(
        customer_passwords::replace(
            &pool,
            "Case@example.com",
            &token,
            "another-password-long",
            &time
        )
        .await,
        Err(PasswordError::Denied)
    ));
    let expired = customer_passwords::issue(&pool, "Case@example.com", &time)
        .await
        .expect("issue");
    let expires: i64 =
        sqlx::query_scalar("SELECT expires_at FROM customer_password_challenges WHERE verifier=$1")
            .bind(verifier(&expired))
            .fetch_one(&pool)
            .await
            .expect("expiry");
    time.0.store(expires as u64, Ordering::SeqCst);
    assert!(matches!(
        customer_passwords::replace(
            &pool,
            "Case@example.com",
            &expired,
            "another-password-long",
            &time
        )
        .await,
        Err(PasswordError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn same_proof_has_one_winner_and_sibling_is_stale(pool: PgPool) {
    let time = clock();
    account(&pool, "a@example.com", &time).await;
    let proof = customer_passwords::issue(&pool, "a@example.com", &time)
        .await
        .expect("issue");
    let sibling = customer_passwords::issue(&pool, "a@example.com", &time)
        .await
        .expect("issue sibling");
    let (a, b) = tokio::join!(
        customer_passwords::replace(&pool, "a@example.com", &proof, "first-password-long", &time),
        customer_passwords::replace(
            &pool,
            "a@example.com",
            &proof,
            "second-password-long",
            &time
        )
    );
    assert!(matches!(
        (a, b),
        (Ok(1), Err(PasswordError::Denied)) | (Err(PasswordError::Denied), Ok(1))
    ));
    assert!(matches!(
        customer_passwords::replace(
            &pool,
            "a@example.com",
            &sibling,
            "stale-password-long",
            &time
        )
        .await,
        Err(PasswordError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, 1));
    let fresh = customer_passwords::issue(&pool, "a@example.com", &time)
        .await
        .expect("fresh");
    assert_eq!(
        customer_passwords::replace(&pool, "a@example.com", &fresh, "fresh-password-long", &time)
            .await
            .expect("fresh reset"),
        2
    );
    assert_eq!(counts(&pool).await, (1, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_credential_write_rolls_back_consumption(pool: PgPool) {
    let time = clock();
    account(&pool, "a@example.com", &time).await;
    let proof = customer_passwords::issue(&pool, "a@example.com", &time)
        .await
        .expect("issue");
    sqlx::query("CREATE FUNCTION suppress_password_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").execute(&pool).await.expect("fault function");
    sqlx::query("CREATE TRIGGER suppress_password_insert BEFORE INSERT ON customer_password_credentials FOR EACH ROW EXECUTE FUNCTION suppress_password_write()").execute(&pool).await.expect("fault trigger");
    assert!(matches!(
        customer_passwords::replace(
            &pool,
            "a@example.com",
            &proof,
            "a-password-long-enough",
            &time
        )
        .await,
        Err(PasswordError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    sqlx::query("DROP TRIGGER suppress_password_insert ON customer_password_credentials")
        .execute(&pool)
        .await
        .expect("remove fault");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "a@example.com",
            &proof,
            "a-password-long-enough",
            &time
        )
        .await
        .expect("retry"),
        1
    );
}

async fn wait_for_password_lock(
    pool: &PgPool,
    blocker_pid: i32,
    query_prefix: &str,
    task: &mut tokio::task::JoinHandle<Result<i32, PasswordError>>,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if task.is_finished() {
            panic!(
                "password worker completed before its lock wait: {:?}",
                task.await
            );
        }
        let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND starts_with(query,$2))")
            .bind(blocker_pid).bind(query_prefix).fetch_one(pool).await.expect("inspect exact password wait");
        if waiting {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker never waited on the test's blocker"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn waiting_for_challenge_lock_does_not_extend_expiry(pool: PgPool) {
    use std::sync::Arc;
    let time = Arc::new(clock());
    account(&pool, "a@example.com", &time).await;
    let proof = customer_passwords::issue(&pool, "a@example.com", time.as_ref())
        .await
        .expect("issue");
    let digest = verifier(&proof);
    let expires: i64 =
        sqlx::query_scalar("SELECT expires_at FROM customer_password_challenges WHERE verifier=$1")
            .bind(&digest)
            .fetch_one(&pool)
            .await
            .expect("expiry");
    let mut blocker = pool.begin().await.expect("transaction");
    let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("blocker backend");
    sqlx::query("SELECT verifier FROM customer_password_challenges WHERE verifier=$1 FOR UPDATE")
        .bind(&digest)
        .fetch_one(&mut *blocker)
        .await
        .expect("lock");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let mut task = tokio::spawn(async move {
        customer_passwords::replace(
            &worker_pool,
            "a@example.com",
            &proof,
            "a-password-long-enough",
            worker_time.as_ref(),
        )
        .await
    });
    wait_for_password_lock(
        &pool, blocker_pid,
        "SELECT account_id,expected_revision,created_at,expires_at,consumed_at,consumed_xid IS NULL AS xid_empty FROM customer_password_challenges WHERE verifier=$1 AND account_id=$2 FOR UPDATE",
        &mut task,
    ).await;
    time.0.store(expires as u64, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert!(matches!(
        task.await.expect("task"),
        Err(PasswordError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_sql_requires_exact_same_transaction_change_and_revision(pool: PgPool) {
    let time = clock();
    let first = account(&pool, "first@example.com", &time).await;
    let second = account(&pool, "second@example.com", &time).await;
    let initial = customer_passwords::issue(&pool, "first@example.com", &time)
        .await
        .expect("initial issue");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "first@example.com",
            &initial,
            "first-password-long",
            &time
        )
        .await
        .expect("initial credential"),
        1
    );
    let hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("valid hash");

    let detached = customer_passwords::issue(&pool, "first@example.com", &time)
        .await
        .expect("detached issue");
    assert!(
        sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
            .bind(verifier(&detached))
            .execute(&pool)
            .await
            .is_err(),
        "detached consumption must fail at commit"
    );
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_at FROM customer_password_challenges WHERE verifier=$1",
    )
    .bind(verifier(&detached))
    .fetch_one(&pool)
    .await
    .expect("unchanged proof");
    assert_eq!(consumed, None);

    let cross = customer_passwords::issue(&pool, "second@example.com", &time)
        .await
        .expect("cross issue");
    let mut tx = pool.begin().await.expect("cross transaction");
    sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(verifier(&cross))
        .execute(&mut *tx)
        .await
        .expect("consume");
    assert!(sqlx::query("UPDATE customer_password_credentials SET revision=2,password_hash=$1,changed_at=1000,challenge_verifier=$2 WHERE account_id=$3").bind(&hash).bind(verifier(&cross)).bind(first).execute(&mut *tx).await.is_err(), "a different account's proof cannot authorize replacement");
    tx.rollback().await.expect("rollback cross");

    let mut tx = pool.begin().await.expect("jump transaction");
    sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(verifier(&detached))
        .execute(&mut *tx)
        .await
        .expect("consume");
    assert!(sqlx::query("UPDATE customer_password_credentials SET revision=3,password_hash=$1,changed_at=1000,challenge_verifier=$2 WHERE account_id=$3").bind(&hash).bind(verifier(&detached)).bind(first).execute(&mut *tx).await.is_err(), "revision cannot skip");
    tx.rollback().await.expect("rollback jump");

    let mut tx = pool.begin().await.expect("healthy transaction");
    sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(verifier(&cross))
        .execute(&mut *tx)
        .await
        .expect("native consume");
    let inserted = sqlx::query("INSERT INTO customer_password_credentials(account_id,revision,password_hash,changed_at,challenge_verifier) VALUES ($1,1,$2,1000,$3)").bind(second).bind(&hash).bind(verifier(&cross)).execute(&mut *tx).await.expect("exact native credential");
    assert_eq!(inserted.rows_affected(), 1);
    sqlx::query("SET CONSTRAINTS require_customer_password_change IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("deferred custody");
    tx.commit().await.expect("healthy commit");
    assert_eq!(counts(&pool).await, (2, 2));
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "first@example.com",
            &detached,
            "fresh-password-long",
            &time
        )
        .await
        .expect("proof survived failed SQL"),
        2
    );
    assert_eq!(counts(&pool).await, (2, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn password_nfc_preserves_spaces_and_bounds_both_input_and_normalized_bytes(pool: PgPool) {
    let time = clock();
    let id = account(&pool, "unicode@example.com", &time).await;
    let proof = customer_passwords::issue(&pool, "unicode@example.com", &time)
        .await
        .expect("issue");
    let composed = "  café phrase longer  ";
    let decomposed = "  cafe\u{301} phrase longer  ";
    assert_eq!(
        customer_passwords::replace(&pool, "unicode@example.com", &proof, decomposed, &time)
            .await
            .expect("normalized password"),
        1
    );
    let hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("credential");
    let parsed = PasswordHash::new(&hash).expect("PHC");
    assert!(
        Argon2::default()
            .verify_password(composed.as_bytes(), &parsed)
            .is_ok()
    );
    assert!(
        Argon2::default()
            .verify_password(decomposed.as_bytes(), &parsed)
            .is_err()
    );
    assert!(
        Argon2::default()
            .verify_password(composed.trim().as_bytes(), &parsed)
            .is_err()
    );

    let next = customer_passwords::issue(&pool, "unicode@example.com", &time)
        .await
        .expect("next issue");
    let max = "😀".repeat(64);
    let oversized_input = format!("{max}a");
    let oversized_normalized = "\u{0344}".repeat(65);
    assert_eq!(max.len(), 256);
    assert_eq!(oversized_input.len(), 257);
    assert_eq!(oversized_normalized.len(), 130);
    for password in [
        "abcdefghijklmn",
        oversized_input.as_str(),
        oversized_normalized.as_str(),
    ] {
        assert!(matches!(
            customer_passwords::replace(&pool, "unicode@example.com", &next, password, &time).await,
            Err(PasswordError::Denied)
        ));
        assert_eq!(counts(&pool).await, (1, 1));
    }
    assert_eq!(
        customer_passwords::replace(&pool, "unicode@example.com", &next, &max, &time)
            .await
            .expect("256 bytes accepted"),
        2
    );
    let hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("max credential");
    assert!(
        Argon2::default()
            .verify_password(max.as_bytes(), &PasswordHash::new(&hash).expect("PHC"))
            .is_ok()
    );
    assert_eq!(counts(&pool).await, (1, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_credential_insert_rolls_back_both_writes(pool: PgPool) {
    use std::sync::Arc;
    let time = Arc::new(clock());
    account(&pool, "fenced@example.com", &time).await;
    let proof = customer_passwords::issue(&pool, "fenced@example.com", time.as_ref())
        .await
        .expect("issue");
    let expires: i64 =
        sqlx::query_scalar("SELECT expires_at FROM customer_password_challenges WHERE verifier=$1")
            .bind(verifier(&proof))
            .fetch_one(&pool)
            .await
            .expect("expiry");
    sqlx::query("CREATE FUNCTION fence_password_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(17543821); RETURN NEW; END $$").execute(&pool).await.expect("fence function");
    sqlx::query("CREATE TRIGGER fence_password_insert AFTER INSERT ON customer_password_credentials FOR EACH ROW EXECUTE FUNCTION fence_password_insert()").execute(&pool).await.expect("fence trigger");
    let mut blocker = pool.begin().await.expect("blocker");
    let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("blocker backend");
    sqlx::query("SELECT pg_advisory_xact_lock(17543821)")
        .execute(&mut *blocker)
        .await
        .expect("hold fence");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_proof = PasswordChangeToken::from_bytes(*proof.expose_bytes());
    let mut task = tokio::spawn(async move {
        customer_passwords::replace(
            &worker_pool,
            "fenced@example.com",
            &worker_proof,
            "fenced-password-long",
            worker_time.as_ref(),
        )
        .await
    });
    wait_for_password_lock(
        &pool,
        blocker_pid,
        "INSERT INTO customer_password_credentials",
        &mut task,
    )
    .await;
    time.0.store(expires as u64, Ordering::SeqCst);
    blocker.commit().await.expect("release fence");
    assert!(matches!(
        task.await.expect("task"),
        Err(PasswordError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    sqlx::query("DROP TRIGGER fence_password_insert ON customer_password_credentials")
        .execute(&pool)
        .await
        .expect("remove fence");
    time.0.store(1000, Ordering::SeqCst);
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "fenced@example.com",
            &proof,
            "fenced-password-long",
            time.as_ref()
        )
        .await
        .expect("healthy proof"),
        1
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[path = "support/customer_password_issue_unit.rs"]
mod customer_password_issue_unit;
#[path = "support/customer_password_upgrade.rs"]
mod customer_password_upgrade;
