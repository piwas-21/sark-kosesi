#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;

#[path = "support/admitted_login.rs"]
mod admitted_login;

use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_sessions::{self, CustomerSessionToken, SessionError},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;

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
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("issue verification");
    establish(pool, email, &proof, time)
        .await
        .expect("verified account")
        .value()
}
async fn set_password(pool: &PgPool, email: &str, password: &str, time: &TestClock) -> i32 {
    let proof = customer_passwords::issue(pool, email, time)
        .await
        .expect("issue password proof");
    customer_passwords::replace(pool, email, &proof, password, time)
        .await
        .expect("replace password")
}
fn digest(token: &CustomerSessionToken) -> Vec<u8> {
    Sha256::new()
        .chain_update(b"piwas:customer-session:verifier:v1\0")
        .chain_update(token.expose_bytes())
        .finalize()
        .to_vec()
}
async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM customer_sessions")
        .fetch_one(pool)
        .await
        .expect("session count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn login_checks_identity_password_normalization_and_domain_separation(pool: PgPool) {
    let time = clock();
    let id = account(&pool, "Case+tag@EXAMPLE.COM", &time).await;
    account(&pool, "unset@example.com", &time).await;
    set_password(
        &pool,
        "Case+tag@example.com",
        "  cafe\u{301} phrase longer  ",
        &time,
    )
    .await;
    for (email, password) in [
        ("Case+tag@example.com", "wrong-password-long"),
        ("case+tag@example.com", "  café phrase longer  "),
        ("missing@example.com", "  café phrase longer  "),
        ("unset@example.com", "  café phrase longer  "),
        ("Case+tag@example.com", "café phrase longer"),
    ] {
        assert!(matches!(
            admitted_login::login(&pool, email, password, &time).await,
            Err(SessionError::Denied)
        ));
    }
    assert_eq!(count(&pool).await, 0);
    let session = admitted_login::login(
        &pool,
        "Case+tag@EXAMPLE.com",
        "  café phrase longer  ",
        &time,
    )
    .await
    .expect("login");
    assert_eq!(
        (
            session.identity.account_id,
            session.identity.credential_revision,
            session.identity.expires_at
        ),
        (id, 1, 44200)
    );
    let stored: (Vec<u8>, i64, i32, i64, i64) = sqlx::query_as("SELECT verifier,account_id,credential_revision,created_at,expires_at FROM customer_sessions")
        .fetch_one(&pool).await.expect("stored session");
    assert_eq!(stored, (digest(&session.token), id, 1, 1000, 44200));
    assert_ne!(stored.0, session.token.expose_bytes().to_vec());
    let other_domain = Sha256::new()
        .chain_update(b"piwas:customer-password-change:verifier:v1\0")
        .chain_update(session.token.expose_bytes())
        .finalize()
        .to_vec();
    assert_ne!(stored.0, other_domain);
    let identity = customer_sessions::authenticate(&pool, &session.token, &time)
        .await
        .expect("authenticate");
    assert_eq!(
        (
            identity.account_id,
            identity.credential_revision,
            identity.expires_at
        ),
        (id, 1, 44200)
    );
    let password_proof = customer_passwords::issue(&pool, "Case+tag@example.com", &time)
        .await
        .expect("password proof");
    let cross_domain = CustomerSessionToken::from_bytes(*password_proof.expose_bytes());
    assert!(matches!(
        customer_sessions::authenticate(&pool, &cross_domain, &time).await,
        Err(SessionError::Denied)
    ));
    let wrong = CustomerSessionToken::from_bytes([8; 32]);
    assert!(matches!(
        customer_sessions::authenticate(&pool, &wrong, &time).await,
        Err(SessionError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn password_reset_invalidates_existing_sessions_and_requires_new_password(pool: PgPool) {
    let time = clock();
    let id = account(&pool, "reset@example.com", &time).await;
    assert_eq!(
        set_password(&pool, "reset@example.com", "first-password-long", &time).await,
        1
    );
    let session = admitted_login::login(&pool, "reset@example.com", "first-password-long", &time)
        .await
        .expect("first login");
    assert_eq!(
        set_password(&pool, "reset@example.com", "second-password-long", &time).await,
        2
    );
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::rotate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::logout(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        admitted_login::login(&pool, "reset@example.com", "first-password-long", &time).await,
        Err(SessionError::Denied)
    ));
    let fresh = admitted_login::login(&pool, "reset@example.com", "second-password-long", &time)
        .await
        .expect("new login");
    assert_eq!(
        (
            fresh.identity.account_id,
            fresh.identity.credential_revision
        ),
        (id, 2)
    );
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotation_preserves_deadline_and_logout_revokes_successor(pool: PgPool) {
    let time = clock();
    account(&pool, "rotate@example.com", &time).await;
    set_password(&pool, "rotate@example.com", "first-password-long", &time).await;
    let initial = admitted_login::login(&pool, "rotate@example.com", "first-password-long", &time)
        .await
        .expect("login");
    time.0.store(2000, Ordering::SeqCst);
    let next = customer_sessions::rotate(&pool, &initial.token, &time)
        .await
        .expect("rotate");
    assert_ne!(initial.token.expose_bytes(), next.token.expose_bytes());
    assert_eq!(next.identity.expires_at, initial.identity.expires_at);
    assert_eq!(next.identity.account_id, initial.identity.account_id);
    assert_eq!(
        next.identity.credential_revision,
        initial.identity.credential_revision
    );
    let predecessor: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT rotated_from FROM customer_sessions WHERE verifier=$1")
            .bind(digest(&next.token))
            .fetch_one(&pool)
            .await
            .expect("rotation chain");
    assert_eq!(predecessor, Some(digest(&initial.token)));
    assert!(matches!(
        customer_sessions::authenticate(&pool, &initial.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::rotate(&pool, &initial.token, &time).await,
        Err(SessionError::Denied)
    ));
    customer_sessions::authenticate(&pool, &next.token, &time)
        .await
        .expect("successor valid");
    customer_sessions::logout(&pool, &next.token, &time)
        .await
        .expect("logout");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &next.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::logout(&pool, &next.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_rotation_has_exactly_one_successor(pool: PgPool) {
    let time = clock();
    account(&pool, "concurrent@example.com", &time).await;
    set_password(
        &pool,
        "concurrent@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "concurrent@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");
    let (a, b) = tokio::join!(
        customer_sessions::rotate(&pool, &initial.token, &time),
        customer_sessions::rotate(&pool, &initial.token, &time)
    );
    let winner = match (a, b) {
        (Ok(winner), Err(SessionError::Denied)) | (Err(SessionError::Denied), Ok(winner)) => winner,
        _ => panic!("exactly one rotation must succeed"),
    };
    customer_sessions::authenticate(&pool, &winner.token, &time)
        .await
        .expect("winner valid");
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_while_waiting_for_session_lock_denies_and_preserves_row(pool: PgPool) {
    let time = Arc::new(clock());
    account(&pool, "wait@example.com", &time).await;
    set_password(&pool, "wait@example.com", "first-password-long", &time).await;
    let initial = admitted_login::login(
        &pool,
        "wait@example.com",
        "first-password-long",
        time.as_ref(),
    )
    .await
    .expect("login");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT verifier FROM customer_sessions WHERE verifier=$1 FOR UPDATE")
        .bind(digest(&initial.token))
        .fetch_one(&mut *blocker)
        .await
        .expect("lock session");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let token = CustomerSessionToken::from_bytes(*initial.token.expose_bytes());
    let task = tokio::spawn(async move {
        customer_sessions::rotate(&worker_pool, &token, worker_time.as_ref()).await
    });
    let mut waiting = false;
    for _ in 0..300 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='transactionid' AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("locks");
        if n > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(waiting && !task.is_finished());
    time.0
        .store(initial.identity.expires_at as u64, Ordering::SeqCst);
    blocker.commit().await.expect("release");
    assert!(matches!(
        task.await.expect("join"),
        Err(SessionError::Denied)
    ));
    assert_eq!(count(&pool).await, 1);
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM customer_sessions WHERE verifier=$1")
            .bind(digest(&initial.token))
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(revoked, None);
    assert!(matches!(
        customer_sessions::authenticate(&pool, &initial.token, &time).await,
        Err(SessionError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_sql_rejects_orphan_detached_and_wrong_revision_rotation(pool: PgPool) {
    let time = clock();
    let id = account(&pool, "native@example.com", &time).await;
    set_password(&pool, "native@example.com", "first-password-long", &time).await;
    let initial = admitted_login::login(&pool, "native@example.com", "first-password-long", &time)
        .await
        .expect("login");
    let prior = digest(&initial.token);
    let orphan = vec![31u8; 32];
    assert!(sqlx::query("INSERT INTO customer_sessions(verifier,account_id,credential_revision,created_at,expires_at,rotated_from) VALUES ($1,$2,1,1001,44200,$3)")
        .bind(&orphan).bind(id).bind(&prior).execute(&pool).await.is_err(), "successor needs same-transaction revocation");
    assert!(sqlx::query("INSERT INTO customer_sessions(verifier,account_id,credential_revision,created_at,expires_at) VALUES ($1,$2,2,1000,44200)")
        .bind(&orphan).bind(id).execute(&pool).await.is_err(), "issuance must use current revision");
    assert!(sqlx::query("UPDATE customer_sessions SET revoked_at=1001,revocation_kind='rotation' WHERE verifier=$1")
        .bind(&prior).execute(&pool).await.is_err(), "rotation without successor cannot commit");
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM customer_sessions WHERE verifier=$1")
            .bind(&prior)
            .fetch_one(&pool)
            .await
            .expect("unmodified predecessor");
    assert_eq!(revoked, None);
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query(
        "UPDATE customer_sessions SET revoked_at=1001,revocation_kind='rotation' WHERE verifier=$1",
    )
    .bind(&prior)
    .execute(&mut *tx)
    .await
    .expect("rotation revocation");
    assert!(sqlx::query("INSERT INTO customer_sessions(verifier,account_id,credential_revision,created_at,expires_at,rotated_from) VALUES ($1,$2,2,1001,44200,$3)")
        .bind(&orphan).bind(id).bind(&prior).execute(&mut *tx).await.is_err(), "successor cannot change revision");
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("positive transaction");
    sqlx::query(
        "UPDATE customer_sessions SET revoked_at=1001,revocation_kind='rotation' WHERE verifier=$1",
    )
    .bind(&prior)
    .execute(&mut *tx)
    .await
    .expect("revoke");
    sqlx::query("INSERT INTO customer_sessions(verifier,account_id,credential_revision,created_at,expires_at,rotated_from) VALUES ($1,$2,1,1001,44200,$3)")
        .bind(&orphan).bind(id).bind(&prior).execute(&mut *tx).await.expect("exact successor");
    sqlx::query("SET CONSTRAINTS require_customer_session_rotation IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("complete custody");
    tx.commit().await.expect("complete rotation");
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn login_verified_before_password_reset_cannot_issue_after_reset(pool: PgPool) {
    let time = Arc::new(clock());
    let id = account(&pool, "race@example.com", &time).await;
    set_password(&pool, "race@example.com", "first-password-long", &time).await;
    let source = account(&pool, "hash@example.com", &time).await;
    set_password(&pool, "hash@example.com", "second-password-long", &time).await;
    let new_hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .expect("known new password hash");
    let reset = customer_passwords::issue(&pool, "race@example.com", time.as_ref())
        .await
        .expect("reset proof");
    let reset_digest = Sha256::new()
        .chain_update(b"piwas:customer-password-change:verifier:v1\0")
        .chain_update(reset.expose_bytes())
        .finalize()
        .to_vec();
    let mut blocker = pool.begin().await.expect("account transaction");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .expect("claim account");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let task = tokio::spawn(async move {
        admitted_login::login(
            &worker_pool,
            "race@example.com",
            "first-password-long",
            worker_time.as_ref(),
        )
        .await
    });
    let mut waiting = false;
    for _ in 0..300 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='transactionid' AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("lock waits");
        if n > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        waiting && !task.is_finished(),
        "login finished password verification and waits on account"
    );
    sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(&reset_digest)
        .execute(&mut *blocker)
        .await
        .expect("consume exact proof");
    sqlx::query("UPDATE customer_password_credentials SET revision=2,password_hash=$1,changed_at=1000,challenge_verifier=$2 WHERE account_id=$3").bind(&new_hash).bind(&reset_digest).bind(id).execute(&mut *blocker).await.expect("replace credential");
    sqlx::query("SET CONSTRAINTS require_customer_password_change IMMEDIATE")
        .execute(&mut *blocker)
        .await
        .expect("complete reset");
    blocker.commit().await.expect("commit reset");
    assert!(matches!(
        task.await.expect("join"),
        Err(SessionError::Denied)
    ));
    assert_eq!(count(&pool).await, 0);
    let fresh = admitted_login::login(
        &pool,
        "race@example.com",
        "second-password-long",
        time.as_ref(),
    )
    .await
    .expect("new password works");
    assert_eq!(
        (
            fresh.identity.account_id,
            fresh.identity.credential_revision
        ),
        (id, 2)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_clock_failures_roll_back_both_rotation_writes(pool: PgPool) {
    let time = Arc::new(clock());
    account(&pool, "late-clock@example.com", &time).await;
    set_password(
        &pool,
        "late-clock@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "late-clock@example.com",
        "first-password-long",
        time.as_ref(),
    )
    .await
    .expect("login");
    sqlx::query("CREATE FUNCTION fence_session_successor() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.rotated_from IS NOT NULL THEN PERFORM pg_advisory_xact_lock(17543822); END IF; RETURN NEW; END $$").execute(&pool).await.expect("fence function");
    sqlx::query("CREATE TRIGGER fence_session_successor AFTER INSERT ON customer_sessions FOR EACH ROW EXECUTE FUNCTION fence_session_successor()").execute(&pool).await.expect("fence trigger");
    for (late_time, backward) in [(initial.identity.expires_at as u64, false), (999, true)] {
        time.0.store(1000, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("fence blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(17543822)")
            .execute(&mut *blocker)
            .await
            .expect("hold fence");
        let worker_pool = pool.clone();
        let worker_time = time.clone();
        let token = CustomerSessionToken::from_bytes(*initial.token.expose_bytes());
        let task = tokio::spawn(async move {
            customer_sessions::rotate(&worker_pool, &token, worker_time.as_ref()).await
        });
        let mut waiting = false;
        for _ in 0..300 {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND classid=0 AND objid=17543822 AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("fence waits");
            if n > 0 {
                waiting = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            waiting && !task.is_finished(),
            "rotation reached successor insert"
        );
        time.0.store(late_time, Ordering::SeqCst);
        blocker.commit().await.expect("release fence");
        let result = task.await.expect("join");
        if backward {
            assert!(matches!(result, Err(SessionError::Unavailable)));
        } else {
            assert!(matches!(result, Err(SessionError::Denied)));
        }
        assert_eq!(count(&pool).await, 1, "successor insert rolled back");
        let revoked: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at FROM customer_sessions WHERE verifier=$1")
                .bind(digest(&initial.token))
                .fetch_one(&pool)
                .await
                .expect("predecessor");
        assert_eq!(revoked, None, "predecessor revocation rolled back");
    }
    sqlx::query("DROP TRIGGER fence_session_successor ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove fence");
    time.0.store(1000, Ordering::SeqCst);
    customer_sessions::authenticate(&pool, &initial.token, time.as_ref())
        .await
        .expect("original remains valid");
    let successor = customer_sessions::rotate(&pool, &initial.token, time.as_ref())
        .await
        .expect("healthy retry");
    customer_sessions::authenticate(&pool, &successor.token, time.as_ref())
        .await
        .expect("successor valid");
    assert_eq!(count(&pool).await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn password_reset_wins_against_rotation_waiting_for_account(pool: PgPool) {
    let time = Arc::new(clock());
    let id = account(&pool, "rotation-race@example.com", &time).await;
    set_password(
        &pool,
        "rotation-race@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "rotation-race@example.com",
        "first-password-long",
        time.as_ref(),
    )
    .await
    .expect("login");
    let source = account(&pool, "rotation-hash@example.com", &time).await;
    set_password(
        &pool,
        "rotation-hash@example.com",
        "second-password-long",
        &time,
    )
    .await;
    let hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM customer_password_credentials WHERE account_id=$1",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .expect("known new password hash");
    let reset = customer_passwords::issue(&pool, "rotation-race@example.com", time.as_ref())
        .await
        .expect("reset proof");
    let proof = Sha256::new()
        .chain_update(b"piwas:customer-password-change:verifier:v1\0")
        .chain_update(reset.expose_bytes())
        .finalize()
        .to_vec();
    let mut blocker = pool.begin().await.expect("account transaction");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .expect("claim account");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let token = CustomerSessionToken::from_bytes(*initial.token.expose_bytes());
    let task = tokio::spawn(async move {
        customer_sessions::rotate(&worker_pool, &token, worker_time.as_ref()).await
    });
    let mut waiting = false;
    for _ in 0..300 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='transactionid' AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("lock waits");
        if n > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        waiting && !task.is_finished(),
        "rotation waits for the account claim"
    );
    sqlx::query("UPDATE customer_password_challenges SET consumed_at=1000 WHERE verifier=$1")
        .bind(&proof)
        .execute(&mut *blocker)
        .await
        .expect("consume reset proof");
    sqlx::query("UPDATE customer_password_credentials SET revision=2,password_hash=$1,changed_at=1000,challenge_verifier=$2 WHERE account_id=$3").bind(&hash).bind(&proof).bind(id).execute(&mut *blocker).await.expect("replace credential");
    sqlx::query("SET CONSTRAINTS require_customer_password_change IMMEDIATE")
        .execute(&mut *blocker)
        .await
        .expect("complete reset");
    blocker.commit().await.expect("commit reset");
    assert!(matches!(
        task.await.expect("join"),
        Err(SessionError::Denied)
    ));
    assert_eq!(count(&pool).await, 1, "stale rotation issued no successor");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &initial.token, time.as_ref()).await,
        Err(SessionError::Denied)
    ));
    let fresh = admitted_login::login(
        &pool,
        "rotation-race@example.com",
        "second-password-long",
        time.as_ref(),
    )
    .await
    .expect("new password works");
    assert_eq!(
        (
            fresh.identity.account_id,
            fresh.identity.credential_revision
        ),
        (id, 2)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logout_late_clock_failures_roll_back_revocation(pool: PgPool) {
    let time = Arc::new(clock());
    account(&pool, "logout-clock@example.com", &time).await;
    set_password(
        &pool,
        "logout-clock@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "logout-clock@example.com",
        "first-password-long",
        time.as_ref(),
    )
    .await
    .expect("login");
    sqlx::query("CREATE FUNCTION fence_session_logout() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.revocation_kind='logout' THEN PERFORM pg_advisory_xact_lock(17543823); END IF; RETURN NEW; END $$")
        .execute(&pool).await.expect("fence function");
    sqlx::query("CREATE TRIGGER fence_session_logout AFTER UPDATE ON customer_sessions FOR EACH ROW EXECUTE FUNCTION fence_session_logout()")
        .execute(&pool).await.expect("fence trigger");

    for (late_time, backward) in [(initial.identity.expires_at as u64, false), (999, true)] {
        time.0.store(1000, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("fence blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(17543823)")
            .execute(&mut *blocker)
            .await
            .expect("hold fence");
        let worker_pool = pool.clone();
        let worker_time = time.clone();
        let token = CustomerSessionToken::from_bytes(*initial.token.expose_bytes());
        let task = tokio::spawn(async move {
            customer_sessions::logout(&worker_pool, &token, worker_time.as_ref()).await
        });
        let mut waiting = false;
        for _ in 0..300 {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND classid=0 AND objid=17543823 AND NOT granted AND pid<>pg_backend_pid()")
                .fetch_one(&pool).await.expect("fence waits");
            if n > 0 {
                waiting = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(waiting && !task.is_finished(), "logout reached its write");
        time.0.store(late_time, Ordering::SeqCst);
        blocker.commit().await.expect("release fence");
        let result = task.await.expect("join");
        if backward {
            assert!(matches!(result, Err(SessionError::Unavailable)));
        } else {
            assert!(matches!(result, Err(SessionError::Denied)));
        }
        let persisted: (Option<i64>, Option<String>, bool) = sqlx::query_as(
            "SELECT revoked_at,revocation_kind,revoked_xid IS NULL FROM customer_sessions WHERE verifier=$1",
        )
        .bind(digest(&initial.token)).fetch_one(&pool).await.expect("original row");
        assert_eq!(persisted, (None, None, true), "revocation rolled back");
        assert_eq!(count(&pool).await, 1);
    }
    sqlx::query("DROP TRIGGER fence_session_logout ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove fence");
    time.0.store(1000, Ordering::SeqCst);
    customer_sessions::authenticate(&pool, &initial.token, time.as_ref())
        .await
        .expect("original remains active");
    customer_sessions::logout(&pool, &initial.token, time.as_ref())
        .await
        .expect("healthy logout");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &initial.token, time.as_ref()).await,
        Err(SessionError::Denied)
    ));
    let persisted: (Option<i64>, Option<String>, bool) = sqlx::query_as(
        "SELECT revoked_at,revocation_kind,revoked_xid IS NOT NULL FROM customer_sessions WHERE verifier=$1",
    )
    .bind(digest(&initial.token)).fetch_one(&pool).await.expect("revoked row");
    assert_eq!(persisted, (Some(1000), Some("logout".to_owned()), true));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logout_rejects_suppressed_and_changed_database_writes(pool: PgPool) {
    let time = clock();
    account(&pool, "logout-write@example.com", &time).await;
    set_password(
        &pool,
        "logout-write@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "logout-write@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");
    sqlx::query("CREATE FUNCTION suppress_session_logout() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("suppression function");
    sqlx::query("CREATE TRIGGER z_suppress_session_logout BEFORE UPDATE ON customer_sessions FOR EACH ROW EXECUTE FUNCTION suppress_session_logout()")
        .execute(&pool).await.expect("suppression trigger");
    assert!(matches!(
        customer_sessions::logout(&pool, &initial.token, &time).await,
        Err(SessionError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER z_suppress_session_logout ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove suppression");

    // This trigger runs after the custody guard and models a write altered by
    // the database. The application must check the persisted transition.
    sqlx::query("CREATE FUNCTION change_session_logout() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.revoked_at := NEW.revoked_at + 1; RETURN NEW; END $$")
        .execute(&pool).await.expect("alteration function");
    sqlx::query("CREATE TRIGGER z_change_session_logout BEFORE UPDATE ON customer_sessions FOR EACH ROW EXECUTE FUNCTION change_session_logout()")
        .execute(&pool).await.expect("alteration trigger");
    assert!(matches!(
        customer_sessions::logout(&pool, &initial.token, &time).await,
        Err(SessionError::Unavailable)
    ));
    let persisted: (Option<i64>, Option<String>, bool) = sqlx::query_as(
        "SELECT revoked_at,revocation_kind,revoked_xid IS NULL FROM customer_sessions WHERE verifier=$1",
    )
    .bind(digest(&initial.token)).fetch_one(&pool).await.expect("original row");
    assert_eq!(
        persisted,
        (None, None, true),
        "both failed writes rolled back"
    );
    assert_eq!(count(&pool).await, 1);
    sqlx::query("DROP TRIGGER z_change_session_logout ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove alteration");
    customer_sessions::authenticate(&pool, &initial.token, &time)
        .await
        .expect("original active");
    customer_sessions::logout(&pool, &initial.token, &time)
        .await
        .expect("healthy logout");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &initial.token, &time).await,
        Err(SessionError::Denied)
    ));
    let persisted: (Option<i64>, Option<String>, bool) = sqlx::query_as(
        "SELECT revoked_at,revocation_kind,revoked_xid IS NOT NULL FROM customer_sessions WHERE verifier=$1",
    )
    .bind(digest(&initial.token)).fetch_one(&pool).await.expect("revoked row");
    assert_eq!(persisted, (Some(1000), Some("logout".to_owned()), true));
}

struct SequenceClock {
    values: Vec<u64>,
    next: AtomicUsize,
}
impl SequenceClock {
    fn new(values: &[u64]) -> Self {
        Self {
            values: values.to_vec(),
            next: AtomicUsize::new(0),
        }
    }
}
impl Clock for SequenceClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.values
            .get(self.next.fetch_add(1, Ordering::SeqCst))
            .copied()
            .ok_or(ContextError::ClockUnavailable)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_at_each_final_prewrite_observation_denies_without_effects(pool: PgPool) {
    let time = clock();
    account(&pool, "boundaries@example.com", &time).await;
    set_password(
        &pool,
        "boundaries@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "boundaries@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");

    // Initial and claim observations succeed; the final authentication read or
    // the rotation/logout capture sees the exact expiry boundary.
    for operation in 0..3 {
        let clock = SequenceClock::new(&[1000, 1000, 44200]);
        let denied = match operation {
            0 => matches!(
                customer_sessions::authenticate(&pool, &initial.token, &clock).await,
                Err(SessionError::Denied)
            ),
            1 => matches!(
                customer_sessions::rotate(&pool, &initial.token, &clock).await,
                Err(SessionError::Denied)
            ),
            _ => matches!(
                customer_sessions::logout(&pool, &initial.token, &clock).await,
                Err(SessionError::Denied)
            ),
        };
        assert!(denied, "operation {operation} must reject expiry");
        let persisted: (Option<i64>, Option<String>, bool) = sqlx::query_as(
            "SELECT revoked_at,revocation_kind,revoked_xid IS NULL FROM customer_sessions WHERE verifier=$1",
        )
        .bind(digest(&initial.token)).fetch_one(&pool).await.expect("original row");
        assert_eq!(persisted, (None, None, true));
        assert_eq!(count(&pool).await, 1);
    }
    time.0.store(44199, Ordering::SeqCst);
    customer_sessions::authenticate(&pool, &initial.token, &time)
        .await
        .expect("session remains valid immediately before expiry");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn claims_reject_inconsistent_account_and_credential_evidence(pool: PgPool) {
    let time = clock();
    let id = account(&pool, "claim-evidence@example.com", &time).await;
    set_password(
        &pool,
        "claim-evidence@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let session = admitted_login::login(
        &pool,
        "claim-evidence@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");
    customer_sessions::authenticate(&pool, &session.token, &time)
        .await
        .expect("valid evidence");

    // Deliberately simulate corrupted records in this isolated test database;
    // normal guards prevent these transitions.
    sqlx::query(
        "ALTER TABLE verified_customer_accounts DISABLE TRIGGER guard_verified_customer_account",
    )
    .execute(&pool)
    .await
    .expect("disable account guard");
    sqlx::query("UPDATE verified_customer_accounts SET verified_at=1001 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("future verification");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    sqlx::query("ALTER TABLE verified_customer_accounts DROP CONSTRAINT verified_customer_accounts_verified_at_check")
        .execute(&pool).await.expect("remove timestamp check in isolated database");
    sqlx::query("UPDATE verified_customer_accounts SET verified_at=-1 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("corrupt verification time");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    sqlx::query("UPDATE verified_customer_accounts SET verified_at=1000 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("restore verification");
    sqlx::query("ALTER TABLE verified_customer_accounts ADD CONSTRAINT verified_customer_accounts_verified_at_check CHECK (verified_at>=0)")
        .execute(&pool).await.expect("restore timestamp check");
    sqlx::query(
        "ALTER TABLE verified_customer_accounts ENABLE TRIGGER guard_verified_customer_account",
    )
    .execute(&pool)
    .await
    .expect("restore account guard");

    sqlx::query("ALTER TABLE customer_password_credentials DISABLE TRIGGER guard_customer_password_credential")
        .execute(&pool).await.expect("disable credential guard");
    sqlx::query("UPDATE customer_password_credentials SET changed_at=1001 WHERE account_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("future credential");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    sqlx::query("ALTER TABLE customer_password_credentials DROP CONSTRAINT customer_password_credentials_changed_at_check")
        .execute(&pool).await.expect("remove credential timestamp check in isolated database");
    sqlx::query("UPDATE customer_password_credentials SET changed_at=-1 WHERE account_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("corrupt credential time");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    sqlx::query("UPDATE customer_password_credentials SET changed_at=1000 WHERE account_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("restore credential time");
    sqlx::query("ALTER TABLE customer_password_credentials ADD CONSTRAINT customer_password_credentials_changed_at_check CHECK (changed_at>=0)")
        .execute(&pool).await.expect("restore credential timestamp check");
    customer_sessions::authenticate(&pool, &session.token, &time)
        .await
        .expect("restored evidence");
    sqlx::query("DELETE FROM customer_password_credentials WHERE account_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("remove credential");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::rotate(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::logout(&pool, &session.token, &time).await,
        Err(SessionError::Denied)
    ));
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn credential_read_failure_is_unavailable_and_recovers(pool: PgPool) {
    let time = clock();
    account(&pool, "read-failure@example.com", &time).await;
    set_password(
        &pool,
        "read-failure@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let session = admitted_login::login(
        &pool,
        "read-failure@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");
    customer_sessions::authenticate(&pool, &session.token, &time)
        .await
        .expect("healthy claim");
    sqlx::query(
        "ALTER TABLE customer_password_credentials RENAME TO unavailable_customer_credentials",
    )
    .execute(&pool)
    .await
    .expect("make credential table unavailable");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Unavailable)
    ));
    sqlx::query(
        "ALTER TABLE unavailable_customer_credentials RENAME TO customer_password_credentials",
    )
    .execute(&pool)
    .await
    .expect("restore credential table");
    customer_sessions::authenticate(&pool, &session.token, &time)
        .await
        .expect("healthy retry");
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_session_pool_returns_unavailable(pool: PgPool) {
    let time = clock();
    account(&pool, "closed-pool@example.com", &time).await;
    set_password(
        &pool,
        "closed-pool@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let session = admitted_login::login(
        &pool,
        "closed-pool@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");
    customer_sessions::authenticate(&pool, &session.token, &time)
        .await
        .expect("healthy claim");
    pool.close().await;
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session.token, &time).await,
        Err(SessionError::Unavailable)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotation_rejects_suppressed_and_changed_writes_and_rolls_back(pool: PgPool) {
    let time = clock();
    account(&pool, "rotation-write@example.com", &time).await;
    set_password(
        &pool,
        "rotation-write@example.com",
        "first-password-long",
        &time,
    )
    .await;
    let initial = admitted_login::login(
        &pool,
        "rotation-write@example.com",
        "first-password-long",
        &time,
    )
    .await
    .expect("login");

    sqlx::query("CREATE FUNCTION suppress_session_rotation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("suppression function");
    for (name, trigger) in [
        (
            "update",
            "CREATE TRIGGER z_suppress_session_rotation BEFORE UPDATE ON customer_sessions FOR EACH ROW EXECUTE FUNCTION suppress_session_rotation()",
        ),
        (
            "insert",
            "CREATE TRIGGER z_suppress_session_rotation BEFORE INSERT ON customer_sessions FOR EACH ROW EXECUTE FUNCTION suppress_session_rotation()",
        ),
    ] {
        sqlx::query(trigger)
            .execute(&pool)
            .await
            .expect("suppression trigger");
        assert!(
            matches!(
                customer_sessions::rotate(&pool, &initial.token, &time).await,
                Err(SessionError::Unavailable)
            ),
            "suppressed {name}"
        );
        sqlx::query("DROP TRIGGER z_suppress_session_rotation ON customer_sessions")
            .execute(&pool)
            .await
            .expect("remove suppression");
        let revoked: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at FROM customer_sessions WHERE verifier=$1")
                .bind(digest(&initial.token))
                .fetch_one(&pool)
                .await
                .expect("predecessor");
        assert_eq!(revoked, None);
        assert_eq!(count(&pool).await, 1);
    }

    // A later trigger can change a column after the guard has inspected NEW.
    // Rotation's readback must reject even if its reciprocal constraint passes.
    sqlx::query("CREATE FUNCTION alter_rotation_xid() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.created_xid := '1'::xid8; RETURN NEW; END $$")
        .execute(&pool).await.expect("alteration function");
    sqlx::query("CREATE TRIGGER z_alter_rotation_xid BEFORE UPDATE ON customer_sessions FOR EACH ROW EXECUTE FUNCTION alter_rotation_xid()")
        .execute(&pool).await.expect("alteration trigger");
    assert!(matches!(
        customer_sessions::rotate(&pool, &initial.token, &time).await,
        Err(SessionError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER z_alter_rotation_xid ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove alteration");
    let persisted: (Option<i64>, Option<String>, bool) = sqlx::query_as(
        "SELECT revoked_at,revocation_kind,revoked_xid IS NULL FROM customer_sessions WHERE verifier=$1",
    )
    .bind(digest(&initial.token)).fetch_one(&pool).await.expect("original row");
    assert_eq!(persisted, (None, None, true));
    assert_eq!(count(&pool).await, 1);
    let successor = customer_sessions::rotate(&pool, &initial.token, &time)
        .await
        .expect("healthy retry");
    customer_sessions::authenticate(&pool, &successor.token, &time)
        .await
        .expect("successor valid");
    assert_eq!(count(&pool).await, 2);
}
