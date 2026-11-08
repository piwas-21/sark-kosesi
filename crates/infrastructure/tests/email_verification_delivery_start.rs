//! Private start custody over isolated PostgreSQL; no mail or provider I/O.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{
        self, VerificationError, VerificationToken,
        delivery::{
            VerificationDeliveryKey, issue_for_delivery, register_key,
            start::{ClaimOutcome, claim},
        },
    },
};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const EMAIL: &str = "buyer@example.test";
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn limits() -> AdmissionLimits {
    AdmissionLimits::new(1000, 100).expect("synthetic limits")
}
fn admission() -> AdmissionKey {
    AdmissionKey::new(1, [27; 32]).expect("synthetic admission key")
}
fn key() -> VerificationDeliveryKey {
    VerificationDeliveryKey::capture("key-one", [41; 32]).expect("synthetic delivery key")
}

async fn fixture(pool: &PgPool) -> (Arc<Time>, Arc<VerificationDeliveryKey>, [u8; 32]) {
    let time = Arc::new(Time(AtomicU64::new(1000)));
    let key = Arc::new(key());
    register_key(pool, time.as_ref(), &key)
        .await
        .expect("register synthetic key");
    let receipt = issue_for_delivery(pool, EMAIL, time.as_ref(), &admission(), limits(), &key)
        .await
        .expect("genuine envelope");
    (time, key, receipt.verifier)
}

// Test-only opening recovers the synthetic bearer for the real consume API.
// It neither prints nor persists the bearer and is not production transport code.
async fn open_test_bearer(pool: &PgPool, verifier: &[u8; 32]) -> VerificationToken {
    let (counter, mut ciphertext, created, expires): (i64, Vec<u8>, i64, i64) = sqlx::query_as("SELECT e.nonce_counter,e.ciphertext,c.created_at,c.expires_at FROM email_verification_delivery_envelopes e JOIN email_verification_challenges c USING(verifier) WHERE e.verifier=$1").bind(verifier.as_slice()).fetch_one(pool).await.expect("test envelope");
    let created_bytes = created.to_be_bytes();
    let expires_bytes = expires.to_be_bytes();
    let counter_bytes = counter.to_be_bytes();
    let version = 1_i16.to_be_bytes();
    let mut aad = Vec::new();
    for value in [
        b"piwas:verification-delivery:aad:v1".as_slice(),
        b"email-verification-v1",
        EMAIL.as_bytes(),
        verifier.as_slice(),
        created_bytes.as_slice(),
        expires_bytes.as_slice(),
        version.as_slice(),
        b"key-one",
        counter_bytes.as_slice(),
    ] {
        aad.extend_from_slice(
            &u32::try_from(value.len())
                .expect("bounded field")
                .to_be_bytes(),
        );
        aad.extend_from_slice(value);
    }
    let mut nonce = [0_u8; 12];
    nonce[..4].copy_from_slice(b"PV01");
    nonce[4..].copy_from_slice(&u64::try_from(counter).expect("counter").to_be_bytes());
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &[41; 32]).expect("test key"));
    let bearer = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut ciphertext,
        )
        .expect("authenticated synthetic envelope");
    let verifier_check = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(&*bearer)
        .finalize();
    assert_eq!(&verifier_check[..], verifier.as_slice());
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(bearer);
    VerificationToken::from_bytes(bytes)
}

async fn starts(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM email_verification_delivery_starts")
        .fetch_one(pool)
        .await
        .expect("start count")
}
async fn wait_locks(pool: &PgPool, minimum: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'").fetch_one(pool).await.expect("observe real lock wait");
            if count >= minimum { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("contenders reached lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn two_simultaneous_claims_have_exactly_one_durable_start(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let mut gate = pool.begin().await.expect("challenge gate");
    sqlx::query("SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE")
        .bind(verifier.as_slice())
        .execute(&mut *gate)
        .await
        .expect("hold source");
    let first = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        async move { claim(&pool, &verifier, time.as_ref(), &key).await }
    });
    wait_locks(&pool, 1).await;
    let second = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        async move { claim(&pool, &verifier, time.as_ref(), &key).await }
    });
    wait_locks(&pool, 2).await;
    gate.rollback().await.expect("release queued claims");
    assert_eq!(first.await.expect("first task"), Ok(ClaimOutcome::Started));
    assert_eq!(
        second.await.expect("second task"),
        Ok(ClaimOutcome::AlreadyStarted)
    );
    assert_eq!(starts(&pool).await, 1);
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::AlreadyStarted)
    );
    let stats: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM email_verification_challenges WHERE consumed_at IS NOT NULL),(SELECT count(*) FROM email_verification_delivery_envelopes),(SELECT count(*) FROM verified_customer_accounts)").fetch_one(&pool).await.expect("no verification authority");
    assert_eq!(stats, (0, 1, 0));
}

async fn race_consume_and_claim(pool: PgPool, consume_first: bool) {
    let (time, key, verifier) = fixture(&pool).await;
    let bearer = open_test_bearer(&pool, &verifier).await;
    let mut gate = pool.begin().await.expect("source gate");
    sqlx::query("SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE")
        .bind(verifier.as_slice())
        .execute(&mut *gate)
        .await
        .expect("hold source");
    let do_claim = || {
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        tokio::spawn(async move { claim(&pool, &verifier, time.as_ref(), &key).await })
    };
    let do_consume = || {
        let pool = pool.clone();
        let time = time.clone();
        tokio::spawn(async move {
            email_verification::consume(
                &pool,
                EMAIL,
                &bearer,
                time.as_ref(),
                &admission(),
                limits(),
            )
            .await
        })
    };
    let (claiming, consuming) = if consume_first {
        let first = do_consume();
        wait_locks(&pool, 1).await;
        let second = do_claim();
        wait_locks(&pool, 2).await;
        (second, first)
    } else {
        let first = do_claim();
        wait_locks(&pool, 1).await;
        let second = do_consume();
        wait_locks(&pool, 2).await;
        (first, second)
    };
    gate.rollback().await.expect("release contenders");
    let claimed = claiming.await.expect("claim task");
    assert_eq!(consuming.await.expect("consume task"), Ok(()));
    if consume_first {
        assert_eq!(claimed, Err(VerificationError::Denied));
        assert_eq!(starts(&pool).await, 0);
    } else {
        assert_eq!(claimed, Ok(ClaimOutcome::Started));
        assert_eq!(starts(&pool).await, 1);
    }
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Err(VerificationError::Denied),
        "consumption prevents later claims and replay"
    );
    let verified_accounts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM verified_customer_accounts")
            .fetch_one(&pool)
            .await
            .expect("no account authority");
    assert_eq!(
        verified_accounts, 0,
        "standalone proof consumption is not account establishment"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn consumption_first_denies_a_waiting_start(pool: PgPool) {
    race_consume_and_claim(pool, true).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn start_first_then_consumption_stays_a_start_only(pool: PgPool) {
    race_consume_and_claim(pool, false).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn wrong_key_missing_direct_and_expired_challenges_never_create_a_start(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let wrong_id = VerificationDeliveryKey::capture("other", [42; 32]).expect("other key");
    let wrong_material =
        VerificationDeliveryKey::capture("key-one", [42; 32]).expect("wrong material");
    assert_eq!(
        claim(&pool, &[9; 32], time.as_ref(), &key).await,
        Err(VerificationError::Denied)
    );
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &wrong_id).await,
        Err(VerificationError::Denied)
    );
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &wrong_material).await,
        Err(VerificationError::Denied)
    );
    let direct = email_verification::issue(
        &pool,
        "direct@example.test",
        time.as_ref(),
        &admission(),
        limits(),
    )
    .await
    .expect("legacy direct proof");
    let digest: [u8; 32] = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(direct.expose_bytes())
        .finalize()
        .into();
    assert_eq!(
        claim(&pool, &digest, time.as_ref(), &key).await,
        Err(VerificationError::Denied)
    );
    time.0.store(1900, Ordering::SeqCst);
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Err(VerificationError::Denied)
    );
    assert_eq!(starts(&pool).await, 0);
    time.0.store(1000, Ordering::SeqCst);
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn final_expiry_and_task_abort_after_start_write_roll_back(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    pool.execute("CREATE FUNCTION test_delivery_start_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876980,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_delivery_start_wait AFTER INSERT ON email_verification_delivery_starts FOR EACH ROW EXECUTE FUNCTION test_delivery_start_wait()").await.expect("actual start-write gate");
    let mut gate = pool.begin().await.expect("expiry gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876980,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let work = claim(&pool, &verifier, time.as_ref(), &key);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO email_verification_delivery_starts%')").fetch_one(&pool).await.expect("actual start-write wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("start reached write gate");
        time.0.store(1900, Ordering::SeqCst);
        gate.rollback().await.expect("release expiry gate");
    };
    let (result, ()) = tokio::join!(work, release);
    assert_eq!(result, Err(VerificationError::Denied));
    assert_eq!(starts(&pool).await, 0);
    time.0.store(1000, Ordering::SeqCst);
    let mut gate = pool.begin().await.expect("abort gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876980,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let mut observer = pool.acquire().await.expect("independent observer");
    let work = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        async move { claim(&pool, &verifier, time.as_ref(), &key).await }
    });
    let pid: i32 = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO email_verification_delivery_starts%' LIMIT 1").fetch_optional(&mut *observer).await.expect("actual start-write wait");
            if let Some(pid) = pid { break pid; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("start reached abort gate");
    work.abort();
    assert!(matches!(work.await, Err(error) if error.is_cancelled()));
    gate.rollback().await.expect("release abort gate");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let cleaned: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND (state IS DISTINCT FROM 'idle' OR xact_start IS NOT NULL))").bind(pid).fetch_one(&mut *observer).await.expect("observe rollback cleanup");
            if cleaned { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("aborted transaction cleaned up");
    drop(observer);
    assert_eq!(starts(&pool).await, 0);
    pool.execute("DROP TRIGGER z_test_delivery_start_wait ON email_verification_delivery_starts")
        .await
        .expect("remove gate");
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started)
    );
    assert_eq!(starts(&pool).await, 1);
}

fn native_error(error: sqlx::Error, message: &str) {
    let sqlx::Error::Database(error) = error else {
        panic!("expected native database rejection")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), message);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_start_is_durable_and_early_flush_cannot_consume_its_source(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let bearer = open_test_bearer(&pool, &verifier).await;
    let insert = "INSERT INTO email_verification_delivery_starts(verifier,key_id,nonce_counter,key_fingerprint,started_at) SELECT e.verifier,e.key_id,e.nonce_counter,k.key_fingerprint,1000 FROM email_verification_delivery_envelopes e JOIN email_verification_delivery_keys k USING(key_id) WHERE e.verifier=$1";
    let mut tx = pool.begin().await.expect("repeatable-read transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .expect("set isolation");
    let error = sqlx::query(insert)
        .bind(verifier.as_slice())
        .execute(&mut *tx)
        .await
        .expect_err("wrong isolation");
    native_error(error, "Verification delivery start requires read committed");
    tx.rollback().await.expect("rollback");
    let async_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                connection.execute("SET synchronous_commit='off'").await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .expect("owned async-default connection");
    let error = sqlx::query(insert)
        .bind(verifier.as_slice())
        .execute(&async_pool)
        .await
        .expect_err("asynchronous native start");
    native_error(error, "Verification delivery start requires durable commit");
    let mut tx = pool.begin().await.expect("early flush transaction");
    sqlx::query(insert)
        .bind(verifier.as_slice())
        .execute(&mut *tx)
        .await
        .expect("valid native start");
    sqlx::query("SET CONSTRAINTS require_email_verification_delivery_start IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("early source check");
    let error =
        sqlx::query("UPDATE email_verification_challenges SET consumed_at=1000 WHERE verifier=$1")
            .bind(verifier.as_slice())
            .execute(&mut *tx)
            .await
            .expect_err("same-transaction consumption after early flush");
    native_error(
        error,
        "Verification delivery start and challenge consumption require separate transactions",
    );
    tx.rollback()
        .await
        .expect("roll back start and consumption");
    assert_eq!(starts(&pool).await, 0);
    assert_eq!(
        claim(&async_pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started),
        "claim forces a durable local transaction"
    );
    let configured: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&async_pool)
        .await
        .expect("restored connection default");
    assert_eq!(configured, "off");
    assert_eq!(
        claim(&async_pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::AlreadyStarted)
    );
    for statement in [
        "UPDATE email_verification_delivery_starts SET started_at=started_at",
        "DELETE FROM email_verification_delivery_starts",
    ] {
        assert!(
            async_pool.execute(statement).await.is_err(),
            "start journal is immutable"
        );
    }
    email_verification::consume(
        &async_pool,
        EMAIL,
        &bearer,
        time.as_ref(),
        &admission(),
        limits(),
    )
    .await
    .expect("later consumption is independent");
    assert_eq!(
        claim(&async_pool, &verifier, time.as_ref(), &key).await,
        Err(VerificationError::Denied)
    );
    assert_eq!(starts(&pool).await, 1);
    async_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_start_write_rolls_back_and_replay_rejects_an_earlier_clock(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    time.0.store(1001, Ordering::SeqCst);
    // The forged timestamp remains within the live challenge window and is not
    // ahead of the clock, so only the exact post-insert readback rejects it.
    pool.execute("CREATE FUNCTION test_change_delivery_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.started_at=1000; RETURN NEW; END; $$; CREATE TRIGGER z_test_change_delivery_start BEFORE INSERT ON email_verification_delivery_starts FOR EACH ROW EXECUTE FUNCTION test_change_delivery_start()").await.expect("isolated post-guard write fault");
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Err(VerificationError::Unavailable)
    );
    assert_eq!(starts(&pool).await, 0, "altered start rolls back");
    let custody: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM email_verification_delivery_nonces),(SELECT count(*) FROM email_verification_challenges),(SELECT count(*) FROM email_verification_delivery_envelopes)").fetch_one(&pool).await.expect("original custody");
    assert_eq!(custody, (1, 1, 1));
    pool.execute("DROP TRIGGER z_test_change_delivery_start ON email_verification_delivery_starts")
        .await
        .expect("remove isolated write fault");
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started)
    );
    time.0.store(1000, Ordering::SeqCst);
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Err(VerificationError::Unavailable),
        "replay cannot predate the durable start"
    );
    assert_eq!(starts(&pool).await, 1);
    time.0.store(1001, Ordering::SeqCst);
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::AlreadyStarted)
    );
}
