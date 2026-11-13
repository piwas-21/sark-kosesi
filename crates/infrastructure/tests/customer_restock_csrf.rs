//! Transaction-bound synchronizer controls over isolated PostgreSQL.
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code, unused_imports)]
mod catalogue {
    include!("../../../tests/fixtures/approved_catalogue.rs");
}
use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use catalogue::*;
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_restock::{self, RestockError},
    customer_sessions::{self, CustomerCsrfToken, CustomerSessionToken, IssuedCsrfSession},
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

async fn account(pool: &PgPool, email: &str) -> i64 {
    let clock = FixedClock(1100);
    let proof = email_verification::issue(pool, email, &clock)
        .await
        .expect("email proof");
    let id = establish(pool, email, &proof, &clock)
        .await
        .expect("account")
        .value();
    let proof = customer_passwords::issue(pool, email, &clock)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, email, &proof, "correct horse battery staple", &clock)
        .await
        .expect("password");
    id
}
async fn issue(pool: &PgPool, email: &str) -> IssuedCsrfSession {
    customer_sessions::login_with_csrf(
        pool,
        email,
        "correct horse battery staple",
        &FixedClock(1100),
        &AdmissionKey::new(1, [7; 32]).expect("key"),
        AdmissionLimits::new(1000, 100).expect("limits"),
    )
    .await
    .expect("issue")
}
async fn activate(pool: &PgPool) {
    approve(pool).await;
    record_publication(
        pool,
        &author(),
        &FixedClock(1100),
        request("activate", &choice()),
    )
    .await
    .expect("publication");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_session_binding_and_hidden_stale_semantics(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let id = account(&pool, "bound@example.test").await;
    let first = issue(&pool, "bound@example.test").await;
    let second = issue(&pool, "bound@example.test").await;
    let clock = FixedClock(1100);
    activate(&pool).await;
    let wrong = CustomerCsrfToken::from_bytes([3; 32]);
    for csrf in [&second.csrf, &wrong] {
        assert_eq!(
            customer_restock::subscribe_with_csrf(
                &pool,
                &first.session.token,
                csrf,
                &clock,
                Locale::Nl,
                "SET-FILLED",
                0
            )
            .await,
            Err(RestockError::Denied)
        );
    }
    let buckets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM customer_restock_buckets WHERE account_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("buckets");
    assert_eq!(buckets, 0);
    assert_eq!(
        customer_restock::subscribe_with_csrf(
            &pool,
            &first.session.token,
            &first.csrf,
            &clock,
            Locale::Nl,
            "SET-FILLED",
            0
        )
        .await,
        Ok(1)
    );
    assert_eq!(
        customer_restock::subscribe_with_csrf(
            &pool,
            &first.session.token,
            &first.csrf,
            &clock,
            Locale::Nl,
            "SET-FILLED",
            0
        )
        .await,
        Ok(1)
    );
    assert_eq!(
        customer_restock::subscribe_with_csrf(
            &pool,
            &first.session.token,
            &second.csrf,
            &clock,
            Locale::Nl,
            "SET-FILLED",
            0
        )
        .await,
        Err(RestockError::Denied)
    );
    assert_eq!(
        customer_restock::unsubscribe_with_csrf(
            &pool,
            &first.session.token,
            &first.csrf,
            &clock,
            "SET-FILLED",
            0
        )
        .await,
        Err(RestockError::Conflict)
    );
    let mut withdrawn = request("withdraw", &PublicationChoice::Withdraw {});
    withdrawn.expected_revision = 1;
    record_publication(&pool, &author(), &clock, withdrawn)
        .await
        .expect("withdraw");
    assert_eq!(
        customer_restock::unsubscribe_with_csrf(
            &pool,
            &first.session.token,
            &first.csrf,
            &clock,
            "SET-FILLED",
            1
        )
        .await,
        Ok(2)
    );
    assert_eq!(
        customer_restock::unsubscribe_with_csrf(
            &pool,
            &first.session.token,
            &first.csrf,
            &clock,
            "SET-FILLED",
            0
        )
        .await,
        Ok(2)
    );
    customer_sessions::logout_with_csrf(&pool, &first.session.token, &first.csrf, &clock)
        .await
        .expect("logout");
    assert_eq!(
        customer_restock::unsubscribe_with_csrf(
            &pool,
            &first.session.token,
            &first.csrf,
            &clock,
            "SET-FILLED",
            2
        )
        .await,
        Err(RestockError::Denied)
    );
    let rotated = customer_sessions::rotate(&pool, &second.session.token, &clock)
        .await
        .expect("rotate");
    assert_eq!(
        customer_restock::unsubscribe_with_csrf(
            &pool,
            &rotated.token,
            &second.csrf,
            &clock,
            "SET-FILLED",
            2
        )
        .await,
        Err(RestockError::Denied)
    );
}
struct MovingClock(AtomicU64);
impl Clock for MovingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_during_bucket_wait_rolls_back_with_bound_proof(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let id = account(&pool, "expiry-bound@example.test").await;
    let issued = issue(&pool, "expiry-bound@example.test").await;
    activate(&pool).await;
    assert_eq!(
        customer_restock::subscribe_with_csrf(
            &pool,
            &issued.session.token,
            &issued.csrf,
            &FixedClock(1100),
            Locale::Nl,
            "SET-FILLED",
            0
        )
        .await,
        Ok(1)
    );
    let mut held = pool.begin().await.expect("hold bucket");
    sqlx::query("SELECT revision FROM customer_restock_buckets WHERE account_id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .expect("claim");
    let clock = Arc::new(MovingClock(AtomicU64::new(1100)));
    let task_clock = clock.clone();
    let task_pool = pool.clone();
    let token = CustomerSessionToken::from_bytes(*issued.session.token.expose_bytes());
    let csrf = CustomerCsrfToken::from_bytes(*issued.csrf.expose_bytes());
    let task = tokio::spawn(async move {
        customer_restock::unsubscribe_with_csrf(
            &task_pool,
            &token,
            &csrf,
            &*task_clock,
            "SET-FILLED",
            1,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revision FROM customer_restock_buckets%'").fetch_one(&pool).await.expect("waiter");
            if n == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("actual wait");
    clock.0.store(44300, Ordering::SeqCst);
    held.commit().await.expect("release");
    assert_eq!(task.await.expect("task"), Err(RestockError::Denied));
    let state: (i64,i64) = sqlx::query_as("SELECT revision,(SELECT count(*) FROM customer_restock_subscriptions WHERE account_id=$1) FROM customer_restock_buckets WHERE account_id=$1").bind(id).fetch_one(&pool).await.expect("state");
    assert_eq!(state, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logout_waits_for_held_session_through_bucket_commit(pool: PgPool) {
    let _fixture = media_seed(&pool).await;
    let id = account(&pool, "lock-bound@example.test").await;
    let issued = issue(&pool, "lock-bound@example.test").await;
    activate(&pool).await;
    assert_eq!(
        customer_restock::subscribe_with_csrf(
            &pool,
            &issued.session.token,
            &issued.csrf,
            &FixedClock(1100),
            Locale::Nl,
            "SET-FILLED",
            0
        )
        .await,
        Ok(1)
    );
    let mut held = pool.begin().await.expect("hold bucket");
    sqlx::query("SELECT revision FROM customer_restock_buckets WHERE account_id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .expect("claim");
    let task_pool = pool.clone();
    let token = CustomerSessionToken::from_bytes(*issued.session.token.expose_bytes());
    let csrf = CustomerCsrfToken::from_bytes(*issued.csrf.expose_bytes());
    let mutation = tokio::spawn(async move {
        customer_restock::unsubscribe_with_csrf(
            &task_pool,
            &token,
            &csrf,
            &FixedClock(1100),
            "SET-FILLED",
            1,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revision FROM customer_restock_buckets%'").fetch_one(&pool).await.expect("waiter");
            if n == 1 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("mutation waiting on bucket");
    let logout_pool = pool.clone();
    let token = CustomerSessionToken::from_bytes(*issued.session.token.expose_bytes());
    let csrf = CustomerCsrfToken::from_bytes(*issued.csrf.expose_bytes());
    let logout = tokio::spawn(async move {
        customer_sessions::logout_with_csrf(&logout_pool, &token, &csrf, &FixedClock(1100)).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT%customer%'").fetch_one(&pool).await.expect("session waiter");
            if n >= 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("logout waiting on claimed account/session");
    held.commit().await.expect("release bucket");
    assert_eq!(mutation.await.expect("mutation"), Ok(2));
    logout.await.expect("logout task").expect("logout");
    assert_eq!(
        customer_restock::unsubscribe_with_csrf(
            &pool,
            &issued.session.token,
            &issued.csrf,
            &FixedClock(1100),
            "SET-FILLED",
            2
        )
        .await,
        Err(RestockError::Denied)
    );
}
