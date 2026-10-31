use super::*;
use crate as infrastructure;
use crate::MIGRATOR;
use sqlx::Executor;
use std::sync::atomic::{AtomicU64, Ordering};
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
#[path = "basket_holds_fixture.rs"]
pub(super) mod fixture;
#[sqlx::test(migrator = "MIGRATOR")]
async fn caller_owns_commit_with_complete_order_side_effect_rollback(pool: PgPool) {
    fixture::seed(&pool, 10, 4).await;
    let lines = vec![
        BasketHoldLine {
            id: "00000000000000000000000000000001".into(),
            sku: "SET-COVERS".into(),
            revision: 1,
            units: 1,
        },
        BasketHoldLine {
            id: "00000000000000000000000000000002".into(),
            sku: "SET-FILLED".into(),
            revision: 1,
            units: 1,
        },
    ];
    for commit in [false, true] {
        let mut tx = pool.begin().await.expect("caller transaction");
        tx.execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .await
            .expect("fresh transaction");
        limits(&mut tx).await.expect("bounds");
        let receipt = reserve_in_transaction(
            &mut tx,
            &Time(AtomicU64::new(1100)),
            BasketHoldRequest {
                id: "11111111111111111111111111111111",
                owner_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                created_at: 1100,
                expires_at: 1200,
                lines: &lines,
            },
        )
        .await
        .expect("all reservations staged");
        assert!(!receipt.replayed);
        assert_eq!(
            fixture::counts(&pool).await,
            (0, 0, 0, 0),
            "no independent inner commit"
        );
        if commit {
            tx.commit().await.expect("outer order commit");
            assert_eq!(fixture::counts(&pool).await, (1, 2, 2, 3));
        } else {
            tx.rollback().await.expect("later order failure rollback");
            assert_eq!(fixture::counts(&pool).await, (0, 0, 0, 0));
            assert_eq!(
                fixture::balances(&pool).await,
                [
                    ("SEAT-COVER".into(), 10, 0, 0),
                    ("SEAT-FOAM".into(), 4, 0, 0)
                ]
            );
        }
    }
    fixture::conserved(&pool).await;
}
