//! Real admission, proof and credential custody with a synthetic offline snapshot.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_passwords::{self, BlocklistManifest, PasswordBlocklist, PasswordError},
    email_verification, verified_customer_accounts,
};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn list(valid_until: i64) -> PasswordBlocklist {
    // SHA-256 of the literal NFC UTF-8 string "café password is blocked".
    let m = BlocklistManifest {
        version: "test-only".into(),
        provenance: "synthetic fixture".into(),
        qualification_ref: "test-only".into(),
        valid_from: 0,
        valid_until,
        digests: vec![[
            0x34, 0xab, 0x20, 0x08, 0x7a, 0x69, 0x33, 0xd7, 0x76, 0x8e, 0xdf, 0x56, 0xb6, 0xf1,
            0x26, 0xf6, 0x92, 0x3c, 0x21, 0x1b, 0x52, 0xb2, 0x04, 0x61, 0x48, 0x6f, 0x0a, 0xd6,
            0x79, 0xfe, 0xb3, 0xe2,
        ]],
    };
    let digest = m.integrity_digest();
    PasswordBlocklist::new(m, digest).expect("synthetic snapshot")
}
async fn account(pool: &PgPool, clock: &Time, key: &AdmissionKey, limits: AdmissionLimits) {
    let proof = email_verification::issue(pool, "blocklist@example.com", clock, key, limits)
        .await
        .expect("email proof");
    verified_customer_accounts::establish(
        pool,
        "blocklist@example.com",
        &proof,
        clock,
        key,
        limits,
    )
    .await
    .expect("account");
}
async fn state(pool: &PgPool) -> (i64, i64, Option<String>, i32) {
    sqlx::query_as("SELECT (SELECT count(*) FROM customer_password_credentials), (SELECT count(*) FROM customer_password_challenges WHERE consumed_at IS NULL), (SELECT password_hash FROM customer_password_credentials LIMIT 1), (SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=5)")
        .fetch_one(pool).await.expect("proof, credential and committed debit")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn rejected_blocklist_checks_retain_existing_credential_proof_and_admission_debits(
    pool: PgPool,
) {
    let clock = Time(AtomicU64::new(1000));
    let key = AdmissionKey::new(1, [81; 32]).expect("synthetic key");
    let limits = AdmissionLimits::new(20, 20).expect("test limits");
    account(&pool, &clock, &key, limits).await;
    let first = customer_passwords::issue(&pool, "blocklist@example.com", &clock, &key, limits)
        .await
        .expect("first proof");
    let current = list(3000);
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "blocklist@example.com",
            &first,
            customer_passwords::PasswordCandidate {
                password: "first safe password",
                blocklist: Some(&current)
            },
            &clock,
            &key,
            limits
        )
        .await,
        Ok(1)
    );
    let proof = customer_passwords::issue(&pool, "blocklist@example.com", &clock, &key, limits)
        .await
        .expect("new revision-bound proof");
    let before = state(&pool).await;
    assert_eq!((before.0, before.1, before.3), (1, 1, 1));
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "blocklist@example.com",
            &proof,
            customer_passwords::PasswordCandidate {
                password: "cafe\u{301} password is blocked",
                blocklist: Some(&current)
            },
            &clock,
            &key,
            limits
        )
        .await,
        Err(PasswordError::Denied)
    );
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "blocklist@example.com",
            &proof,
            customer_passwords::PasswordCandidate {
                password: "another safe password",
                blocklist: None
            },
            &clock,
            &key,
            limits
        )
        .await,
        Err(PasswordError::Unavailable)
    );
    let expired = list(999);
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "blocklist@example.com",
            &proof,
            customer_passwords::PasswordCandidate {
                password: "another safe password",
                blocklist: Some(&expired)
            },
            &clock,
            &key,
            limits
        )
        .await,
        Err(PasswordError::Unavailable)
    );
    let after = state(&pool).await;
    assert_eq!((after.0, after.1, after.3), (1, 1, 4));
    assert_eq!(after.2, before.2);
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "blocklist@example.com",
            &proof,
            customer_passwords::PasswordCandidate {
                password: " café password is blocked ",
                blocklist: Some(&current)
            },
            &clock,
            &key,
            limits
        )
        .await,
        Ok(2)
    );
    let end = state(&pool).await;
    assert_eq!((end.0, end.1, end.3), (1, 0, 5));
    assert_ne!(end.2, before.2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiration_during_account_lock_wait_rolls_back_then_fresh_snapshot_allows_retry(
    pool: PgPool,
) {
    let clock = Arc::new(Time(AtomicU64::new(1000)));
    let key = AdmissionKey::new(1, [82; 32]).expect("synthetic key");
    let limits = AdmissionLimits::new(20, 20).expect("test limits");
    account(&pool, clock.as_ref(), &key, limits).await;
    let proof =
        customer_passwords::issue(&pool, "blocklist@example.com", clock.as_ref(), &key, limits)
            .await
            .expect("proof");
    let mut blocker = pool.begin().await.expect("lock owner");
    sqlx::query("SELECT id FROM verified_customer_accounts WHERE email_identity='blocklist@example.com' FOR UPDATE").execute(&mut *blocker).await.expect("hold account claim");
    let worker_pool = pool.clone();
    let worker_clock = clock.clone();
    let worker = tokio::spawn(async move {
        let result = customer_passwords::replace(
            &worker_pool,
            "blocklist@example.com",
            &proof,
            customer_passwords::PasswordCandidate {
                password: "another safe password",
                blocklist: Some(&list(1010)),
            },
            worker_clock.as_ref(),
            &key,
            limits,
        )
        .await;
        (proof, key, result)
    });
    tokio::time::timeout(std::time::Duration::from_secs(3),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%verified_customer_accounts%FOR UPDATE%')").fetch_one(&pool).await.expect("actual account wait");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("replacement reached actual account lock");
    clock.0.store(1010, Ordering::SeqCst);
    blocker.rollback().await.expect("release lock");
    let (proof, key, result) = worker.await.expect("replacement worker");
    assert_eq!(result, Err(PasswordError::Unavailable));
    let rejected = state(&pool).await;
    assert_eq!((rejected.0, rejected.1, rejected.3), (0, 1, 1));
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "blocklist@example.com",
            &proof,
            customer_passwords::PasswordCandidate {
                password: "another safe password",
                blocklist: Some(&list(3000))
            },
            clock.as_ref(),
            &key,
            limits
        )
        .await,
        Ok(1)
    );
    let complete = state(&pool).await;
    assert_eq!((complete.0, complete.1, complete.3), (1, 0, 2));
}
