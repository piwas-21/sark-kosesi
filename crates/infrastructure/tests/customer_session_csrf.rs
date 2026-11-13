//! Private synchronizer custody over isolated PostgreSQL; no HTTP behavior.
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;

use admitted_proofs::{customer_passwords, email_verification, verified_customer_accounts};
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_sessions::{self, CustomerCsrfToken, IssuedCsrfSession, SessionError},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;

static CASE: Mutex<()> = Mutex::const_new(());
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn time() -> Time {
    Time(AtomicU64::new(1000))
}
async fn account(pool: &PgPool, email: &str, time: &Time) {
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("verification");
    verified_customer_accounts::establish(pool, email, &proof, time)
        .await
        .expect("account");
    let proof = customer_passwords::issue(pool, email, time)
        .await
        .expect("proof");
    customer_passwords::replace(pool, email, &proof, "long fixture password", time)
        .await
        .expect("password");
}
async fn issue(pool: &PgPool, email: &str, time: &Time) -> IssuedCsrfSession {
    let key = AdmissionKey::new(1, [7; 32]).expect("key");
    let limits = AdmissionLimits::new(1000, 100).expect("limits");
    customer_sessions::login_with_csrf(pool, email, "long fixture password", time, &key, limits)
        .await
        .expect("issue")
}
fn verifier(token: &customer_sessions::CustomerSessionToken) -> Vec<u8> {
    Sha256::new()
        .chain_update(b"piwas:customer-session:verifier:v1\0")
        .chain_update(token.expose_bytes())
        .finalize()
        .to_vec()
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn proof_is_distinct_bound_immutable_and_revokes_only_its_session(pool: PgPool) {
    let _case = CASE.lock().await;
    let time = time();
    account(&pool, "csrf@example.test", &time).await;
    let first = issue(&pool, "csrf@example.test", &time).await;
    let second = issue(&pool, "csrf@example.test", &time).await;
    let id = verifier(&first.session.token);
    let digest: Vec<u8> = Sha256::new()
        .chain_update(b"piwas:customer-session:csrf:v1\0")
        .chain_update(first.csrf.expose_bytes())
        .finalize()
        .to_vec();
    let row: (Vec<u8>, bool) = sqlx::query_as("SELECT csrf_digest,created_xid=(SELECT created_xid FROM customer_sessions WHERE verifier=$1) FROM customer_session_csrf WHERE session_verifier=$1")
        .bind(&id).fetch_one(&pool).await.expect("binding");
    assert!(row.0 == digest && row.1, "exact session and digest binding");
    assert!(
        row.0 != first.csrf.expose_bytes(),
        "raw synchronizer is not stored"
    );
    assert!(
        first.csrf.expose_bytes() != first.session.token.expose_bytes(),
        "session and synchronizer differ"
    );
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &first.session.token, &second.csrf, &time).await,
        Err(SessionError::Denied)
    ));
    let wrong = CustomerCsrfToken::from_bytes([3; 32]);
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &first.session.token, &wrong, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(
        sqlx::query("UPDATE customer_session_csrf SET csrf_digest=$2 WHERE session_verifier=$1")
            .bind(&id)
            .bind([0_u8; 32].as_slice())
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM customer_session_csrf WHERE session_verifier=$1")
            .bind(&id)
            .execute(&pool)
            .await
            .is_err()
    );
    customer_sessions::authenticate(&pool, &first.session.token, &time)
        .await
        .expect("still live");
    customer_sessions::logout_with_csrf(&pool, &first.session.token, &first.csrf, &time)
        .await
        .expect("protected logout");
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &first.session.token, &first.csrf, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::authenticate(&pool, &first.session.token, &time).await,
        Err(SessionError::Denied)
    ));
    customer_sessions::authenticate(&pool, &second.session.token, &time)
        .await
        .expect("other remains live");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotation_and_expiry_never_transfer_proof_authority(pool: PgPool) {
    let _case = CASE.lock().await;
    let time = time();
    account(&pool, "rotation@example.test", &time).await;
    let old = issue(&pool, "rotation@example.test", &time).await;
    let successor = customer_sessions::rotate(&pool, &old.session.token, &time)
        .await
        .expect("rotate");
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &old.session.token, &old.csrf, &time).await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &successor.token, &old.csrf, &time).await,
        Err(SessionError::Denied)
    ));
    customer_sessions::logout(&pool, &successor.token, &time)
        .await
        .expect("legacy logout remains supported");
    let expiring = issue(&pool, "rotation@example.test", &time).await;
    time.0.store(44_200, Ordering::SeqCst);
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &expiring.session.token, &expiring.csrf, &time)
            .await,
        Err(SessionError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_protected_logouts_have_one_winner(pool: PgPool) {
    let _case = CASE.lock().await;
    let time = time();
    account(&pool, "race@example.test", &time).await;
    let issued = issue(&pool, "race@example.test", &time).await;
    let (first, second) = tokio::join!(
        customer_sessions::logout_with_csrf(&pool, &issued.session.token, &issued.csrf, &time),
        customer_sessions::logout_with_csrf(&pool, &issued.session.token, &issued.csrf, &time)
    );
    assert!(matches!(
        (&first, &second),
        (Ok(()), Err(SessionError::Denied)) | (Err(SessionError::Denied), Ok(()))
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM customer_sessions WHERE revocation_kind='logout'")
            .fetch_one(&pool)
            .await
            .expect("logout count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = false)]
async fn legacy_unbound_sessions_upgrade_without_acquiring_csrf_authority(pool: PgPool) {
    let _case = CASE.lock().await;
    MIGRATOR.run_to(61, &pool).await.expect("previous schema");
    let time = time();
    account(&pool, "legacy@example.test", &time).await;
    let old = admitted_login::login(&pool, "legacy@example.test", "long fixture password", &time)
        .await
        .expect("legacy login");
    MIGRATOR.run_to(62, &pool).await.expect("upgrade");
    let id = verifier(&old.token);
    assert!(sqlx::query("INSERT INTO customer_session_csrf(session_verifier,csrf_digest,created_xid) VALUES($1,$2,NULL)").bind(&id).bind([5_u8;32].as_slice()).execute(&pool).await.is_err());
    let proof = CustomerCsrfToken::from_bytes([5; 32]);
    assert!(matches!(
        customer_sessions::logout_with_csrf(&pool, &old.token, &proof, &time).await,
        Err(SessionError::Denied)
    ));
    customer_sessions::logout(&pool, &old.token, &time)
        .await
        .expect("legacy operation remains supported");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_binding_rolls_back_session_issuance(pool: PgPool) {
    let _case = CASE.lock().await;
    let time = time();
    account(&pool, "suppressed@example.test", &time).await;
    sqlx::query("CREATE FUNCTION suppress_csrf_binding() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("suppression function");
    sqlx::query("CREATE TRIGGER z_suppress_csrf BEFORE INSERT ON customer_session_csrf FOR EACH ROW EXECUTE FUNCTION suppress_csrf_binding()")
        .execute(&pool).await.expect("suppression trigger");
    let key = AdmissionKey::new(1, [7; 32]).expect("key");
    let limits = AdmissionLimits::new(1000, 100).expect("limits");
    assert!(matches!(
        customer_sessions::login_with_csrf(
            &pool,
            "suppressed@example.test",
            "long fixture password",
            &time,
            &key,
            limits
        )
        .await,
        Err(SessionError::Unavailable)
    ));
    let counts: (i64,i64,i32) = sqlx::query_as("SELECT (SELECT count(*) FROM customer_sessions),(SELECT count(*) FROM customer_session_csrf),(SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=1)")
        .fetch_one(&pool).await.expect("session and admission counts");
    assert_eq!(counts, (0, 0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_during_account_lock_wait_preserves_session(pool: PgPool) {
    let _case = CASE.lock().await;
    let time = std::sync::Arc::new(time());
    account(&pool, "wait@example.test", &time).await;
    let issued = std::sync::Arc::new(issue(&pool, "wait@example.test", &time).await);
    let mut lock = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE id=$1 FOR UPDATE")
        .bind(issued.session.identity.account_id)
        .fetch_one(&mut *lock)
        .await
        .expect("hold account");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_issued = issued.clone();
    let worker = tokio::spawn(async move {
        customer_sessions::logout_with_csrf(
            &worker_pool,
            &worker_issued.session.token,
            &worker_issued.csrf,
            worker_time.as_ref(),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%verified_customer_accounts%FOR UPDATE%')")
                .fetch_one(&pool).await.expect("waiter state");
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("real account lock wait");
    time.0.store(44_200, Ordering::SeqCst);
    lock.rollback().await.expect("release account");
    assert!(matches!(
        worker.await.expect("worker"),
        Err(SessionError::Denied)
    ));
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM customer_sessions WHERE verifier=$1")
            .bind(verifier(&issued.session.token))
            .fetch_one(&pool)
            .await
            .expect("session state");
    assert_eq!(revoked, None);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotation_and_protected_logout_serialize_on_the_same_session(pool: PgPool) {
    let _case = CASE.lock().await;
    let time = time();
    account(&pool, "rotate-race@example.test", &time).await;
    let issued = issue(&pool, "rotate-race@example.test", &time).await;
    let (logout, rotation) = tokio::join!(
        customer_sessions::logout_with_csrf(&pool, &issued.session.token, &issued.csrf, &time),
        customer_sessions::rotate(&pool, &issued.session.token, &time)
    );
    match (logout, rotation) {
        (Ok(()), Err(SessionError::Denied)) => {}
        (Err(SessionError::Denied), Ok(successor)) => {
            assert!(matches!(
                customer_sessions::logout_with_csrf(&pool, &successor.token, &issued.csrf, &time)
                    .await,
                Err(SessionError::Denied)
            ));
            customer_sessions::authenticate(&pool, &successor.token, &time)
                .await
                .expect("successor remains live");
        }
        _ => panic!("exactly one session transition must win"),
    }
    assert!(matches!(
        customer_sessions::authenticate(&pool, &issued.session.token, &time).await,
        Err(SessionError::Denied)
    ));
}
