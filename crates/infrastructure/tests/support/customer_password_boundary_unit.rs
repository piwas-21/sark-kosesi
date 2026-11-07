use super::*;
use crate::MIGRATOR;
use argon2::{
    Argon2,
    password_hash::{PasswordVerifier, phc::PasswordHash},
};
use commerce_application::request_context::ContextError;

struct FixedClock(Result<u64, ContextError>);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
    }
}

#[test]
fn clock_and_token_boundary_fail_closed() {
    assert_eq!(now(&FixedClock(Ok(0))), Ok(0));
    assert_eq!(now(&FixedClock(Ok(i64::MAX as u64))), Ok(i64::MAX));
    assert_eq!(
        now(&FixedClock(Ok(u64::MAX))),
        Err(PasswordError::Unavailable)
    );
    assert_eq!(
        now(&FixedClock(Err(ContextError::ClockUnavailable))),
        Err(PasswordError::Unavailable)
    );
    let token = PasswordChangeToken::from_bytes([42; 32]);
    let other = PasswordChangeToken::from_bytes([43; 32]);
    assert_eq!(token.expose_bytes(), &[42; 32]);
    assert_ne!(token.verifier(), other.verifier());
    assert_eq!(format!("{:?}", PasswordError::Denied), "Denied");
    assert_eq!(format!("{:?}", PasswordError::Unavailable), "Unavailable");
}

#[tokio::test]
async fn password_boundary_checks_normalized_length_and_argon2_parameters() {
    let _fixture = TEST_HASH_FIXTURE.lock().await;
    for rejected in [
        "abcdefghijklmn".to_owned(),
        "e\u{301}".repeat(14),
        "a".repeat(257),
        "\u{0344}".repeat(65),
    ] {
        assert!(matches!(
            hash::hash_password(&rejected).await,
            Err(PasswordError::Denied)
        ));
    }

    let raw = "  cafe\u{301} phrase long  ";
    let encoded = hash::hash_password(raw).await.expect("valid password");
    assert!(encoded.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    let parsed = PasswordHash::new(&encoded).expect("valid PHC");
    assert_eq!(parsed.salt.as_ref().expect("salt").len(), 16);
    assert_eq!(parsed.hash.as_ref().expect("digest").len(), 32);
    assert!(
        Argon2::default()
            .verify_password("  café phrase long  ".as_bytes(), &parsed)
            .is_ok()
    );
    assert!(
        Argon2::default()
            .verify_password(raw.as_bytes(), &parsed)
            .is_err()
    );
    assert!(
        Argon2::default()
            .verify_password("café phrase long".as_bytes(), &parsed)
            .is_err()
    );
    let second = hash::hash_password(raw).await.expect("valid password");
    assert!(
        encoded != second,
        "independent hashes must have different salts"
    );
    for accepted in ["abcdefghijklmno".to_owned(), "😀".repeat(64)] {
        let encoded = hash::hash_password(&accepted)
            .await
            .expect("boundary password");
        let parsed = PasswordHash::new(&encoded).expect("valid PHC");
        assert!(
            Argon2::default()
                .verify_password(accepted.as_bytes(), &parsed)
                .is_ok()
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn current_revision_rejects_corrupt_rows_and_database_errors(pool: PgPool) {
    let mut tx = begin(&pool).await.expect("isolated transaction");
    sqlx::query("CREATE TEMP TABLE customer_password_credentials (account_id BIGINT, revision INTEGER) ON COMMIT DROP")
        .execute(&mut *tx).await.expect("isolated temporary table");
    assert_eq!(current_revision(&mut tx, 1).await, Ok(0));
    sqlx::query("INSERT INTO customer_password_credentials VALUES (1, 4), (2, 0), (3, -1)")
        .execute(&mut *tx)
        .await
        .expect("fixture rows");
    assert_eq!(current_revision(&mut tx, 1).await, Ok(4));
    assert_eq!(
        current_revision(&mut tx, 2).await,
        Err(PasswordError::Unavailable)
    );
    assert_eq!(
        current_revision(&mut tx, 3).await,
        Err(PasswordError::Unavailable)
    );
    sqlx::query("ALTER TABLE customer_password_credentials DROP COLUMN revision")
        .execute(&mut *tx)
        .await
        .expect("simulate invalid stored schema");
    assert_eq!(
        current_revision(&mut tx, 1).await,
        Err(PasswordError::Unavailable)
    );
}
