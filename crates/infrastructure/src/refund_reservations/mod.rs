//! Private reservation of captured amounts; no provider, credit, or stock effect.
use crate::{
    payment_attempts::{identifier, recognition},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{
    request_context::{Clock, Identifiers, ObservedClock},
    staff_access::Capability,
};
use commerce_domain::{Currency, checkout_orders::OrderSnapshot};
use sqlx::{PgPool, Postgres, Transaction};
mod allocation;
mod store;
pub use allocation::{RefundAllocation, RefundAllocationAmounts, RefundAllocationLine};

pub struct RefundRequest<'a> {
    pub order_id: &'a str,
    pub expected_recognition_id: &'a str,
    pub effect_key: &'a str,
    pub lines: &'a [RefundLine<'a>],
    pub shipping_gross_minor: i64,
    pub reason: &'a str,
    pub confirmed: bool,
}
pub struct RefundLine<'a> {
    pub sku: &'a str,
    pub gross_minor: i64,
}
#[derive(PartialEq, Eq)]
pub struct RefundReceipt {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub recognition_id: String,
    pub actor_id: String,
    pub effect_key: String,
    pub generation: i16,
    pub reason: String,
    pub reserved_at: i64,
    pub allocation: RefundAllocation,
    pub replayed: bool,
}
impl std::fmt::Debug for RefundReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefundReceipt")
            .field("id", &self.id)
            .field("order_id", &self.order_id)
            .field("generation", &self.generation)
            .field("reason", &"[redacted]")
            .field("allocation", &self.allocation)
            .field("replayed", &self.replayed)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefundError {
    InvalidRequest,
    Denied,
    Conflict,
    Unavailable,
    InvalidStoredData,
    ClockUnavailable,
    EntropyUnavailable,
    Database,
}
impl From<sqlx::Error> for RefundError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
fn staff(error: StaffError) -> RefundError {
    match error {
        StaffError::Denied | StaffError::InvalidRequest => RefundError::Denied,
        StaffError::ClockUnavailable => RefundError::ClockUnavailable,
        _ => RefundError::Database,
    }
}
fn fresh(ids: &dyn Identifiers) -> Result<String, RefundError> {
    ids.next()
        .map(|id| format!("{:032x}", u128::from_be_bytes(id)))
        .map_err(|_| RefundError::EntropyUnavailable)
}
fn request_valid(r: &RefundRequest<'_>) -> bool {
    identifier(r.order_id)
        && identifier(r.expected_recognition_id)
        && commerce_domain::media::valid_command_id(r.effect_key)
        && r.confirmed
        && !r.reason.is_empty()
        && r.reason.len() <= 240
        && r.reason.trim() == r.reason
        && !r.reason.chars().any(char::is_control)
        && (1..=20).contains(&r.lines.len())
        && r.shipping_gross_minor >= 0
        && r.lines
            .iter()
            .all(|l| !l.sku.is_empty() && l.sku.len() <= 65 && l.gross_minor >= 0)
}
async fn source(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
    recognition_id: Option<&str>,
) -> Result<(OrderSnapshot, recognition::Recognition), RefundError> {
    let (frozen, found) = if let Some(id) = recognition_id {
        let (f, r) = recognition::historical_in_transaction(tx, attempt, id)
            .await
            .map_err(|_| RefundError::InvalidStoredData)?;
        (f, Some(r))
    } else {
        recognition::current_in_transaction(tx, attempt)
            .await
            .map_err(|e| match e {
                crate::payment_attempts::PaymentStoreError::Unavailable => RefundError::Unavailable,
                crate::payment_attempts::PaymentStoreError::Database => RefundError::Database,
                _ => RefundError::InvalidStoredData,
            })?
    };
    let received = found
        .filter(|r| r.state == recognition::State::Received)
        .ok_or(RefundError::Unavailable)?;
    let binding = &frozen.snapshot().binding;
    let order =
        crate::checkout_orders::snapshot_in_transaction(tx, &binding.basket_id, &binding.order_id)
            .await
            .map_err(|_| RefundError::InvalidStoredData)?;
    let totals = order
        .quote
        .totals()
        .map_err(|_| RefundError::InvalidStoredData)?;
    if received.order_id != order.identity.id
        || received.attempt_id != attempt
        || received.currency != Currency::EUR
        || received.amount_minor != totals.total.gross.minor_units()
        || binding.amount_minor != received.amount_minor
        || binding.currency != Currency::EUR
    {
        return Err(RefundError::InvalidStoredData);
    }
    Ok((order, received))
}

pub async fn reserve(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    request: RefundRequest<'_>,
) -> Result<RefundReceipt, RefundError> {
    if !request_valid(&request) {
        return Err(RefundError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::RefundPrepare)
        .await
        .map_err(staff)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("refund-reservation:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior = store::lookup(&mut access.tx, request.effect_key).await?;
    let attempt = if let Some((_, attempt)) = &prior {
        attempt.clone()
    } else {
        crate::order_stock::source::attempt(&mut access.tx, request.order_id)
            .await
            .map_err(|e| match e {
                crate::order_stock::SettlementError::NotFound => RefundError::Unavailable,
                crate::order_stock::SettlementError::Database => RefundError::Database,
                _ => RefundError::InvalidStoredData,
            })?
    };
    crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(|_| RefundError::Database)?;
    access.check_time(&observed).map_err(staff)?;
    if let Some((order_id, _)) = prior {
        store::require_exact_history(&mut access.tx, &order_id).await?;
        let history = store::history(&mut access.tx, &order_id).await?;
        let receipt = history
            .iter()
            .find(|r| r.effect_key == request.effect_key)
            .ok_or(RefundError::InvalidStoredData)?;
        let (order, _) = source(&mut access.tx, &attempt, Some(&receipt.recognition_id)).await?;
        allocation::validate_history(&order, &attempt, &history)?;
        if receipt.order_id != request.order_id
            || receipt.recognition_id != request.expected_recognition_id
            || receipt.actor_id != access.actor
            || receipt.reason != request.reason
            || !allocation::same_request(&receipt.allocation, &request)
        {
            return Err(RefundError::Conflict);
        }
        let mut receipt = store::take_receipt(history, request.effect_key)?;
        access.check_time(&observed).map_err(staff)?;
        receipt.replayed = true;
        access.tx.commit().await?;
        return Ok(receipt);
    }
    let (order, received) = source(&mut access.tx, &attempt, None).await?;
    if order.identity.id != request.order_id || received.id != request.expected_recognition_id {
        return Err(RefundError::Conflict);
    }
    store::require_exact_history(&mut access.tx, request.order_id).await?;
    let history = store::history(&mut access.tx, request.order_id).await?;
    allocation::validate_history(&order, &attempt, &history)?;
    if history.len() >= 16 {
        return Err(RefundError::Conflict);
    }
    let allocation = allocation::calculate(&order, &history, &request)?;
    let at = access.authorized_at(&observed).map_err(staff)?;
    if at < received.recorded_at
        || at < order.identity.created_at
        || history.last().is_some_and(|r| at < r.reserved_at)
    {
        return Err(RefundError::ClockUnavailable);
    }
    let receipt = RefundReceipt {
        id: fresh(ids)?,
        order_id: request.order_id.into(),
        attempt_id: attempt,
        recognition_id: received.id,
        actor_id: access.actor.clone(),
        effect_key: request.effect_key.into(),
        generation: i16::try_from(history.len() + 1).map_err(|_| RefundError::InvalidStoredData)?,
        reason: request.reason.into(),
        reserved_at: at,
        allocation,
        replayed: false,
    };
    store::insert(&mut access.tx, &receipt, &fresh(ids)?).await?;
    // Complete native custody while the authority and attempt claims are held.
    // The seal prevents the checked payment source from changing before commit.
    sqlx::query("SET CONSTRAINTS require_complete_refund_reservation IMMEDIATE")
        .execute(&mut *access.tx)
        .await?;
    let written = store::history(&mut access.tx, request.order_id).await?;
    allocation::validate_history(&order, &receipt.attempt_id, &written)?;
    if written.last() != Some(&receipt) {
        return Err(RefundError::InvalidStoredData);
    }
    access.check_time(&observed).map_err(staff)?;
    access.tx.commit().await?;
    Ok(receipt)
}
