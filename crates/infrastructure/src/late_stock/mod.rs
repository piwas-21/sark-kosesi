//! Private recovery of the exact frozen paid component demand; no operational runner.
use crate::payment_attempts::{PaymentStoreError, recognition};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
pub use commerce_domain::late_stock::*;
use commerce_domain::{checkout_orders::OrderSnapshot, order_stock::identifier};
use sqlx::{Acquire, PgPool, Postgres, Transaction};
mod decode;
pub mod disposition;
mod engine;
mod history;
pub mod inbox;
mod index;
mod retry_engine;
mod rows;
mod source;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LateStockError {
    InvalidRequest,
    NotFound,
    Unavailable,
    InvalidStoredData,
    NotApplicable,
    StateChanged,
    ClockUnavailable,
    EntropyUnavailable,
    Database,
}
impl From<sqlx::Error> for LateStockError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<PolicyError> for LateStockError {
    fn from(error: PolicyError) -> Self {
        match error {
            PolicyError::InvalidSnapshot => Self::InvalidStoredData,
            PolicyError::InvalidClock => Self::ClockUnavailable,
            PolicyError::WindowExpired => Self::StateChanged,
        }
    }
}
impl From<crate::order_stock::SettlementError> for LateStockError {
    fn from(error: crate::order_stock::SettlementError) -> Self {
        use crate::order_stock::SettlementError as E;
        match error {
            E::InvalidRequest => Self::InvalidRequest,
            E::NotFound => Self::NotFound,
            E::Unavailable => Self::Unavailable,
            E::RecoveryRequired => Self::NotApplicable,
            E::ClockUnavailable => Self::ClockUnavailable,
            E::EntropyUnavailable => Self::EntropyUnavailable,
            E::Database => Self::Database,
            E::InvalidStoredData => Self::InvalidStoredData,
        }
    }
}
impl From<PaymentStoreError> for LateStockError {
    fn from(error: PaymentStoreError) -> Self {
        crate::order_stock::SettlementError::from(error).into()
    }
}
impl From<crate::holds::HoldWriteError> for LateStockError {
    fn from(error: crate::holds::HoldWriteError) -> Self {
        use crate::holds::HoldWriteError as E;
        match error {
            E::Database(_) => Self::Database,
            E::ClockUnavailable | E::Policy(commerce_domain::holds::HoldError::InvalidClock) => {
                Self::ClockUnavailable
            }
            E::Policy(commerce_domain::holds::HoldError::Expired) => Self::StateChanged,
            _ => Self::InvalidStoredData,
        }
    }
}
pub struct RecoveryReceipt {
    pub outcome: Outcome,
    pub replayed: bool,
}
pub async fn recover(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    order_id: &str,
) -> Result<RecoveryReceipt, LateStockError> {
    validate(order_id)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    if let Some(outcome) = rows::read(&mut tx, order_id).await? {
        tx.rollback().await?;
        return Ok(RecoveryReceipt {
            outcome,
            replayed: true,
        });
    }
    let attempt = crate::order_stock::source::attempt(&mut tx, order_id).await?;
    crate::order_stock::source::claim_attempt(&mut tx, &attempt).await?;
    if let Some(outcome) = rows::read(&mut tx, order_id).await? {
        tx.rollback().await?;
        return Ok(RecoveryReceipt {
            outcome,
            replayed: true,
        });
    }
    let outcome = engine::recover(&mut tx, clock, ids, order_id, &attempt).await?;
    tx.commit().await?;
    Ok(RecoveryReceipt {
        outcome,
        replayed: false,
    })
}
pub async fn read_recorded(
    pool: &PgPool,
    order_id: &str,
) -> Result<Option<Outcome>, LateStockError> {
    validate(order_id)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let outcome = rows::read(&mut tx, order_id).await?;
    tx.rollback().await?;
    Ok(outcome)
}
pub(crate) async fn read_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<Outcome>, LateStockError> {
    rows::read(tx, order).await
}
fn validate(id: &str) -> Result<(), LateStockError> {
    if !identifier(id) {
        return Err(LateStockError::InvalidRequest);
    }
    Ok(())
}
async fn limits(tx: &mut Transaction<'_, Postgres>) -> Result<(), LateStockError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut **tx)
        .await?;
    #[cfg(test)]
    crate::late_stock_wait_budget::apply(tx).await?;
    Ok(())
}
fn now(clock: &dyn Clock) -> Result<i64, LateStockError> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| LateStockError::ClockUnavailable)?,
    )
    .map_err(|_| LateStockError::ClockUnavailable)
}
fn fresh(ids: &dyn Identifiers) -> Result<String, LateStockError> {
    ids.next()
        .map(|id| format!("{:032x}", u128::from_be_bytes(id)))
        .map_err(|_| LateStockError::EntropyUnavailable)
}
struct Context {
    order: OrderSnapshot,
    received: recognition::Recognition,
}
async fn current(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
    attempt: &str,
) -> Result<Context, LateStockError> {
    let (frozen, received) = recognition::current_in_transaction(tx, attempt).await?;
    let received = received
        .filter(|receipt| receipt.state == recognition::State::Received)
        .ok_or(LateStockError::Unavailable)?;
    if frozen.snapshot().binding.order_id != order {
        return Err(LateStockError::InvalidStoredData);
    }
    let order = crate::checkout_orders::snapshot_in_transaction(
        tx,
        &frozen.snapshot().binding.basket_id,
        order,
    )
    .await
    .map_err(|error| match error {
        crate::checkout_orders::OrderStoreError::Database => LateStockError::Database,
        _ => LateStockError::InvalidStoredData,
    })?;
    Ok(Context { order, received })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MIGRATOR, mollie::tests::order_stock_cases::fixture as orders};

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn late_stock_current_source_requires_the_exact_order_for_a_received_attempt(
        pool: PgPool,
    ) {
        let fixture = orders::accepted(&pool, false, true).await;
        let order = &fixture.order.identity.id;
        let attempt = &fixture.frozen.snapshot().binding.id;
        let foreign = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_ne!(order, foreign);
        let mut tx = pool.begin().await.expect("inspect current paid source");
        assert!(matches!(
            current(&mut tx, foreign, attempt).await,
            Err(LateStockError::InvalidStoredData)
        ));
        let bound = current(&mut tx, order, attempt)
            .await
            .expect("same received attempt authorizes its exact order");
        assert_eq!(&bound.order.identity.id, order);
        tx.rollback().await.expect("read-only transaction");
    }
}

#[cfg(test)]
mod wait_budget_tests {
    use super::limits;
    use crate::{MIGRATOR, late_stock_wait_budget};
    use sqlx::PgPool;

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn wait_budget_is_explicit_task_scoped_and_transaction_local(pool: PgPool) {
        let mut tx = pool.begin().await.expect("default transaction");
        limits(&mut tx).await.expect("default limits");
        assert_eq!(
            late_stock_wait_budget::settings(&mut tx).await,
            ("1s".into(), "2s".into())
        );
        tx.rollback().await.expect("release");
        late_stock_wait_budget::scope(async {
            let mut tx = pool.begin().await.expect("scoped transaction");
            limits(&mut tx).await.expect("scoped limits");
            assert_eq!(
                late_stock_wait_budget::settings(&mut tx).await,
                ("10s".into(), "20s".into())
            );
            tx.rollback().await.expect("release");
        })
        .await;
        let mut tx = pool.begin().await.expect("post-scope transaction");
        limits(&mut tx).await.expect("post-scope limits");
        assert_eq!(
            late_stock_wait_budget::settings(&mut tx).await,
            ("1s".into(), "2s".into())
        );
        tx.rollback().await.expect("release");
    }
}
