//! Synthetic callback and isolated PostgreSQL only; no provider or real mail.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{
        self, VerificationError, VerificationToken,
        delivery::{
            VerificationDeliveryKey, issue_for_delivery,
            opening::{OpenOutcome, claim_and_open, claim_open_and_record},
            outcome::{DeliveryState, TransportOutcome, inspect},
            register_key,
        },
    },
};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

const ADDRESS: &str = "Example+synthetic@example.com";
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
async fn fixture(pool: &PgPool) -> (Arc<Time>, VerificationDeliveryKey, [u8; 32]) {
    let time = Arc::new(Time(AtomicU64::new(1000)));
    let key = VerificationDeliveryKey::capture("test-key", [41; 32]).expect("key");
    register_key(pool, time.as_ref(), &key)
        .await
        .expect("key registration");
    let receipt = issue_for_delivery(pool, ADDRESS, time.as_ref(), &admission(), limits(), &key)
        .await
        .expect("issue");
    (time, key, receipt.verifier)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_results_are_immutable_and_replay_never_calls_back(pool: PgPool) {
    for expected in [
        TransportOutcome::Accepted,
        TransportOutcome::ExplicitRejection,
        TransportOutcome::Uncertain,
    ] {
        let (time, key, verifier) = fixture(&pool).await;
        assert_eq!(
            inspect(&pool, &verifier).await,
            Ok(DeliveryState::NotStarted)
        );
        let calls = AtomicUsize::new(0);
        assert_eq!(
            claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                expected
            })
            .await,
            Ok(OpenOutcome::Opened)
        );
        assert_eq!(
            inspect(&pool, &verifier).await,
            Ok(DeliveryState::Recorded(expected))
        );
        assert_eq!(
            claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                TransportOutcome::Accepted
            })
            .await,
            Ok(OpenOutcome::AlreadyStarted)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let update = sqlx::query(
            "UPDATE email_verification_delivery_outcomes SET outcome='accepted' WHERE verifier=$1",
        )
        .bind(verifier.as_slice())
        .execute(&pool)
        .await;
        assert!(update.is_err());
        let delete =
            sqlx::query("DELETE FROM email_verification_delivery_outcomes WHERE verifier=$1")
                .bind(verifier.as_slice())
                .execute(&pool)
                .await;
        assert!(delete.is_err());
        let columns: Vec<String> = sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_name='email_verification_delivery_outcomes' ORDER BY ordinal_position")
            .fetch_all(&pool).await.expect("outcome columns");
        assert_eq!(
            columns,
            [
                "verifier",
                "start_transaction_id",
                "outcome",
                "observed_at",
                "transaction_id"
            ]
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_result_stays_unknown_after_opening_or_record_failure(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    assert_eq!(
        claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {}).await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    let calls = AtomicUsize::new(0);
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            TransportOutcome::Accepted
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let (time, key, verifier) = fixture(&pool).await;
    pool.execute("CREATE FUNCTION test_suppress_verification_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER z_suppress_verification_outcome BEFORE INSERT ON email_verification_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION test_suppress_verification_outcome()").await.expect("suppress result");
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            TransportOutcome::Accepted
        })
        .await,
        Err(VerificationError::Unavailable)
    );
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    pool.execute(
        "DROP TRIGGER z_suppress_verification_outcome ON email_verification_delivery_outcomes",
    )
    .await
    .expect("restore writes");
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| panic!(
            "must not retry"
        ))
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn result_after_expiry_remains_historical_evidence(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            time.0.store(1900, Ordering::SeqCst);
            TransportOutcome::Accepted
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_at FROM email_verification_challenges WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("challenge");
    assert!(consumed.is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn result_after_redemption_records_without_holding_a_transaction(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let options = (*pool.connect_options()).clone();
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |identity, bearer| {
            let identity = identity.to_owned();
            let token = VerificationToken::from_bytes(*bearer);
            let time = time.clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                runtime.block_on(async move {
                    let other = PgPoolOptions::new()
                        .max_connections(2)
                        .connect_with(options)
                        .await
                        .expect("separate pool");
                    email_verification::consume(
                        &other,
                        &identity,
                        &token,
                        time.as_ref(),
                        &admission(),
                        limits(),
                    )
                    .await
                    .expect("consume while callback runs");
                    other.close().await;
                });
            })
            .join()
            .expect("consumer thread");
            TransportOutcome::ExplicitRejection
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::ExplicitRejection))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_openers_have_one_callback_and_one_result(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let calls = AtomicUsize::new(0);
    let open = || {
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            TransportOutcome::Accepted
        })
    };
    let (first, second) = tokio::join!(open(), open());
    assert!(matches!(
        (first, second),
        (Ok(OpenOutcome::Opened), Ok(OpenOutcome::AlreadyStarted))
            | (Ok(OpenOutcome::AlreadyStarted), Ok(OpenOutcome::Opened))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_guard_rejects_wrong_start_and_unbounded_outcome(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    claim_and_open(&pool, &verifier, time.as_ref(), &key, |_, _| {})
        .await
        .expect("start");
    let wrong_xid = sqlx::query("INSERT INTO email_verification_delivery_outcomes(verifier,start_transaction_id,outcome,observed_at) VALUES($1,pg_current_xact_id(),'accepted',1000)")
        .bind(verifier.as_slice()).execute(&pool).await;
    assert!(wrong_xid.is_err());
    let invalid = sqlx::query("INSERT INTO email_verification_delivery_outcomes(verifier,start_transaction_id,outcome,observed_at) SELECT verifier,transaction_id,'provider body',1000 FROM email_verification_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&pool).await;
    assert!(invalid.is_err());
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn panic_after_start_leaves_unknown_and_never_reopens(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let task = tokio::spawn({
        let pool = pool.clone();
        async move {
            claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
                panic!("synthetic callback crash")
            })
            .await
        }
    });
    assert!(task.await.expect_err("callback must panic").is_panic());
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    let key = VerificationDeliveryKey::capture("test-key", [41; 32]).expect("key");
    let time = Time(AtomicU64::new(1000));
    assert_eq!(
        claim_open_and_record(&pool, &verifier, &time, &key, |_, _| panic!(
            "must not reopen"
        ))
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_start_transaction_after_callback_fails_closed(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let options = (*pool.connect_options()).clone();
    assert_eq!(claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            runtime.block_on(async move {
                let other = PgPoolOptions::new().max_connections(1).connect_with(options).await.expect("separate pool");
                other.execute("ALTER TABLE email_verification_delivery_starts DISABLE TRIGGER guard_email_verification_delivery_start").await.expect("isolate corrupted start");
                sqlx::query("UPDATE email_verification_delivery_starts SET transaction_id=pg_current_xact_id() WHERE verifier=$1").bind(verifier.as_slice()).execute(&other).await.expect("replace start xid");
                other.execute("ALTER TABLE email_verification_delivery_starts ENABLE TRIGGER guard_email_verification_delivery_start").await.expect("restore guard");
                other.close().await;
            });
        }).join().expect("corruption thread");
        TransportOutcome::Accepted
    }).await, Err(VerificationError::Unavailable));
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn outcome_row_does_not_contain_synthetic_mailbox_or_bearer(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let mut captured = [0_u8; 32];
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, bearer| {
            captured.copy_from_slice(bearer);
            TransportOutcome::Accepted
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    let (row, bearer_hex): (String, String) = sqlx::query_as("SELECT row_to_json(o)::text,encode($2::bytea,'hex') FROM email_verification_delivery_outcomes o WHERE verifier=$1")
        .bind(verifier.as_slice()).bind(captured.as_slice()).fetch_one(&pool).await.expect("inspect synthetic row");
    assert!(!row.contains(ADDRESS));
    assert!(!row.contains(&bearer_hex));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inspect_rejects_damaged_outcome_and_start_binding(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            TransportOutcome::Accepted
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
    pool.execute("ALTER TABLE email_verification_delivery_outcomes DISABLE TRIGGER guard_email_verification_delivery_outcome; ALTER TABLE email_verification_delivery_outcomes DROP CONSTRAINT email_verification_delivery_outcomes_outcome_check").await.expect("isolate damaged storage");
    sqlx::query(
        "UPDATE email_verification_delivery_outcomes SET outcome='invalid' WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .execute(&pool)
    .await
    .expect("damage classification");
    assert_eq!(
        inspect(&pool, &verifier).await,
        Err(VerificationError::Unavailable)
    );
    sqlx::query("UPDATE email_verification_delivery_outcomes SET outcome='accepted',start_transaction_id=transaction_id WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("damage start binding");
    assert_eq!(
        inspect(&pool, &verifier).await,
        Err(VerificationError::Unavailable)
    );
    sqlx::query("UPDATE email_verification_delivery_outcomes o SET start_transaction_id=s.transaction_id FROM email_verification_delivery_starts s WHERE o.verifier=$1 AND s.verifier=o.verifier").bind(verifier.as_slice()).execute(&pool).await.expect("restore binding");
    assert_eq!(
        inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_classification_after_guard_rolls_back_exact_readback(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    pool.execute("CREATE FUNCTION test_change_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.outcome='uncertain'; RETURN NEW; END $$; CREATE TRIGGER z_change_outcome BEFORE INSERT ON email_verification_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION test_change_outcome()").await.expect("install post-guard change");
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            TransportOutcome::Accepted
        })
        .await,
        Err(VerificationError::Unavailable)
    );
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    pool.execute("DROP TRIGGER z_change_outcome ON email_verification_delivery_outcomes")
        .await
        .expect("remove change");
    let (time, key, control) = fixture(&pool).await;
    assert_eq!(
        claim_open_and_record(&pool, &control, time.as_ref(), &key, |_, _| {
            TransportOutcome::Accepted
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        inspect(&pool, &control).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_start_after_insert_fails_exact_binding_even_when_native_check_passes(
    pool: PgPool,
) {
    let (time, key, verifier) = fixture(&pool).await;
    pool.execute("CREATE FUNCTION test_change_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN UPDATE email_verification_delivery_starts SET started_at=started_at-1 WHERE verifier=NEW.verifier; RETURN NEW; END $$; CREATE TRIGGER z_change_start AFTER INSERT ON email_verification_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION test_change_start()").await.expect("install post-insert start mutation");
    let options = (*pool.connect_options()).clone();
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
                runtime.block_on(async move {
                    let other = PgPoolOptions::new().max_connections(1).connect_with(options).await.expect("separate pool");
                    other.execute("ALTER TABLE email_verification_delivery_starts DISABLE TRIGGER guard_email_verification_delivery_start").await.expect("isolate corrupted start after opening");
                    other.close().await;
                });
            }).join().expect("isolation thread");
            TransportOutcome::Accepted
        })
        .await,
        Err(VerificationError::Unavailable)
    );
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    let retained: i64 = sqlx::query_scalar(
        "SELECT started_at FROM email_verification_delivery_starts WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("retained start");
    assert_eq!(retained, 1000, "failed operation rolled the mutation back");
    let mut tx = pool.begin().await.expect("native control");
    sqlx::query("INSERT INTO email_verification_delivery_outcomes(verifier,start_transaction_id,outcome,observed_at) SELECT verifier,transaction_id,'accepted',1000 FROM email_verification_delivery_starts WHERE verifier=$1").bind(verifier.as_slice()).execute(&mut *tx).await.expect("insert control");
    sqlx::query("SET CONSTRAINTS require_email_verification_delivery_outcome IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("native constraint accepts changed start time");
    let changed: i64 = sqlx::query_scalar(
        "SELECT started_at FROM email_verification_delivery_starts WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&mut *tx)
    .await
    .expect("changed start");
    assert_eq!(changed, 999);
    tx.rollback().await.expect("discard control");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_start_xid_or_chronology_fails_real_deferred_constraint(pool: PgPool) {
    for setup in [
        "CREATE OR REPLACE FUNCTION test_damage_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.start_transaction_id=NEW.transaction_id; RETURN NEW; END $$; CREATE TRIGGER z_damage_outcome BEFORE INSERT ON email_verification_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION test_damage_outcome()",
        "CREATE OR REPLACE FUNCTION test_damage_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.observed_at=0; RETURN NEW; END $$; CREATE TRIGGER z_damage_outcome BEFORE INSERT ON email_verification_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION test_damage_outcome()",
    ] {
        let (time, key, verifier) = fixture(&pool).await;
        pool.execute(setup)
            .await
            .expect("install post-guard corruption");
        assert_eq!(
            claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
                TransportOutcome::Accepted
            })
            .await,
            Err(VerificationError::Unavailable)
        );
        assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
        let mut tx = pool.begin().await.expect("native control");
        sqlx::query("INSERT INTO email_verification_delivery_outcomes(verifier,start_transaction_id,outcome,observed_at) SELECT verifier,transaction_id,'accepted',1000 FROM email_verification_delivery_starts WHERE verifier=$1").bind(verifier.as_slice()).execute(&mut *tx).await.expect("guard executes before test mutation");
        let error =
            sqlx::query("SET CONSTRAINTS require_email_verification_delivery_outcome IMMEDIATE")
                .execute(&mut *tx)
                .await
                .expect_err("real deferred constraint must fail");
        assert!(
            error
                .to_string()
                .contains("Verification delivery outcome requires its prior durable start")
        );
        tx.rollback().await.expect("discard failed control");
        pool.execute("DROP TRIGGER z_damage_outcome ON email_verification_delivery_outcomes")
            .await
            .expect("remove mutation");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_rewind_in_callback_leaves_start_unknown(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| {
            time.0.store(999, Ordering::SeqCst);
            TransportOutcome::Accepted
        })
        .await,
        Err(VerificationError::Unavailable)
    );
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    time.0.store(1000, Ordering::SeqCst);
    assert_eq!(
        claim_open_and_record(&pool, &verifier, time.as_ref(), &key, |_, _| panic!(
            "must not reopen"
        ))
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
}
