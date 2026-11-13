//! Private authenticated opening with synthetic keys and isolated PostgreSQL; no delivery I/O.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{
        self, VerificationError, VerificationToken,
        delivery::{
            VerificationDeliveryKey, issue_for_delivery,
            opening::{OpenOutcome, claim_and_open},
            register_key,
            start::{ClaimOutcome, claim},
        },
    },
};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgConnection, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

const ADDRESS: &str = "Case+tag@EXAMPLE.COM";
const NORMALIZED: &str = "Case+tag@example.com";
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
fn key() -> VerificationDeliveryKey {
    VerificationDeliveryKey::capture("key-one", [41; 32]).expect("synthetic delivery key")
}
async fn fixture(pool: &PgPool) -> (Arc<Time>, Arc<VerificationDeliveryKey>, [u8; 32]) {
    let time = Arc::new(Time(AtomicU64::new(1000)));
    let key = Arc::new(key());
    register_key(pool, time.as_ref(), &key)
        .await
        .expect("register key");
    let receipt = issue_for_delivery(pool, ADDRESS, time.as_ref(), &admission(), limits(), &key)
        .await
        .expect("encrypted envelope");
    (time, key, receipt.verifier)
}
fn exact_callback(identity: &str, bearer: &[u8; 32], verifier: &[u8; 32]) -> bool {
    let digest: [u8; 32] = Sha256::new()
        .chain_update(b"piwas:email-verification:verifier:v1\0")
        .chain_update(bearer)
        .finalize()
        .into();
    identity == NORMALIZED && digest == *verifier
}
async fn starts(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM email_verification_delivery_starts")
        .fetch_one(pool)
        .await
        .expect("start count")
}

// This test-only decryption feeds the real consumption API without logging or persisting plaintext.
async fn test_bearer(pool: &PgPool, verifier: &[u8; 32]) -> VerificationToken {
    let (counter, mut ciphertext, created, expires):(i64,Vec<u8>,i64,i64)=sqlx::query_as("SELECT e.nonce_counter,e.ciphertext,c.created_at,c.expires_at FROM email_verification_delivery_envelopes e JOIN email_verification_challenges c USING(verifier) WHERE e.verifier=$1").bind(verifier.as_slice()).fetch_one(pool).await.expect("envelope");
    let created = created.to_be_bytes();
    let expires = expires.to_be_bytes();
    let version = 1_i16.to_be_bytes();
    let counter_bytes = counter.to_be_bytes();
    let mut aad = Vec::new();
    for field in [
        b"piwas:verification-delivery:aad:v1".as_slice(),
        b"email-verification-v1",
        NORMALIZED.as_bytes(),
        verifier.as_slice(),
        created.as_slice(),
        expires.as_slice(),
        version.as_slice(),
        b"key-one",
        counter_bytes.as_slice(),
    ] {
        aad.extend_from_slice(&u32::try_from(field.len()).expect("bounded").to_be_bytes());
        aad.extend_from_slice(field);
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
        .expect("authenticated test envelope");
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(bearer);
    VerificationToken::from_bytes(bytes)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_openers_invoke_one_exact_callback_and_replay_never_reopens(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let calls = AtomicUsize::new(0);
    let correct = AtomicBool::new(false);
    let open = || {
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |identity, bearer| {
            correct.store(
                exact_callback(identity, bearer, &verifier),
                Ordering::SeqCst,
            );
            calls.fetch_add(1, Ordering::SeqCst);
        })
    };
    let (a, b) = tokio::join!(open(), open());
    assert!(matches!(
        (a, b),
        (Ok(OpenOutcome::Opened), Ok(OpenOutcome::AlreadyStarted))
            | (Ok(OpenOutcome::AlreadyStarted), Ok(OpenOutcome::Opened))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(correct.load(Ordering::SeqCst));
    assert_eq!(starts(&pool).await, 1);
    assert!(matches!(open().await, Ok(OpenOutcome::AlreadyStarted)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_at FROM email_verification_challenges WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("challenge");
    assert!(consumed.is_none(), "opening does not verify a mailbox");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn journal_only_prestart_never_grants_an_opening_callback(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    assert_eq!(
        claim(&pool, &verifier, time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started)
    );
    let calls = AtomicUsize::new(0);
    assert!(matches!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(starts(&pool).await, 1);
}

async fn tampered_envelope_case(pool: PgPool, tamper_identity: bool) {
    let (time, key, verifier) = fixture(&pool).await;
    if tamper_identity {
        pool.execute("ALTER TABLE email_verification_challenges DISABLE TRIGGER guard_email_verification_challenge").await.expect("isolate damaged AAD");
        sqlx::query("UPDATE email_verification_challenges SET email_identity='Other@example.com' WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("alter authenticated identity");
    } else {
        pool.execute("ALTER TABLE email_verification_delivery_envelopes DISABLE TRIGGER guard_email_verification_delivery_envelope").await.expect("isolate ciphertext damage");
        sqlx::query("UPDATE email_verification_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("change one ciphertext byte");
    }
    let calls = AtomicUsize::new(0);
    assert!(
        matches!(
            claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
            .await,
            Err(VerificationError::Unavailable)
        ),
        "authentication rejects modified ciphertext or AAD"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        starts(&pool).await,
        1,
        "failed authentication retains the committed unknown start"
    );
    if tamper_identity {
        sqlx::query("UPDATE email_verification_challenges SET email_identity=$1 WHERE verifier=$2")
            .bind(NORMALIZED)
            .bind(verifier.as_slice())
            .execute(&pool)
            .await
            .expect("restore identity");
        pool.execute("ALTER TABLE email_verification_challenges ENABLE TRIGGER guard_email_verification_challenge").await.expect("restore challenge guard");
    } else {
        sqlx::query("UPDATE email_verification_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("restore ciphertext");
        pool.execute("ALTER TABLE email_verification_delivery_envelopes ENABLE TRIGGER guard_email_verification_delivery_envelope").await.expect("restore envelope guard");
    }
    assert!(matches!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "uncertain prior start never receives another opening"
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn same_length_ciphertext_damage_fails_closed_after_start(pool: PgPool) {
    tampered_envelope_case(pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_normalized_identity_fails_authenticated_opening(pool: PgPool) {
    tampered_envelope_case(pool, true).await;
}

async fn wait_for(pool: &PgPool, sql: &'static str) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let found: bool = sqlx::query_scalar(sql)
                .fetch_one(pool)
                .await
                .expect("observe database lock");
            if found {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected actual lock wait");
}
// The observed waiter must be blocked by the specific transaction we hold.
async fn wait_for_blocked(
    connection: &mut PgConnection,
    blocking_pid: i32,
    expected_pid: i32,
    phase: &'static str,
) {
    let mut observed_waiters = 0;
    let mut expected_waiting = false;
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiters: Vec<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%email_verification_challenges%FOR UPDATE%' ORDER BY pid LIMIT 2",
            )
            .bind(blocking_pid)
            .fetch_all(&mut *connection)
            .await
            .expect("observe exact blocking transaction");
            observed_waiters = waiters.len();
            expected_waiting = waiters.contains(&expected_pid);
            assert!(observed_waiters <= 1, "ambiguous waiter at {phase}");
            if expected_waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "expected actual lock wait at {phase}; selected_waiting={expected_waiting}, matching_waiters={observed_waiters}"
    );
}

fn start_gate_sql() -> &'static str {
    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO email_verification_delivery_starts%')"
}
fn challenge_wait_sql() -> &'static str {
    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%email_verification_challenges%FOR UPDATE%')"
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn consumption_queued_before_fresh_open_denies_callback_but_keeps_start(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let token = test_bearer(&pool, &verifier).await;
    pool.execute("CREATE FUNCTION test_opening_start_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876990,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_opening_start_wait AFTER INSERT ON email_verification_delivery_starts FOR EACH ROW EXECUTE FUNCTION test_opening_start_wait()").await.expect("start gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876990,1)")
        .execute(&mut *gate)
        .await
        .expect("hold");
    let calls = Arc::new(AtomicUsize::new(0));
    let opening = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        let calls = calls.clone();
        async move {
            claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
        }
    });
    wait_for(&pool, start_gate_sql()).await;
    let consuming = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        async move {
            email_verification::consume(
                &pool,
                ADDRESS,
                &token,
                time.as_ref(),
                &admission(),
                limits(),
            )
            .await
        }
    });
    wait_for(&pool, challenge_wait_sql()).await;
    gate.rollback().await.expect("release start");
    assert_eq!(consuming.await.expect("consume task"), Ok(()));
    assert!(matches!(
        opening.await.expect("opening task"),
        Err(VerificationError::Denied)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        starts(&pool).await,
        1,
        "start precedes subsequent consumption"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_while_fresh_open_waits_for_challenge_denies_callback(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let held = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let opener_pid = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let first_release = Arc::new(AtomicBool::new(true));
    // Hold the single pool connection after its first transaction returns it.
    // The independent start-count check below proves that COMMIT succeeded.
    let opening_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_release({
            let held = held.clone();
            let resume = resume.clone();
            let opener_pid = opener_pid.clone();
            let first_release = first_release.clone();
            move |connection, _| {
                let held = held.clone();
                let resume = resume.clone();
                let opener_pid = opener_pid.clone();
                let first_release = first_release.clone();
                Box::pin(async move {
                    if first_release.swap(false, Ordering::SeqCst) {
                        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                            .fetch_one(connection)
                            .await?;
                        opener_pid.store(pid, Ordering::SeqCst);
                        held.notify_one();
                        if tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            resume.notified(),
                        )
                        .await
                        .is_err()
                        {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })
            }
        })
        .connect_lazy_with(pool.connect_options().as_ref().clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut opening = tokio::spawn({
        let pool = opening_pool.clone();
        let time = time.clone();
        let key = key.clone();
        let calls = calls.clone();
        async move {
            claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), held.notified())
        .await
        .expect("first connection release deadline");
    assert_eq!(starts(&pool).await, 1, "start committed before source lock");
    let mut blocker = pool.begin().await.expect("source blocker");
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("source blocker backend");
    sqlx::query("SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE")
        .bind(verifier.as_slice())
        .execute(&mut *blocker)
        .await
        .expect("hold source after committed start");
    let selected_pid = opener_pid.load(Ordering::SeqCst);
    assert!(selected_pid > 0, "opening backend selected");
    resume.notify_one();
    tokio::select! {
        () = wait_for_blocked(&mut blocker, blocker_pid, selected_pid, "fresh opening") => {},
        result = &mut opening => {
            let outcome = match result {
                Ok(Ok(OpenOutcome::Opened)) => "opened",
                Ok(Ok(OpenOutcome::AlreadyStarted)) => "already_started",
                Ok(Err(VerificationError::Denied)) => "denied",
                Ok(Err(VerificationError::Unavailable)) => "unavailable",
                Err(_) => "task_failed",
            };
            panic!("opening completed before actual source wait: {outcome}");
        },
    }
    time.0.store(1900, Ordering::SeqCst);
    blocker
        .rollback()
        .await
        .expect("release source after expiry");
    assert!(matches!(
        opening.await.expect("opening task"),
        Err(VerificationError::Denied)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(starts(&pool).await, 1);
    opening_pool.close().await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn callback_runs_after_challenge_lock_is_released(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let options = pool.connect_options().as_ref().clone();
    let lock_free = AtomicBool::new(false);
    let correct = AtomicBool::new(false);
    let outcome=claim_and_open(&pool,&verifier,time.as_ref(),&key,|identity,bearer| {
        correct.store(exact_callback(identity,bearer,&verifier),Ordering::SeqCst);
        let acquired=std::thread::spawn(move || {
            let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build();
            let Ok(runtime)=runtime else {return false;};
            runtime.block_on(async move {
                let Ok(pool)=sqlx::postgres::PgPoolOptions::new().max_connections(1).connect_with(options).await else {return false;};
                let Ok(mut tx)=pool.begin().await else {return false;};
                let acquired=sqlx::query("SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE NOWAIT").bind(verifier.as_slice()).execute(&mut *tx).await.is_ok();
                let _=tx.rollback().await;
                acquired
            })
        }).join().unwrap_or(false);
        lock_free.store(acquired,Ordering::SeqCst);
    }).await.expect("authenticated opening");
    assert!(matches!(outcome, OpenOutcome::Opened));
    assert!(correct.load(Ordering::SeqCst));
    assert!(
        lock_free.load(Ordering::SeqCst),
        "independent writer acquired challenge while callback was active"
    );
    assert_eq!(starts(&pool).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn substituted_ciphertext_after_start_commit_is_not_opened(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    pool.execute("ALTER TABLE email_verification_delivery_envelopes DISABLE TRIGGER guard_email_verification_delivery_envelope; CREATE FUNCTION test_opening_swap_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876992,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_opening_swap_wait AFTER INSERT ON email_verification_delivery_starts FOR EACH ROW EXECUTE FUNCTION test_opening_swap_wait()").await.expect("isolated source-change gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876992,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let calls = Arc::new(AtomicUsize::new(0));
    let opening = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        let calls = calls.clone();
        async move {
            claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
        }
    });
    wait_for(&pool, start_gate_sql()).await;
    tokio::time::timeout(std::time::Duration::from_secs(2),sqlx::query("UPDATE email_verification_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool)).await.expect("independent source update does not wait for start").expect("same-length change");
    gate.rollback().await.expect("release start");
    assert!(matches!(
        opening.await.expect("opening task"),
        Err(VerificationError::Unavailable)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(starts(&pool).await, 1);
    sqlx::query("UPDATE email_verification_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("restore ciphertext");
    pool.execute("ALTER TABLE email_verification_delivery_envelopes ENABLE TRIGGER guard_email_verification_delivery_envelope").await.expect("restore guard");
    assert!(matches!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

async fn changed_retained_start_case(pool: PgPool, change_xid: bool) {
    let (time, key, verifier) = fixture(&pool).await;
    time.0.store(1001, Ordering::SeqCst);
    pool.execute("CREATE FUNCTION test_opening_retained_start_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876993,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_opening_retained_start_wait AFTER INSERT ON email_verification_delivery_starts FOR EACH ROW EXECUTE FUNCTION test_opening_retained_start_wait()")
        .await
        .expect("isolate start change and gate insertion");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876993,1)")
        .execute(&mut *gate)
        .await
        .expect("hold start gate");
    let calls = Arc::new(AtomicUsize::new(0));
    let opening = tokio::spawn({
        let pool = pool.clone();
        let time = time.clone();
        let key = key.clone();
        let calls = calls.clone();
        async move {
            claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
        }
    });
    wait_for(&pool, start_gate_sql()).await;
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let blocker = tokio::spawn({
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.expect("fresh-opening blocker");
            sqlx::query(
                "SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE",
            )
            .bind(verifier.as_slice())
            .execute(&mut *tx)
            .await
            .expect("hold challenge after start");
            held_tx.send(()).expect("signal challenge held");
            release_rx.await.expect("release signal");
            tx.rollback().await.expect("release challenge");
        }
    });
    wait_for(&pool, challenge_wait_sql()).await;
    gate.rollback().await.expect("let start commit");
    tokio::time::timeout(std::time::Duration::from_secs(2), held_rx)
        .await
        .expect("blocker acquired challenge")
        .expect("blocker signal");
    wait_for(&pool, challenge_wait_sql()).await;

    let snapshot_sql = "SELECT s.started_at,s.transaction_id::text,email_verification_delivery_start_source(s) FROM email_verification_delivery_starts s WHERE s.verifier=$1";
    let original: (i64, String, bool) = sqlx::query_as(snapshot_sql)
        .bind(verifier.as_slice())
        .fetch_one(&pool)
        .await
        .expect("original retained start");
    assert_eq!(original.0, 1001);
    assert!(original.2);
    pool.execute("ALTER TABLE email_verification_delivery_starts DISABLE TRIGGER guard_email_verification_delivery_start")
        .await
        .expect("isolate committed start change");
    let update = if change_xid {
        "UPDATE email_verification_delivery_starts SET transaction_id=pg_current_xact_id() WHERE verifier=$1"
    } else {
        "UPDATE email_verification_delivery_starts SET started_at=1000 WHERE verifier=$1"
    };
    sqlx::query(update)
        .bind(verifier.as_slice())
        .execute(&pool)
        .await
        .expect("change retained start while fresh opening waits");
    let damaged: (i64, String, bool) = sqlx::query_as(snapshot_sql)
        .bind(verifier.as_slice())
        .fetch_one(&pool)
        .await
        .expect("changed retained start");
    assert!(
        damaged.2,
        "changed start still passes native source validation"
    );
    if change_xid {
        assert_ne!(damaged.1, original.1);
        assert_eq!(damaged.0, original.0);
    } else {
        assert_eq!(damaged.0, 1000);
        assert_eq!(damaged.1, original.1);
    }
    release_tx.send(()).expect("release fresh opening");
    blocker.await.expect("blocker task");
    assert!(matches!(
        opening.await.expect("opening task"),
        Err(VerificationError::Unavailable)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(starts(&pool).await, 1);
    let after: (i64, String, bool) = sqlx::query_as(snapshot_sql)
        .bind(verifier.as_slice())
        .fetch_one(&pool)
        .await
        .expect("start after failed opening");
    assert_eq!(
        after, damaged,
        "failed opening leaves the retained start unchanged"
    );
    sqlx::query("UPDATE email_verification_delivery_starts SET started_at=$2,transaction_id=$3::text::xid8 WHERE verifier=$1")
        .bind(verifier.as_slice())
        .bind(original.0)
        .bind(&original.1)
        .execute(&pool)
        .await
        .expect("restore retained start");
    pool.execute("ALTER TABLE email_verification_delivery_starts ENABLE TRIGGER guard_email_verification_delivery_start")
        .await
        .expect("restore start guard");
    assert!(matches!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_retained_start_time_during_fresh_open_wait_fails_closed(pool: PgPool) {
    changed_retained_start_case(pool, false).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_retained_start_xid_during_fresh_open_wait_fails_closed(pool: PgPool) {
    changed_retained_start_case(pool, true).await;
}

struct FinalRewind(AtomicUsize);
impl Clock for FinalRewind {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(if index >= 4 { 999 } else { 1000 })
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn final_clock_rewind_after_authenticated_open_does_not_invoke_callback(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let rewind = FinalRewind(AtomicUsize::new(0));
    let calls = AtomicUsize::new(0);
    assert!(matches!(
        claim_and_open(&pool, &verifier, &rewind, &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Err(VerificationError::Unavailable)
    ));
    assert!(
        rewind.0.load(Ordering::SeqCst) >= 5,
        "failure happens on the final clock sample after authenticated opening"
    );
    assert_eq!(starts(&pool).await, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

struct PostCommitExpiry(AtomicUsize);
impl Clock for PostCommitExpiry {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        let index = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(if index >= 5 { 1900 } else { 1000 })
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_at_post_commit_fence_never_invokes_callback(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let expiry = PostCommitExpiry(AtomicUsize::new(0));
    let calls = AtomicUsize::new(0);
    assert!(matches!(
        claim_and_open(&pool, &verifier, &expiry, &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Err(VerificationError::Denied)
    ));
    assert_eq!(
        expiry.0.load(Ordering::SeqCst),
        6,
        "expiry is observed at the post-commit clock fence"
    );
    assert_eq!(starts(&pool).await, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
