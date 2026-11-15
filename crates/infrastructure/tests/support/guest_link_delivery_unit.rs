use super::*;
use commerce_application::request_context::ContextError;

struct FailingClock;
impl Clock for FailingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Err(ContextError::ClockUnavailable)
    }
}
struct ExcessiveClock;
impl Clock for ExcessiveClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(u64::MAX)
    }
}
#[test]
fn guest_order_link_delivery_clock_rejects_failure_and_out_of_range_time() {
    assert_eq!(now(&FailingClock), Err(LinkError::Unavailable));
    assert_eq!(now(&ExcessiveClock), Err(LinkError::Unavailable));
}
#[sqlx::test(migrator = "crate::MIGRATOR")]
async fn guest_order_link_delivery_refuses_async_commit_with_positive_control(pool: PgPool) {
    let mut tx = begin_durable(&pool).await.expect("transaction");
    sqlx::query("SET LOCAL synchronous_commit='off'")
        .execute(&mut *tx)
        .await
        .expect("fault");
    assert_eq!(durable(&mut tx).await, Err(LinkError::Unavailable));
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("restore");
    assert_eq!(durable(&mut tx).await, Ok(()));
}
