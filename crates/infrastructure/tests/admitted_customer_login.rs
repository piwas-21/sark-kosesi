//! Login admission is committed before credential lookup or password work.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_passwords,
    customer_sessions::{self, SessionError},
    email_verification, verified_customer_accounts,
};
use sqlx::PgPool;

struct TestClock;
impl Clock for TestClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(1000)
    }
}

fn key() -> AdmissionKey {
    AdmissionKey::new(1, [7; 32]).expect("valid key")
}
fn limits(global: i32, target: i32) -> AdmissionLimits {
    AdmissionLimits::new(global, target).expect("valid limits")
}
async fn account(pool: &PgPool, email: &str, password: &str) -> i64 {
    let proof = email_verification::issue(pool, email, &TestClock)
        .await
        .expect("verification proof");
    let id = verified_customer_accounts::establish(pool, email, &proof, &TestClock)
        .await
        .expect("verified account")
        .value();
    let proof = customer_passwords::issue(pool, email, &TestClock)
        .await
        .expect("password proof");
    assert_eq!(
        customer_passwords::replace(pool, email, &proof, password, &TestClock)
            .await
            .expect("set password"),
        1
    );
    id
}
async fn counts(pool: &PgPool) -> (i32, Vec<i32>) {
    let global = sqlx::query_scalar(
        "SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=1",
    )
    .fetch_one(pool)
    .await
    .expect("global count");
    let targets = sqlx::query_scalar("SELECT admitted FROM customer_credential_admission_targets WHERE purpose=1 ORDER BY admitted")
        .fetch_all(pool).await.expect("target counts");
    (global, targets)
}
async fn sessions(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM customer_sessions")
        .fetch_one(pool)
        .await
        .expect("session count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn saturated_normalized_target_stops_before_lookup_and_failed_lookup_keeps_debit(
    pool: PgPool,
) {
    let id = account(&pool, "Case@EXAMPLE.COM", "correct-password-long").await;
    let key = key();
    let cap = limits(4, 1);
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "Case@example.com",
            "wrong-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, vec![1]));
    sqlx::query("ALTER TABLE customer_password_credentials RENAME TO unavailable_credentials")
        .execute(&pool)
        .await
        .expect("make credential lookup unavailable");
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "Case@EXAMPLE.COM",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Denied)
    ));
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "other@example.com",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Unavailable)
    ));
    assert_eq!(
        counts(&pool).await,
        (2, vec![1, 1]),
        "lookup failure retains its admission"
    );
    sqlx::query("ALTER TABLE unavailable_credentials RENAME TO customer_password_credentials")
        .execute(&pool)
        .await
        .expect("restore credential lookup");
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "other@EXAMPLE.COM",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Denied)
    ));
    assert_eq!(counts(&pool).await, (2, vec![1, 1]));
    assert_eq!(sessions(&pool).await, 0);
    let fresh = customer_sessions::login(
        &pool,
        "Case@example.com",
        "correct-password-long",
        &NextWindow,
        &key,
        cap,
    )
    .await
    .expect("healthy lookup after rollover");
    assert_eq!(
        (
            fresh.identity.account_id,
            fresh.identity.credential_revision
        ),
        (id, 1)
    );
}

struct NextWindow;
impl Clock for NextWindow {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(1020)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn known_wrong_and_unknown_consume_equal_budgets_and_sessions_bind_revision(pool: PgPool) {
    let id = account(&pool, "known@example.com", "correct-password-long").await;
    let key = key();
    let cap = limits(6, 2);
    for (email, password) in [
        ("known@example.com", "wrong-password-long"),
        ("unknown@example.com", "correct-password-long"),
    ] {
        assert!(matches!(
            customer_sessions::login(&pool, email, password, &TestClock, &key, cap).await,
            Err(SessionError::Denied)
        ));
    }
    assert_eq!(counts(&pool).await, (2, vec![1, 1]));
    assert_eq!(sessions(&pool).await, 0);
    let issued = customer_sessions::login(
        &pool,
        "known@EXAMPLE.COM",
        "correct-password-long",
        &TestClock,
        &key,
        cap,
    )
    .await
    .expect("correct login");
    assert_eq!(
        (
            issued.identity.account_id,
            issued.identity.credential_revision,
            issued.identity.expires_at
        ),
        (id, 1, 44200)
    );
    assert_eq!(counts(&pool).await, (3, vec![1, 2]));
    assert_eq!(
        customer_sessions::authenticate(&pool, &issued.token, &TestClock)
            .await
            .expect("live session")
            .credential_revision,
        1
    );
    let proof = customer_passwords::issue(&pool, "known@example.com", &TestClock)
        .await
        .expect("reset proof");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "known@example.com",
            &proof,
            "replacement-password-long",
            &TestClock
        )
        .await
        .expect("reset"),
        2
    );
    assert!(matches!(
        customer_sessions::authenticate(&pool, &issued.token, &TestClock).await,
        Err(SessionError::Denied)
    ));
    assert_eq!(sessions(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_or_broken_admission_denies_correct_credentials(pool: PgPool) {
    account(&pool, "valid@example.com", "correct-password-long").await;
    let key = key();
    let cap = limits(3, 2);
    sqlx::query("DELETE FROM customer_credential_admission_scopes WHERE purpose=1")
        .execute(&pool)
        .await
        .expect("remove inactive scope");
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "valid@example.com",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Unavailable)
    ));
    assert_eq!(sessions(&pool).await, 0);
    sqlx::query(
        "INSERT INTO customer_credential_admission_scopes (purpose, admitted) VALUES (1,0)",
    )
    .execute(&pool)
    .await
    .expect("restore inactive scope");
    sqlx::query(
        "ALTER TABLE customer_credential_admission_targets RENAME TO unavailable_admission_targets",
    )
    .execute(&pool)
    .await
    .expect("break admission store");
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "valid@example.com",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Unavailable)
    ));
    assert_eq!(sessions(&pool).await, 0);
    sqlx::query(
        "ALTER TABLE unavailable_admission_targets RENAME TO customer_credential_admission_targets",
    )
    .execute(&pool)
    .await
    .expect("restore admission store");
    assert_eq!(counts(&pool).await, (0, vec![]));
    customer_sessions::login(
        &pool,
        "valid@example.com",
        "correct-password-long",
        &TestClock,
        &key,
        cap,
    )
    .await
    .expect("healthy admission and credentials");
    assert_eq!(counts(&pool).await, (1, vec![1]));
    assert_eq!(sessions(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_or_modified_session_insert_rolls_back_without_refunding_admission(
    pool: PgPool,
) {
    account(&pool, "insert@example.com", "correct-password-long").await;
    let key = key();
    let cap = limits(5, 5);
    sqlx::query("CREATE FUNCTION suppress_session_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("suppression function");
    sqlx::query("CREATE TRIGGER z_suppress_session_insert BEFORE INSERT ON customer_sessions FOR EACH ROW EXECUTE FUNCTION suppress_session_insert()")
        .execute(&pool).await.expect("suppression trigger");
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "insert@example.com",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (1, vec![1]));
    assert_eq!(sessions(&pool).await, 0);
    sqlx::query("DROP TRIGGER z_suppress_session_insert ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove suppression");

    // Alphabetical trigger order puts this after the native guard's lifetime check.
    sqlx::query("CREATE FUNCTION alter_session_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.expires_at := NEW.expires_at + 1; RETURN NEW; END $$")
        .execute(&pool).await.expect("alter function");
    sqlx::query("CREATE TRIGGER z_alter_session_expiry BEFORE INSERT ON customer_sessions FOR EACH ROW EXECUTE FUNCTION alter_session_expiry()")
        .execute(&pool).await.expect("alter trigger");
    assert!(matches!(
        customer_sessions::login(
            &pool,
            "insert@example.com",
            "correct-password-long",
            &TestClock,
            &key,
            cap
        )
        .await,
        Err(SessionError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (2, vec![2]));
    assert_eq!(
        sessions(&pool).await,
        0,
        "readback mismatch rolls back the modified row"
    );
    sqlx::query("DROP TRIGGER z_alter_session_expiry ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove alteration");
    customer_sessions::login(
        &pool,
        "insert@example.com",
        "correct-password-long",
        &TestClock,
        &key,
        cap,
    )
    .await
    .expect("healthy issuance");
    assert_eq!(counts(&pool).await, (3, vec![3]));
    assert_eq!(sessions(&pool).await, 1);
}

struct MutableClock(std::sync::atomic::AtomicU64);
impl Clock for MutableClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_session_insert_rolls_back_without_refunding_admission(pool: PgPool) {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    account(&pool, "late@example.com", "correct-password-long").await;
    sqlx::query("CREATE FUNCTION hold_login_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(274912); RETURN NEW; END $$")
        .execute(&pool).await.expect("fence function");
    sqlx::query("CREATE TRIGGER hold_login_insert AFTER INSERT ON customer_sessions FOR EACH ROW EXECUTE FUNCTION hold_login_insert()")
        .execute(&pool).await.expect("fence trigger");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(274912)")
        .execute(&mut *blocker)
        .await
        .expect("hold fence");
    let time = Arc::new(MutableClock(AtomicU64::new(1000)));
    let worker_time = time.clone();
    let worker_pool = pool.clone();
    let task = tokio::spawn(async move {
        customer_sessions::login(
            &worker_pool,
            "late@example.com",
            "correct-password-long",
            worker_time.as_ref(),
            &key(),
            limits(3, 3),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND classid=0 AND objid=274912 AND NOT granted)")
                .fetch_one(&pool).await.expect("observe insert wait");
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("session insert reached fence");
    assert!(!task.is_finished());
    assert_eq!(counts(&pool).await, (1, vec![1]));
    assert_eq!(sessions(&pool).await, 0);
    time.0.store(44200, Ordering::SeqCst);
    blocker.commit().await.expect("release fence");
    assert!(matches!(
        task.await.expect("join"),
        Err(SessionError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, vec![1]));
    assert_eq!(sessions(&pool).await, 0, "expired issuance rolled back");
    sqlx::query("DROP TRIGGER hold_login_insert ON customer_sessions")
        .execute(&pool)
        .await
        .expect("remove fence");
    customer_sessions::login(
        &pool,
        "late@example.com",
        "correct-password-long",
        time.as_ref(),
        &key(),
        limits(3, 3),
    )
    .await
    .expect("healthy later login");
    assert_eq!(counts(&pool).await, (1, vec![1]));
    assert_eq!(sessions(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_password_length_is_denied_after_admission(pool: PgPool) {
    account(&pool, "length@example.com", "correct-password-long").await;
    let key = key();
    let cap = limits(3, 3);
    for password in ["short", &"x".repeat(257)] {
        assert!(matches!(
            customer_sessions::login(&pool, "length@example.com", password, &TestClock, &key, cap)
                .await,
            Err(SessionError::Denied)
        ));
    }
    assert_eq!(counts(&pool).await, (2, vec![2]));
    assert_eq!(sessions(&pool).await, 0);
    customer_sessions::login(
        &pool,
        "length@example.com",
        "correct-password-long",
        &TestClock,
        &key,
        cap,
    )
    .await
    .expect("valid password");
    assert_eq!(counts(&pool).await, (3, vec![3]));
    assert_eq!(sessions(&pool).await, 1);
}
