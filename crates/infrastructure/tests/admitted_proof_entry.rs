//! The private proof entry points commit an admission debit before work.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_passwords::{self, PasswordChangeToken, PasswordError},
    email_verification::{self, VerificationError, VerificationToken},
    verified_customer_accounts,
};
use sqlx::PgPool;

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}

async fn admitted(pool: &PgPool, purpose: i16) -> i32 {
    sqlx::query_scalar("SELECT admitted FROM customer_credential_admission_scopes WHERE purpose=$1")
        .bind(purpose)
        .fetch_one(pool)
        .await
        .expect("purpose debit count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn verification_debits_before_storage_and_shares_consume_quota_with_enrolment(pool: PgPool) {
    let key = AdmissionKey::new(1, [27; 32]).expect("test key");
    let limits = AdmissionLimits::new(4, 1).expect("test limits");
    let time = At(1000);
    let proof = email_verification::issue(&pool, "Owner@example.com", &time, &key, limits)
        .await
        .expect("exact proof");
    sqlx::query("ALTER TABLE email_verification_challenges RENAME TO unavailable_verification")
        .execute(&pool)
        .await
        .expect("interrupt proof storage");
    assert!(matches!(
        email_verification::issue(&pool, "Owner@EXAMPLE.COM", &time, &key, limits).await,
        Err(VerificationError::Denied)
    ));
    assert!(matches!(
        email_verification::issue(&pool, "other@example.com", &time, &key, limits).await,
        Err(VerificationError::Unavailable)
    ));
    assert_eq!(admitted(&pool, 2).await, 2);
    sqlx::query("ALTER TABLE unavailable_verification RENAME TO email_verification_challenges")
        .execute(&pool)
        .await
        .expect("restore proof storage");

    let wrong = VerificationToken::from_bytes([12; 32]);
    assert_eq!(
        email_verification::consume(&pool, "Owner@example.com", &wrong, &time, &key, limits).await,
        Err(VerificationError::Denied)
    );
    assert!(matches!(
        verified_customer_accounts::establish(
            &pool,
            "Owner@EXAMPLE.COM",
            &proof,
            &time,
            &key,
            limits
        )
        .await,
        Err(VerificationError::Denied)
    ));
    let effects: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM verified_customer_accounts),
                (SELECT count(consumed_at) FROM email_verification_challenges)",
    )
    .fetch_one(&pool)
    .await
    .expect("atomic effects");
    assert_eq!(effects, (0, 0));
    assert_eq!(admitted(&pool, 3).await, 1);
    verified_customer_accounts::establish(
        &pool,
        "Owner@example.com",
        &proof,
        &At(1020),
        &key,
        limits,
    )
    .await
    .expect("enrolment succeeds with one debit after rollover");
    assert_eq!(admitted(&pool, 3).await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn password_proof_and_replacement_have_independent_durable_budgets(pool: PgPool) {
    let key = AdmissionKey::new(1, [27; 32]).expect("test key");
    let limits = AdmissionLimits::new(4, 1).expect("test limits");
    let time = At(1000);
    let email = "owner@example.com";
    let proof = email_verification::issue(&pool, email, &time, &key, limits)
        .await
        .expect("email proof");
    verified_customer_accounts::establish(&pool, email, &proof, &time, &key, limits)
        .await
        .expect("verified identity");
    let proof = customer_passwords::issue(&pool, email, &time, &key, limits)
        .await
        .expect("password proof");
    assert!(matches!(
        customer_passwords::issue(&pool, "unknown@example.com", &time, &key, limits).await,
        Err(PasswordError::Denied)
    ));
    assert_eq!(admitted(&pool, 4).await, 2);

    let wrong = PasswordChangeToken::from_bytes([12; 32]);
    assert_eq!(
        customer_passwords::replace(
            &pool,
            email,
            &wrong,
            "correct-password-long",
            &time,
            &key,
            limits,
        )
        .await,
        Err(PasswordError::Denied)
    );
    assert_eq!(admitted(&pool, 5).await, 1);
    sqlx::query("ALTER TABLE customer_password_challenges RENAME TO unavailable_password_proofs")
        .execute(&pool)
        .await
        .expect("interrupt password proof lookup");
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "owner@EXAMPLE.COM",
            &proof,
            "correct-password-long",
            &time,
            &key,
            limits,
        )
        .await,
        Err(PasswordError::Denied)
    );
    assert_eq!(
        customer_passwords::replace(
            &pool,
            "other@example.com",
            &proof,
            "correct-password-long",
            &time,
            &key,
            limits,
        )
        .await,
        Err(PasswordError::Unavailable)
    );
    assert_eq!(admitted(&pool, 5).await, 2);
    sqlx::query("ALTER TABLE unavailable_password_proofs RENAME TO customer_password_challenges")
        .execute(&pool)
        .await
        .expect("restore password proof lookup");
    let effects: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM customer_password_credentials),
                (SELECT count(consumed_at) FROM customer_password_challenges)",
    )
    .fetch_one(&pool)
    .await
    .expect("unchanged proof and credential");
    assert_eq!(effects, (0, 0));
    assert_eq!(
        customer_passwords::replace(
            &pool,
            email,
            &proof,
            "correct-password-long",
            &At(1020),
            &key,
            limits,
        )
        .await,
        Ok(1)
    );
    assert_eq!(admitted(&pool, 5).await, 1);
    assert_eq!(admitted(&pool, 4).await, 2);
}
