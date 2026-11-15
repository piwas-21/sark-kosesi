//! One durable initial dispatch claim. No network I/O occurs in this module.
use super::*;
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::{DispatchPlan, DispatchState, plan_dispatch};
use sqlx::PgPool;
/// The sole initial dispatch capability is neither cloneable nor serializable.
/// Losing it leaves a durable Started fact: recovery must reconcile, never re-POST.
pub struct DispatchPermit {
    frozen: FrozenAttempt,
    checked_at: i64,
    proof: Box<eligibility::Eligibility>,
}
impl DispatchPermit {
    /// Consume once immediately before transport I/O, after releasing every DB claim.
    /// Failure retains durable uncertainty and cannot grant a replacement capability.
    pub fn into_request(
        mut self,
        clock: &dyn Clock,
        configuration: &Configuration,
    ) -> Result<FrozenAttempt, PaymentStoreError> {
        let at = now(clock)?;
        if at < self.checked_at {
            return Err(PaymentStoreError::ClockUnavailable);
        }
        initial_plan(&self.frozen, at, configuration)?;
        self.proof.check_at(at)?;
        Ok(self.frozen)
    }
}
pub enum DispatchOutcome {
    Start(DispatchPermit),
    Reconcile {
        frozen: FrozenAttempt,
        started_at: i64,
    },
}
fn now(clock: &dyn Clock) -> Result<i64, PaymentStoreError> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| PaymentStoreError::ClockUnavailable)?,
    )
    .map_err(|_| PaymentStoreError::ClockUnavailable)
}
fn initial_plan(
    frozen: &FrozenAttempt,
    at: i64,
    config: &Configuration,
) -> Result<(), PaymentStoreError> {
    match plan_dispatch(
        frozen.snapshot(),
        &DispatchState::Prepared,
        at,
        &config.identity(),
    ) {
        Ok(DispatchPlan::Start { .. }) => Ok(()),
        _ => Err(PaymentStoreError::Unavailable),
    }
}
/// Trusted internal worker entry. Not guest HTTP authority and not payment success.
/// Current source/stock claims protect only a NEW start. Existing starts remain
/// reconcilable after guest, catalogue, hold or configuration expiry/withdrawal.
pub async fn start_dispatch(
    pool: &PgPool,
    clock: &dyn Clock,
    identifiers: &dyn Identifiers,
    attempt_id: &str,
    configuration: &Configuration,
) -> Result<DispatchOutcome, PaymentStoreError> {
    if !identifier(attempt_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let clock = ObservedClock::new(
        clock,
        clock
            .unix_seconds()
            .map_err(|_| PaymentStoreError::ClockUnavailable)?,
    );
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s';")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s';")
        .execute(&mut *tx)
        .await?;
    // Serialize all contenders on the immutable attempt, before inspecting starts.
    let owner: String = sqlx::query_scalar(
        "SELECT left(basket_id,33) FROM payment_attempts WHERE id=$1 FOR UPDATE",
    )
    .bind(attempt_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(PaymentStoreError::NotFound)?;
    let frozen = stored::by_id(
        &mut tx,
        &owner,
        attempt_id,
        checkout_orders::OrderAccess::Internal,
    )
    .await?;
    let previous: Option<(String, i64)> = sqlx::query_as(
        "SELECT left(id,33),started_at FROM payment_dispatch_starts WHERE attempt_id=$1",
    )
    .bind(attempt_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, started_at)) = previous {
        let at = now(&clock)?;
        let state = DispatchState::Started { started_at };
        if !identifier(&id) || state.validate(frozen.snapshot()).is_err() {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        if at < started_at || at > commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS {
            return Err(PaymentStoreError::ClockUnavailable);
        }
        tx.rollback().await?;
        return Ok(DispatchOutcome::Reconcile { frozen, started_at });
    }
    crate::checkout_cancellations::ensure_not_canceled(
        &mut tx,
        &frozen.snapshot().binding.order_id,
    )
    .await?;
    let order = checkout_orders::snapshot_in_transaction(
        &mut tx,
        &owner,
        &frozen.snapshot().binding.order_id,
    )
    .await?;
    initial_plan(&frozen, now(&clock)?, configuration)?;
    let mut proof = eligibility::claim(&mut tx, &clock, &order).await?;
    let started_at = now(&clock)?;
    proof.check_at(started_at)?;
    initial_plan(&frozen, started_at, configuration)?;
    // A one-use POST capability must not outlive a start whose WAL was only
    // acknowledged asynchronously. This setting remains local to this write.
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await?;
    let dispatch_id = new_id(identifiers)?;
    let event_id = new_id(identifiers)?;
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,$2,$3)")
        .bind(attempt_id)
        .bind(&dispatch_id)
        .bind(started_at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reconciliation_required.v1',$3,$4)")
        .bind(event_id).bind(&order.identity.id).bind(serde_json::json!({"schema_version":1,"order_id":order.identity.id,"attempt_id":attempt_id,"dispatch_id":dispatch_id})).bind(started_at).execute(&mut *tx).await?;
    let (fsync, synchronous_commit): (String, String) = sqlx::query_as(
        "SELECT pg_catalog.current_setting('fsync'),pg_catalog.current_setting('synchronous_commit')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if (fsync.as_str(), synchronous_commit.as_str()) != ("on", "on") {
        return Err(PaymentStoreError::Unavailable);
    }
    let at = now(&clock)?;
    proof.check_at(at)?;
    initial_plan(&frozen, at, configuration)?;
    tx.commit().await?;
    Ok(DispatchOutcome::Start(DispatchPermit {
        frozen,
        checked_at: at,
        proof: Box::new(proof),
    }))
}
fn new_id(identifiers: &dyn Identifiers) -> Result<String, PaymentStoreError> {
    identifiers
        .next()
        .map(|v| format!("{:032x}", u128::from_be_bytes(v)))
        .map_err(|_| {
            PaymentStoreError::Boundary(checkout_orders::OrderStoreError::EntropyUnavailable)
        })
}

/// Result of one explicitly invoked private test dispatch. Neither variant grants
/// payment, stock, redirect or fulfilment authority.
#[derive(Debug, PartialEq, Eq)]
pub enum InitialDispatchOutcome {
    Observed,
    Reconcile,
}
/// Once a start commits, any later failure is uncertain, including a lost
/// observation commit. The durable recovery event retains the original attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum InitialDispatchError {
    /// Start failed; a database commit error can still leave a durable start.
    /// Retry only this entry, which rechecks the serialized start before I/O.
    Start(PaymentStoreError),
    Uncertain,
}
/// Start once, POST once, GET the returned ID and retain only authenticated facts.
/// This trusted internal entry installs no route, scheduler, credentials or worker.
pub async fn dispatch_and_observe_test_payment(
    pool: &PgPool,
    clock: &dyn Clock,
    identifiers: &dyn Identifiers,
    client: &crate::mollie::VerifiedTestClient,
    attempt_id: &str,
) -> Result<InitialDispatchOutcome, InitialDispatchError> {
    let outcome = start_dispatch(pool, clock, identifiers, attempt_id, client.configuration())
        .await
        .map_err(InitialDispatchError::Start)?;
    let DispatchOutcome::Start(permit) = outcome else {
        return Ok(InitialDispatchOutcome::Reconcile);
    };
    // The binding comes exclusively from the committed permit, not a caller or
    // the create response. Neither the ID hint nor any checkout URL is retained.
    let snapshot = permit.frozen.snapshot().clone();
    let hint = client
        .create(permit, clock)
        .await
        .map_err(|_| InitialDispatchError::Uncertain)?;
    let verified = client
        .fetch(&snapshot, hint.provider_id())
        .await
        .map_err(|_| InitialDispatchError::Uncertain)?;
    let initial = hint
        .bind(verified)
        .map_err(|_| InitialDispatchError::Uncertain)?;
    observations::record_initial(pool, clock, identifiers, attempt_id, initial)
        .await
        .map_err(|_| InitialDispatchError::Uncertain)?;
    Ok(InitialDispatchOutcome::Observed)
}
