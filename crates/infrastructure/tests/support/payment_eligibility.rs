//! The private composition contract rejects changed accepted commercial snapshots.
use super::*;
use crate::MIGRATOR;
use crate::payment_test_fixture as fixture;
use sqlx::PgPool;
#[sqlx::test(migrator = "MIGRATOR")]
async fn current_sources_must_equal_the_accepted_money_and_merchandise_with_exact_review(
    pool: PgPool,
) {
    let (_, time, _, _, order) = fixture::accepted(&pool).await;
    for mode in 0..4 {
        let mut candidate = order.clone();
        match mode {
            0 => candidate.quote.lines[0].unit_gross_minor = 12344,
            1 => candidate.products[0].title = "Different accepted merchandise".into(),
            2 => candidate.quote.binding.policy_review_revision = 2,
            _ => candidate.market.evidence_ref = "Different policy acceptance".into(),
        }
        let mut tx = pool.begin().await.expect("caller transaction");
        assert!(matches!(
            claim(&mut tx, time.as_ref(), &candidate).await,
            Err(PaymentStoreError::Unavailable)
        ));
        tx.rollback().await.expect("no new authority");
    }
    let mut tx = pool.begin().await.expect("positive transaction");
    let mut proof = claim(&mut tx, time.as_ref(), &order)
        .await
        .expect("exact original source");
    assert!(proof.check_at(&order, 1500).is_ok());
    tx.rollback().await.expect("read claims only");
    assert_eq!(fixture::counts(&pool).await, (0, 1, 1, 1, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_locale_seller_deadline_and_clock_outages_never_grant_source_authority(
    pool: PgPool,
) {
    use commerce_application::request_context::{Clock, ContextError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Interrupted {
        remaining: AtomicUsize,
        overflow: bool,
    }
    impl Clock for Interrupted {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            if self
                .remaining
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
                .is_err()
            {
                if self.overflow {
                    Ok(u64::MAX)
                } else {
                    Err(ContextError::ClockUnavailable)
                }
            } else {
                Ok(1500)
            }
        }
    }
    let (_, time, _, _, order) = fixture::accepted(&pool).await;
    let mut invalid = order.clone();
    invalid.quote.binding.locale = "unknown".into();
    let mut tx = pool.begin().await.expect("caller");
    assert!(matches!(
        claim(&mut tx, time.as_ref(), &invalid).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("caller");
    let mut proof = claim(&mut tx, time.as_ref(), &order)
        .await
        .expect("positive source");
    let mut expired = order.clone();
    expired.seller.expires_at = 1600;
    assert!(matches!(
        proof.check_at(&expired, 1600),
        Err(PaymentStoreError::Unavailable)
    ));
    tx.rollback().await.expect("rollback");
    for overflow in [false, true] {
        let mut denied = 0;
        let mut permitted = 0;
        for allowance in 0..32 {
            let clock = Interrupted {
                remaining: AtomicUsize::new(allowance),
                overflow,
            };
            let mut tx = pool.begin().await.expect("caller");
            let result = claim(&mut tx, &clock, &order).await;
            if result.is_err() {
                denied += 1;
            } else {
                permitted += 1;
            }
            tx.rollback().await.expect("read-only rollback");
        }
        assert!(
            denied > 0 && permitted > 0,
            "clock outage negatives and healthy positive control"
        );
    }
    assert_eq!(fixture::counts(&pool).await, (0, 1, 1, 1, 2));
}
