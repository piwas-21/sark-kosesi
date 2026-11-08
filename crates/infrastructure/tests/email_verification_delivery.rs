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

#[test]
fn delivery_key_rejects_ambiguous_identifiers_and_empty_material() {
    for invalid in ["", "-prefix", "bad.key", "bad/key", "clé", &"a".repeat(65)] {
        assert_eq!(
            VerificationDeliveryKey::capture(invalid, [41; 32]).err(),
            Some(VerificationError::Unavailable)
        );
    }
    assert_eq!(
        VerificationDeliveryKey::capture("valid", [0; 32]).err(),
        Some(VerificationError::Unavailable)
    );
    assert!(VerificationDeliveryKey::capture("Key-A_b1", [41; 32]).is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn key_registration_rejects_invalid_clock_or_changed_registration(pool: PgPool) {
    let clock = Time(AtomicU64::new(9_007_199_254_740_992));
    let key = delivery();
    assert_eq!(
        register_key(&pool, &clock, &key).await,
        Err(VerificationError::Unavailable)
    );
    clock.0.store(1000, Ordering::SeqCst);
    pool.execute("CREATE FUNCTION test_shift_key_registration() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.registered_at=NEW.registered_at+1; RETURN NEW; END; $$; CREATE TRIGGER z_test_shift_key_registration BEFORE INSERT ON email_verification_delivery_keys FOR EACH ROW EXECUTE FUNCTION test_shift_key_registration()").await.expect("change timestamp before persistence");
    assert_eq!(
        register_key(&pool, &clock, &key).await,
        Err(VerificationError::Unavailable)
    );
    let saved: i64 = sqlx::query_scalar("SELECT count(*) FROM email_verification_delivery_keys")
        .fetch_one(&pool)
        .await
        .expect("key registry count");
    assert_eq!(saved, 0);
    pool.execute("DROP TRIGGER z_test_shift_key_registration ON email_verification_delivery_keys")
        .await
        .expect("restore key registry");
    register_key(&pool, &clock, &key)
        .await
        .expect("intact key registration");
    clock.0.store(999, Ordering::SeqCst);
    assert_eq!(
        register_key(&pool, &clock, &key).await,
        Err(VerificationError::Unavailable)
    );
    let saved: i64 = sqlx::query_scalar(
        "SELECT registered_at FROM email_verification_delivery_keys WHERE key_id='key-one'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact prior key registration");
    assert_eq!(saved, 1000);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn custody_readback_rejects_changed_writes_and_local_durability_downgrade(pool: PgPool) {
    let clock = Time(AtomicU64::new(1000));
    let admission = admission();
    let key = delivery();
    let attempt = |key| {
        issue_for_delivery(
            &pool,
            "buyer@example.test",
            &clock,
            &admission,
            limits(),
            key,
        )
    };
    register_key(&pool, &clock, &key)
        .await
        .expect("register key");
    let wrong = VerificationDeliveryKey::capture("key-one", [42; 32]).expect("wrong material");
    assert_eq!(
        attempt(&wrong).await.err(),
        Some(VerificationError::Unavailable)
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 1));

    for (index, (add, remove)) in [
        (
            "CREATE FUNCTION test_change_nonce_time() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.reserved_at=NEW.reserved_at+1; RETURN NEW; END; $$; CREATE TRIGGER z_test_change_nonce_time BEFORE INSERT ON email_verification_delivery_nonces FOR EACH ROW EXECUTE FUNCTION test_change_nonce_time()",
            "DROP TRIGGER z_test_change_nonce_time ON email_verification_delivery_nonces",
        ),
        (
            "CREATE FUNCTION test_downgrade_nonce_durability() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM set_config('synchronous_commit','off',true); RETURN NEW; END; $$; CREATE TRIGGER z_test_downgrade_nonce_durability AFTER INSERT ON email_verification_delivery_nonces FOR EACH ROW EXECUTE FUNCTION test_downgrade_nonce_durability()",
            "DROP TRIGGER z_test_downgrade_nonce_durability ON email_verification_delivery_nonces",
        ),
    ].into_iter().enumerate() {
        pool.execute(add).await.expect("install isolated nonce fault");
        assert_eq!(attempt(&key).await.err(), Some(VerificationError::Unavailable));
        assert_eq!(counts(&pool).await, (0, 0, 0, i32::try_from(index).expect("small index") + 2));
        pool.execute(remove).await.expect("remove isolated nonce fault");
    }

    pool.execute("CREATE FUNCTION test_change_verification_identity() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.email_identity='substitution@example.test'; RETURN NEW; END; $$; CREATE TRIGGER z_test_change_verification_identity BEFORE INSERT ON email_verification_challenges FOR EACH ROW EXECUTE FUNCTION test_change_verification_identity()").await.expect("install coherent identity substitution");
    assert_eq!(
        attempt(&key).await.err(),
        Some(VerificationError::Unavailable)
    );
    assert_eq!(
        counts(&pool).await,
        (1, 0, 0, 4),
        "only the earlier debit and nonce survive"
    );
    pool.execute(
        "DROP TRIGGER z_test_change_verification_identity ON email_verification_challenges",
    )
    .await
    .expect("remove identity substitution");
    attempt(&key).await.expect("issue with intact custody");
    assert_eq!(counts(&pool).await, (2, 1, 1, 5));
    let bound: (String,i64) = sqlx::query_as("SELECT c.email_identity,e.nonce_counter FROM email_verification_challenges c JOIN email_verification_delivery_envelopes e USING(verifier)")
        .fetch_one(&pool).await.expect("exact positive source");
    assert_eq!(bound, ("buyer@example.test".into(), 2));
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
    // Inspect persisted custody encodings rather than only the ciphertext column.
    // A deliberately stored test secret proves that this scan can find a leak.
    let bearer_hex: String = bearer.iter().map(|byte| format!("{byte:02x}")).collect();
    let key_hex = "29".repeat(32);
    let marker = "synthetic-private-delivery-marker-513704";
    let custody: String = sqlx::query_scalar("SELECT jsonb_build_object('keys',(SELECT jsonb_agg(to_jsonb(k)) FROM email_verification_delivery_keys k),'nonces',(SELECT jsonb_agg(to_jsonb(n)) FROM email_verification_delivery_nonces n),'challenges',(SELECT jsonb_agg(to_jsonb(c)) FROM email_verification_challenges c),'envelopes',(SELECT jsonb_agg(to_jsonb(e)) FROM email_verification_delivery_envelopes e),'admission_scopes',(SELECT jsonb_agg(to_jsonb(s)) FROM customer_credential_admission_scopes s),'admission_targets',(SELECT jsonb_agg(to_jsonb(t)) FROM customer_credential_admission_targets t))::text")
        .fetch_one(&pool).await.expect("persisted custody scan");
    assert!(
        !custody.contains(&bearer_hex),
        "persisted custody contains no raw bearer"
    );
    assert!(
        !custody.contains(&key_hex),
        "persisted custody contains no raw encryption key"
    );
    assert!(!custody.contains(marker), "negative marker control");
    pool.execute("CREATE TABLE synthetic_delivery_secret_probe(bearer bytea, marker text)")
        .await
        .expect("probe table");
    sqlx::query("INSERT INTO synthetic_delivery_secret_probe VALUES($1,$2)")
        .bind(&*bearer)
        .bind(marker)
        .execute(&pool)
        .await
        .expect("persist synthetic scan control");
    let positive: String =
        sqlx::query_scalar("SELECT to_jsonb(p)::text FROM synthetic_delivery_secret_probe p")
            .fetch_one(&pool)
            .await
            .expect("positive control scan");
    assert!(
        positive.contains(&bearer_hex),
        "scan detects the persisted raw bearer control"
    );
    assert!(
        positive.contains(marker),
        "scan detects the persisted marker control"
    );
    pool.execute("DROP TABLE synthetic_delivery_secret_probe")
        .await
        .expect("remove synthetic control");
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

#[sqlx::test(migrator = "MIGRATOR")]
async fn dropping_a_blocked_envelope_write_burns_nonce_and_rolls_back_pair(pool: PgPool) {
    use std::sync::Arc;
    let clock = Arc::new(Time(AtomicU64::new(1000)));
    let key = delivery();
    register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("register key");
    pool.execute("CREATE FUNCTION test_verification_cancel_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876971,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_verification_cancel_wait AFTER INSERT ON email_verification_delivery_envelopes FOR EACH ROW EXECUTE FUNCTION test_verification_cancel_wait()").await.expect("actual envelope write gate");
    let mut gate = pool.begin().await.expect("gate transaction");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876971,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let mut observer = pool
        .acquire()
        .await
        .expect("independent observer connection");
    let work = tokio::spawn({
        let pool = pool.clone();
        let clock = clock.clone();
        async move {
            let admission = admission();
            let key = delivery();
            issue_for_delivery(
                &pool,
                "buyer@example.test",
                clock.as_ref(),
                &admission,
                limits(),
                &key,
            )
            .await
        }
    });
    let pid: i32 = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO email_verification_delivery_envelopes%' LIMIT 1").fetch_optional(&mut *observer).await.expect("observe actual envelope write wait");
            if let Some(pid) = pid { break pid; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("issuer reached real write gate");
    assert_eq!(
        counts(&pool).await,
        (1, 0, 0, 1),
        "committed nonce and debit precede the blocked writes"
    );
    work.abort();
    assert!(matches!(work.await, Err(error) if error.is_cancelled()));
    gate.rollback().await.expect("release wait");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let cleaned: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND (state IS DISTINCT FROM 'idle' OR xact_start IS NOT NULL))").bind(pid).fetch_one(&mut *observer).await.expect("observe transaction cleanup");
            if cleaned { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("aborted transaction finished cleanup");
    drop(observer);
    assert_eq!(
        counts(&pool).await,
        (1, 0, 0, 1),
        "drop rolls back challenge and envelope only"
    );
    pool.execute(
        "DROP TRIGGER z_test_verification_cancel_wait ON email_verification_delivery_envelopes",
    )
    .await
    .expect("remove gate");
    let admission = admission();
    issue_for_delivery(
        &pool,
        "buyer@example.test",
        clock.as_ref(),
        &admission,
        limits(),
        &key,
    )
    .await
    .expect("fresh nonce retry");
    assert_eq!(counts(&pool).await, (2, 1, 1, 2));
    let (reserved, used): (Vec<i64>, i64) = sqlx::query_as("SELECT (SELECT array_agg(nonce_counter ORDER BY nonce_counter) FROM email_verification_delivery_nonces),(SELECT nonce_counter FROM email_verification_delivery_envelopes)").fetch_one(&pool).await.expect("nonce burn and retry");
    assert_eq!((reserved, used), (vec![1, 2], 2));
}
