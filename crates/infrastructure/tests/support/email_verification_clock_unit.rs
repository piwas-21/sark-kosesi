use super::{Consumption, PURPOSE, VerificationError, VerificationToken, consume_in_transaction};
use crate::MIGRATOR;
use commerce_application::request_context::{Clock, ContextError};
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};

struct SequenceClock {
    values: &'static [u64],
    next: AtomicUsize,
}
impl SequenceClock {
    fn new(values: &'static [u64]) -> Self {
        Self {
            values,
            next: AtomicUsize::new(0),
        }
    }
}
impl Clock for SequenceClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.values
            .get(self.next.fetch_add(1, Ordering::SeqCst))
            .copied()
            .ok_or(ContextError::ClockUnavailable)
    }
}

#[test]
fn consumption_guard_rejects_backwards_and_expired_clock() {
    let guard = Consumption {
        identity: "owner@example.com".to_owned(),
        verifier: [7; 32],
        captured: 1000,
        expires: 1900,
    };
    assert!(matches!(
        guard.check(&SequenceClock::new(&[999])),
        Err(VerificationError::Unavailable)
    ));
    assert!(matches!(
        guard.check(&SequenceClock::new(&[1900])),
        Err(VerificationError::Denied)
    ));
    assert!(guard.check(&SequenceClock::new(&[1899])).is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn private_consumer_rejects_clock_reversal_after_row_claim(pool: PgPool) {
    let token = VerificationToken::from_bytes([7; 32]);
    sqlx::query("INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at) VALUES ($1,$2,$3,1000,1900)")
        .bind(token.verifier().as_slice()).bind(PURPOSE).bind("owner@example.com")
        .execute(&pool).await.expect("insert valid proof");
    let mut tx = pool.begin().await.expect("begin claim");
    assert!(matches!(
        consume_in_transaction(
            &mut tx,
            "owner@example.com",
            &token,
            &SequenceClock::new(&[1001, 1000])
        )
        .await,
        Err(VerificationError::Unavailable)
    ));
    tx.rollback().await.expect("release claim");
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("proof state");
    assert_eq!(consumed, None);

    let mut tx = pool.begin().await.expect("retry transaction");
    let guard = consume_in_transaction(
        &mut tx,
        "owner@example.com",
        &token,
        &SequenceClock::new(&[1001, 1001]),
    )
    .await
    .expect("healthy claim");
    guard
        .check(&SequenceClock::new(&[1001]))
        .expect("healthy final check");
    tx.commit().await.expect("commit consumption");
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM email_verification_challenges")
            .fetch_one(&pool)
            .await
            .expect("consumed proof");
    assert_eq!(consumed, Some(1001));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issuance_rolls_back_after_rewind_or_expiry_at_the_final_fence(pool: PgPool) {
    use super::{AdmissionKey, AdmissionLimits, issue};
    let key = AdmissionKey::new(1, [8; 32]).expect("fixture admission key");
    let limits = AdmissionLimits::new(100, 10).expect("fixture admission limits");
    // Four admission reads precede creation and the final issuance observation.
    for sequence in [
        &[1000, 1000, 1000, 1000, 1000, 999][..],
        &[1000, 1000, 1000, 1000, 1000, 1900][..],
    ] {
        assert!(matches!(
            issue(
                &pool,
                "owner@example.com",
                &SequenceClock::new(sequence),
                &key,
                limits,
            )
            .await,
            Err(VerificationError::Unavailable)
        ));
        let proof_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM email_verification_challenges")
                .fetch_one(&pool)
                .await
                .expect("proof count");
        assert_eq!(proof_count, 0);
    }
    issue(
        &pool,
        "owner@example.com",
        &SequenceClock::new(&[1000; 6]),
        &key,
        limits,
    )
    .await
    .expect("healthy issue after both failures");
    let custody: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM email_verification_challenges),
         (SELECT sum(admitted)::bigint FROM customer_credential_admission_scopes)",
    )
    .fetch_one(&pool)
    .await
    .expect("proof and admission custody");
    assert_eq!(custody, (1, 3));
}
