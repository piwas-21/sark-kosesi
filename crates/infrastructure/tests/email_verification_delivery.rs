//! Isolated verification-delivery custody. Synthetic keys, no transport or provider.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{
        self, VerificationError,
        delivery::{VerificationDeliveryKey, issue_for_delivery, register_key},
    },
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};

struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn admission() -> AdmissionKey {
    AdmissionKey::new(1, [27; 32]).expect("synthetic admission key")
}
fn limits() -> AdmissionLimits {
    AdmissionLimits::new(1000, 100).expect("synthetic limits")
}
fn delivery() -> VerificationDeliveryKey {
    VerificationDeliveryKey::capture("key-one", [41; 32]).expect("synthetic delivery key")
}
async fn counts(pool: &PgPool) -> (i64, i64, i64, i32) {
    sqlx::query_as("SELECT (SELECT count(*) FROM email_verification_delivery_nonces),(SELECT count(*) FROM email_verification_challenges),(SELECT count(*) FROM email_verification_delivery_envelopes),(SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=2)").fetch_one(pool).await.expect("nonce, challenge, envelope, quota")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn real_issuance_uses_one_debit_and_stores_only_an_encrypted_bearer(pool: PgPool) {
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let key = delivery();
    register_key(&pool, &clock, &key)
        .await
        .expect("register key");
    register_key(&pool, &clock, &key)
        .await
        .expect("idempotent exact key registration");
    let different_id =
        VerificationDeliveryKey::capture("other-id", [41; 32]).expect("same synthetic bytes");
    assert_eq!(
        register_key(&pool, &clock, &different_id).await,
        Err(VerificationError::Unavailable),
        "same key bytes cannot reset nonce under a different ID"
    );
    let same_id =
        VerificationDeliveryKey::capture("key-one", [42; 32]).expect("other synthetic key");
    assert_eq!(
        register_key(&pool, &clock, &same_id).await,
        Err(VerificationError::Unavailable),
        "key ID cannot change material"
    );
    let same_as_admission =
        VerificationDeliveryKey::capture("cross-purpose", [27; 32]).expect("synthetic key object");
    assert_eq!(
        issue_for_delivery(
            &pool,
            "cross@example.test",
            &clock,
            &admission,
            limits(),
            &same_as_admission
        )
        .await
        .err(),
        Some(VerificationError::Unavailable)
    );
    assert_eq!(
        counts(&pool).await,
        (0, 0, 0, 0),
        "cross-purpose key rejected before quota or nonce"
    );
    let receipt = issue_for_delivery(
        &pool,
        "Case+tag@EXAMPLE.COM",
        &clock,
        &admission,
        limits(),
        &key,
    )
    .await
    .expect("durable encrypted delivery");
    assert_eq!((receipt.created_at, receipt.expires_at), (1000, 1900));
    assert_eq!(counts(&pool).await, (1, 1, 1, 1));
    let row: (Vec<u8>, i64, i16, Vec<u8>, String) = sqlx::query_as("SELECT verifier,nonce_counter,envelope_version,ciphertext,to_jsonb(e)::text FROM email_verification_delivery_envelopes e").fetch_one(&pool).await.expect("encrypted envelope");
    assert_eq!(row.0, receipt.verifier);
    assert_eq!(row.2, 1);
    assert_eq!(
        row.3.len(),
        48,
        "32-byte encrypted bearer and 16-byte GCM tag"
    );
    assert!(
        !row.4.contains("Case+tag@example.com"),
        "envelope contains no duplicate mailbox"
    );
    assert!(
        !row.3
            .windows(b"Case+tag@example.com".len())
            .any(|part| part == b"Case+tag@example.com"),
        "ciphertext contains no plaintext mailbox"
    );
    assert!(!row.4.contains("bearer"), "no plaintext token column");
    // Independent test-only decryption proves the row is ciphertext of exactly
    // the token bound to the verifier, without persisting the recovered bearer.
    use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
    use sha2::{Digest, Sha256};
    let created = receipt.created_at.to_be_bytes();
    let expires = receipt.expires_at.to_be_bytes();
    let version = 1_i16.to_be_bytes();
    let counter = row.1.to_be_bytes();
    let mut aad = Vec::new();
    for field in [
        b"piwas:verification-delivery:aad:v1".as_slice(),
        b"email-verification-v1",
        b"Case+tag@example.com",
        row.0.as_slice(),
        created.as_slice(),
        expires.as_slice(),
        version.as_slice(),
        b"key-one",
        counter.as_slice(),
    ] {
        aad.extend_from_slice(
            &u32::try_from(field.len())
                .expect("bounded field")
                .to_be_bytes(),
        );
        aad.extend_from_slice(field);
    }
    let mut nonce = [0_u8; 12];
    nonce[..4].copy_from_slice(b"PV01");
    nonce[4..].copy_from_slice(
        &u64::try_from(row.1)
            .expect("positive counter")
            .to_be_bytes(),
    );
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &[41; 32]).expect("test key"));
    let mut encrypted = row.3.clone();
    let bearer = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut encrypted,
        )
        .expect("exact authenticated envelope");
    assert_eq!(bearer.len(), 32);
    let verifier = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(&*bearer)
        .finalize();
    assert_eq!(&verifier[..], receipt.verifier.as_slice());
    assert!(
        !row.3.windows(bearer.len()).any(|part| part == &*bearer),
        "ciphertext contains no raw bearer"
    );
    let columns: Vec<String> = sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='email_verification_delivery_envelopes' ORDER BY ordinal_position").fetch_all(&pool).await.expect("envelope shape");
    assert_eq!(
        columns,
        [
            "verifier",
            "key_id",
            "nonce_counter",
            "envelope_version",
            "ciphertext",
            "transaction_id"
        ]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_issuance_uses_one_unique_durable_nonce_per_challenge(pool: PgPool) {
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let key = delivery();
    register_key(&pool, &clock, &key)
        .await
        .expect("register key");
    let (a, b, c, d) = tokio::join!(
        issue_for_delivery(
            &pool,
            "one@example.test",
            &clock,
            &admission,
            limits(),
            &key
        ),
        issue_for_delivery(
            &pool,
            "two@example.test",
            &clock,
            &admission,
            limits(),
            &key
        ),
        issue_for_delivery(
            &pool,
            "three@example.test",
            &clock,
            &admission,
            limits(),
            &key
        ),
        issue_for_delivery(
            &pool,
            "four@example.test",
            &clock,
            &admission,
            limits(),
            &key
        ),
    );
    for result in [a, b, c, d] {
        assert!(result.is_ok(), "distinct admitted challenges succeed");
    }
    let nonces: Vec<i64> = sqlx::query_scalar(
        "SELECT nonce_counter FROM email_verification_delivery_nonces ORDER BY nonce_counter",
    )
    .fetch_all(&pool)
    .await
    .expect("durable unique counters");
    assert_eq!(
        nonces,
        vec![1, 2, 3, 4],
        "consecutive serialized counter claims"
    );
    let matched: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_delivery_envelopes e JOIN email_verification_delivery_nonces n USING(key_id,nonce_counter,verifier)").fetch_one(&pool).await.expect("exact nonce linkage");
    assert_eq!(matched, 4);
    assert_eq!(counts(&pool).await, (4, 4, 4, 4));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_challenge_or_envelope_burns_nonce_but_rolls_back_both_writes(pool: PgPool) {
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let key = delivery();
    register_key(&pool, &clock, &key)
        .await
        .expect("register key");
    pool.execute("CREATE FUNCTION test_suppress_verification_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$").await.expect("isolated write fault");
    for (index, (add, remove)) in [
        (
            "CREATE TRIGGER z_test_suppress_verification_write BEFORE INSERT ON email_verification_challenges FOR EACH ROW EXECUTE FUNCTION test_suppress_verification_write()",
            "DROP TRIGGER z_test_suppress_verification_write ON email_verification_challenges",
        ),
        (
            "CREATE TRIGGER z_test_suppress_verification_write BEFORE INSERT ON email_verification_delivery_envelopes FOR EACH ROW EXECUTE FUNCTION test_suppress_verification_write()",
            "DROP TRIGGER z_test_suppress_verification_write ON email_verification_delivery_envelopes",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        pool.execute(add).await.expect("install fault");
        assert_eq!(
            issue_for_delivery(
                &pool,
                "buyer@example.test",
                &clock,
                &admission,
                limits(),
                &key
            )
            .await
            .err(),
            Some(VerificationError::Unavailable)
        );
        let burned = i64::try_from(index + 1).expect("small index");
        assert_eq!(
            counts(&pool).await,
            (burned, 0, 0, i32::try_from(burned).expect("small count"))
        );
        pool.execute(remove).await.expect("remove fault");
    }
    issue_for_delivery(
        &pool,
        "buyer@example.test",
        &clock,
        &admission,
        limits(),
        &key,
    )
    .await
    .expect("same identity retry succeeds");
    assert_eq!(counts(&pool).await, (3, 1, 1, 3));
    let counters: Vec<i64> = sqlx::query_scalar(
        "SELECT nonce_counter FROM email_verification_delivery_nonces ORDER BY nonce_counter",
    )
    .fetch_all(&pool)
    .await
    .expect("burned nonce sequence");
    assert_eq!(counters, vec![1, 2, 3]);
    let used: i64 =
        sqlx::query_scalar("SELECT nonce_counter FROM email_verification_delivery_envelopes")
            .fetch_one(&pool)
            .await
            .expect("retained envelope nonce");
    assert_eq!(used, 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_actual_envelope_write_rolls_back_pair_but_keeps_reserved_nonce(pool: PgPool) {
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let key = delivery();
    register_key(&pool, &clock, &key)
        .await
        .expect("register key");
    pool.execute("CREATE FUNCTION test_verification_envelope_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876970,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_verification_envelope_wait AFTER INSERT ON email_verification_delivery_envelopes FOR EACH ROW EXECUTE FUNCTION test_verification_envelope_wait()").await.expect("post-write gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876970,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let work = issue_for_delivery(
        &pool,
        "buyer@example.test",
        &clock,
        &admission,
        limits(),
        &key,
    );
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO email_verification_delivery_envelopes%')").fetch_one(&pool).await.expect("actual envelope write wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("envelope reached write gate");
        clock.0.store(1900, Ordering::SeqCst);
        gate.rollback().await.expect("release gate");
    };
    let (result, ()) = tokio::join!(work, release);
    assert_eq!(result.err(), Some(VerificationError::Unavailable));
    assert_eq!(
        counts(&pool).await,
        (1, 0, 0, 1),
        "challenge and envelope roll back; admission and nonce remain"
    );
    clock.0.store(1000, Ordering::SeqCst);
    issue_for_delivery(
        &pool,
        "buyer@example.test",
        &clock,
        &admission,
        limits(),
        &key,
    )
    .await
    .expect("healthy retry");
    assert_eq!(counts(&pool).await, (2, 1, 1, 2));
    let used: i64 =
        sqlx::query_scalar("SELECT nonce_counter FROM email_verification_delivery_envelopes")
            .fetch_one(&pool)
            .await
            .expect("fresh nonce");
    assert_eq!(used, 2);
}

#[sqlx::test(migrations = false)]
async fn legacy_direct_challenges_survive_populated_upgrade_and_remain_direct(pool: PgPool) {
    MIGRATOR.run_to(57, &pool).await.expect("schema 57");
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let old = email_verification::issue(&pool, "old@example.test", &clock, &admission, limits())
        .await
        .expect("legacy challenge");
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade to delivery schema");
    let current =
        email_verification::issue(&pool, "new@example.test", &clock, &admission, limits())
            .await
            .expect("new direct challenge");
    let modes: Vec<(String, bool)> = sqlx::query_as("SELECT delivery_mode,delivery_transaction_id IS NULL FROM email_verification_challenges ORDER BY email_identity").fetch_all(&pool).await.expect("direct legacy modes");
    assert_eq!(
        modes,
        vec![("direct".into(), true), ("direct".into(), true)]
    );
    email_verification::consume(
        &pool,
        "old@example.test",
        &old,
        &clock,
        &admission,
        limits(),
    )
    .await
    .expect("old challenge remains consumable");
    email_verification::consume(
        &pool,
        "new@example.test",
        &current,
        &clock,
        &admission,
        limits(),
    )
    .await
    .expect("new direct challenge consumable");
    let envelopes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM email_verification_delivery_envelopes")
            .fetch_one(&pool)
            .await
            .expect("no implicit delivery");
    assert_eq!(envelopes, 0);
}

fn guard_error(error: sqlx::Error, message: &str) {
    let sqlx::Error::Database(error) = error else {
        panic!("expected native PostgreSQL error")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), message);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_challenge_and_envelope_require_exact_prior_committed_nonce(pool: PgPool) {
    let clock = Time(AtomicU64::new(1000));
    let key = delivery();
    register_key(&pool, &clock, &key)
        .await
        .expect("register key");
    let verifier = vec![11_u8; 32];
    let nonce_insert = "INSERT INTO email_verification_delivery_nonces(key_id,verifier,reserved_at) VALUES('key-one',$1,1000) RETURNING nonce_counter";
    let challenge = "INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at,delivery_mode) VALUES($1,'email-verification-v1','native@example.test',1000,1900,'envelope')";
    let envelope = "INSERT INTO email_verification_delivery_envelopes(verifier,key_id,nonce_counter,envelope_version,ciphertext) VALUES($1,'key-one',$2,1,$3)";
    let mut tx = pool.begin().await.expect("wrong isolation");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .expect("repeatable read");
    let error = sqlx::query(nonce_insert)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect_err("wrong isolation cannot allocate nonce");
    guard_error(error, "Verification delivery nonce requires read committed");
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("same transaction");
    let counter: i64 = sqlx::query_scalar(nonce_insert)
        .bind(&verifier)
        .fetch_one(&mut *tx)
        .await
        .expect("nonce within transaction");
    assert_eq!(counter, 1);
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge");
    let error = sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect_err("nonce not committed earlier");
    guard_error(
        error,
        "Verification delivery envelope requires exact challenge and prior nonce",
    );
    tx.rollback().await.expect("rollback all");
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    let counter: i64 = sqlx::query_scalar(nonce_insert)
        .bind(&verifier)
        .fetch_one(&pool)
        .await
        .expect("committed nonce");
    assert_eq!(counter, 1);
    let mut tx = pool.begin().await.expect("orphan challenge");
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge");
    let error =
        sqlx::query("SET CONSTRAINTS require_email_verification_challenge_envelope IMMEDIATE")
            .execute(&mut *tx)
            .await
            .expect_err("missing envelope");
    guard_error(
        error,
        "Verification challenge requires its same-transaction envelope",
    );
    tx.rollback().await.expect("rollback orphan");
    let mut tx = pool.begin().await.expect("orphan envelope");
    let error = sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect_err("missing challenge");
    guard_error(
        error,
        "Verification delivery envelope requires exact challenge and prior nonce",
    );
    tx.rollback().await.expect("rollback orphan");
    let mut tx = pool
        .begin()
        .await
        .expect("nonce bound to different verifier");
    let other_verifier = vec![12_u8; 32];
    sqlx::query(challenge)
        .bind(&other_verifier)
        .execute(&mut *tx)
        .await
        .expect("different challenge");
    let error = sqlx::query(envelope)
        .bind(&other_verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect_err("prior nonce must bind the same verifier");
    guard_error(
        error,
        "Verification delivery envelope requires exact challenge and prior nonce",
    );
    tx.rollback().await.expect("rollback mismatched verifier");
    let mut tx = pool.begin().await.expect("wrong ciphertext length");
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge");
    let error = sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 47])
        .execute(&mut *tx)
        .await
        .expect_err("ciphertext length must be exact");
    let sqlx::Error::Database(error) = error else {
        panic!("native shape error")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    tx.rollback().await.expect("rollback wrong shape");
    let mut tx = pool.begin().await.expect("positive pair");
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge");
    sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect("exact native envelope shape");
    tx.commit().await.expect("reciprocal constraints commit");
    let relation: (bool, bool, i64) = sqlx::query_as("SELECT c.delivery_transaction_id=e.transaction_id,n.transaction_id<>e.transaction_id,octet_length(e.ciphertext)::bigint FROM email_verification_delivery_envelopes e JOIN email_verification_challenges c USING(verifier) JOIN email_verification_delivery_nonces n USING(key_id,nonce_counter,verifier)").fetch_one(&pool).await.expect("native transaction bindings");
    assert_eq!(relation, (true, true, 48));
    assert_eq!(counts(&pool).await, (1, 1, 1, 0));
    for statement in [
        "UPDATE email_verification_delivery_nonces SET reserved_at=reserved_at",
        "DELETE FROM email_verification_delivery_nonces",
        "UPDATE email_verification_delivery_envelopes SET ciphertext=ciphertext",
        "DELETE FROM email_verification_delivery_envelopes",
    ] {
        assert!(
            pool.execute(statement).await.is_err(),
            "nonce and envelope are immutable"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn nonce_rejects_async_commit_but_issue_locally_forces_durable_reservation(pool: PgPool) {
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
    let configured: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&async_pool)
        .await
        .expect("async default");
    assert_eq!(configured, "off");
    let fsync: String = sqlx::query_scalar("SHOW fsync")
        .fetch_one(&async_pool)
        .await
        .expect("isolated server durability");
    assert_eq!(fsync, "on");
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let key = delivery();
    register_key(&async_pool, &clock, &key)
        .await
        .expect("register key");
    let error = sqlx::query("INSERT INTO email_verification_delivery_nonces(key_id,verifier,reserved_at) VALUES('key-one',$1,1000)").bind(vec![9_u8;32]).execute(&async_pool).await.expect_err("raw asynchronous nonce must be rejected");
    guard_error(error, "Verification delivery nonce requires durable commit");
    let mut downgrade = async_pool
        .begin()
        .await
        .expect("nonce downgrade transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *downgrade)
        .await
        .expect("enable synchronous commit for insert");
    sqlx::query("INSERT INTO email_verification_delivery_nonces(key_id,verifier,reserved_at) VALUES('key-one',$1,1000)")
        .bind(vec![9_u8; 32])
        .execute(&mut *downgrade)
        .await
        .expect("insert nonce with synchronous commit");
    sqlx::query("SET LOCAL synchronous_commit='off'")
        .execute(&mut *downgrade)
        .await
        .expect("downgrade after insert");
    let error = downgrade
        .commit()
        .await
        .expect_err("deferred guard rejects downgrade at commit");
    guard_error(error, "Verification delivery nonce requires durable commit");
    assert_eq!(counts(&async_pool).await, (0, 0, 0, 0));
    issue_for_delivery(
        &async_pool,
        "buyer@example.test",
        &clock,
        &admission,
        limits(),
        &key,
    )
    .await
    .expect("API forces durable nonce commit locally");
    assert_eq!(counts(&async_pool).await, (1, 1, 1, 1));
    let configured: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&async_pool)
        .await
        .expect("connection default restored");
    assert_eq!(
        configured, "off",
        "reservation override is transaction-local"
    );
    let verifier = vec![10_u8; 32];
    let mut tx = async_pool.begin().await.expect("durable nonce transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable nonce mode");
    let counter: i64 = sqlx::query_scalar("INSERT INTO email_verification_delivery_nonces(key_id,verifier,reserved_at) VALUES('key-one',$1,1000) RETURNING nonce_counter").bind(&verifier).fetch_one(&mut *tx).await.expect("prior durable nonce");
    tx.commit().await.expect("commit prior nonce");
    assert_eq!(counter, 2);
    let challenge = "INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at,delivery_mode) VALUES($1,'email-verification-v1','direct-test@example.test',1000,1900,'envelope')";
    let envelope = "INSERT INTO email_verification_delivery_envelopes(verifier,key_id,nonce_counter,envelope_version,ciphertext) VALUES($1,'key-one',$2,1,$3)";
    let mut tx = async_pool
        .begin()
        .await
        .expect("async envelope transaction");
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge before envelope");
    let error = sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect_err("raw async envelope rejected");
    guard_error(
        error,
        "Verification delivery envelope requires durable commit",
    );
    tx.rollback().await.expect("roll back pair");
    let mut tx = async_pool
        .begin()
        .await
        .expect("deferred durability transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable write mode");
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge");
    sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect("durable envelope write");
    sqlx::query("SET LOCAL synchronous_commit='off'")
        .execute(&mut *tx)
        .await
        .expect("simulate later downgrade");
    let error = sqlx::query("SET CONSTRAINTS require_email_verification_envelope IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect_err("deferred guard rejects downgrade");
    guard_error(
        error,
        "Verification delivery envelope requires durable commit",
    );
    tx.rollback().await.expect("roll back downgrade");
    assert_eq!(counts(&async_pool).await, (2, 1, 1, 1));
    let mut tx = async_pool
        .begin()
        .await
        .expect("durable positive transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable mode");
    sqlx::query(challenge)
        .bind(&verifier)
        .execute(&mut *tx)
        .await
        .expect("challenge");
    sqlx::query(envelope)
        .bind(&verifier)
        .bind(counter)
        .bind(vec![7_u8; 48])
        .execute(&mut *tx)
        .await
        .expect("envelope");
    tx.commit().await.expect("exact durable pair commits");
    assert_eq!(counts(&async_pool).await, (2, 2, 2, 1));
    async_pool.close().await;
}
