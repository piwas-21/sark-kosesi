//! Exact private historical inspection; the result grants no disposition authority.
use super::{checkout_orders, identifier, post_dispatch};
use crate::{
    payment_attempts::{PaymentStoreError, evidence},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, PartialEq, Eq)]
pub struct CancellationInspection {
    pub order_id: String,
    pub requested_at: i64,
    pub request_digest: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InspectionError {
    InvalidRequest,
    Denied,
    ClockUnavailable,
    InvalidStoredData,
    Database,
}
impl From<sqlx::Error> for InspectionError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
fn staff(error: StaffError) -> InspectionError {
    match error {
        StaffError::Denied | StaffError::InvalidRequest => InspectionError::Denied,
        StaffError::ClockUnavailable => InspectionError::ClockUnavailable,
        _ => InspectionError::Database,
    }
}
fn order_error(error: checkout_orders::OrderStoreError) -> InspectionError {
    match error {
        checkout_orders::OrderStoreError::Database => InspectionError::Database,
        _ => InspectionError::InvalidStoredData,
    }
}
fn payment(error: PaymentStoreError) -> InspectionError {
    match error {
        PaymentStoreError::Database
        | PaymentStoreError::Boundary(checkout_orders::OrderStoreError::Database) => {
            InspectionError::Database
        }
        _ => InspectionError::InvalidStoredData,
    }
}

/// Inspect one exact historical customer request under live recent-MFA authority.
/// Absence and malformed custody are disclosed only after the final authority check.
pub async fn inspect_post_dispatch_request(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order_id: &str,
) -> Result<Option<CancellationInspection>, InspectionError> {
    if !identifier(order_id) {
        return Err(InspectionError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::CancellationReview)
        .await
        .map_err(staff)?;
    let result = inspect_locked(&mut access.tx, order_id).await;
    let at = access.authorized_at(&observed).map_err(staff)?;
    if result
        .as_ref()
        .is_ok_and(|row| row.as_ref().is_some_and(|value| at < value.requested_at))
    {
        return Err(InspectionError::ClockUnavailable);
    }
    drop(access);
    result
}

// One statement captures absence consistently, including an orphan event.
async fn index(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<(Option<String>, bool, bool), InspectionError> {
    Ok(sqlx::query_as("SELECT (SELECT left(id,33) FROM public.payment_attempts WHERE order_id=$1),EXISTS(SELECT 1 FROM public.checkout_post_dispatch_requests WHERE order_id=$1),EXISTS(SELECT 1 FROM public.commerce_outbox WHERE aggregate_id=$1 AND kind='checkout.cancellation_requested.v1')")
        .bind(order).fetch_one(&mut **tx).await?)
}

async fn inspect_locked(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<CancellationInspection>, InspectionError> {
    let (attempt, request, event) = index(tx, order).await?;
    let Some(attempt) = attempt else {
        return if request || event {
            Err(InspectionError::InvalidStoredData)
        } else {
            Ok(None)
        };
    };
    if !identifier(&attempt) {
        return Err(InspectionError::InvalidStoredData);
    }
    crate::order_stock::source::claim_attempt(tx, &attempt)
        .await
        .map_err(|_| InspectionError::Database)?;
    let (current, request, event) = index(tx, order).await?;
    if current.as_deref() != Some(attempt.as_str()) {
        return Err(InspectionError::InvalidStoredData);
    }
    if !request {
        return if event {
            Err(InspectionError::InvalidStoredData)
        } else {
            Ok(None)
        };
    }
    let (frozen, _dispatch, _started_at) = evidence::context_with_start(tx, &attempt, false)
        .await
        .map_err(payment)?;
    let binding = &frozen.snapshot().binding;
    // The locked unique order attempt and frozen decoder already bind these identities.
    let (_, account) = checkout_orders::snapshot_with_account_in_transaction(
        tx,
        &binding.basket_id,
        order,
        checkout_orders::OrderAccess::Internal,
    )
    .await
    .map_err(order_error)?;
    let row = post_dispatch::saved(tx, order, &binding.basket_id, account)
        .await
        .map_err(order_error)?
        .ok_or(InspectionError::InvalidStoredData)?;
    // saved() checks the source function's exact order, attempt, start and event.
    // The validated source requires this exact timestamp and payload; the
    // unique outbox constraint permits only one recovery event for the order.
    let recovery: String = sqlx::query_scalar("SELECT left(id,33) FROM public.commerce_outbox WHERE aggregate_id=$1 AND kind='payment.reconciliation_required.v1'")
        .bind(order).fetch_optional(&mut **tx).await?.ok_or(InspectionError::InvalidStoredData)?;
    if !identifier(&recovery) {
        return Err(InspectionError::InvalidStoredData);
    }
    let digest: String = sqlx::query_scalar("SELECT public.checkout_post_dispatch_request_digest(r) FROM public.checkout_post_dispatch_requests r WHERE r.order_id=$1")
        .bind(order).fetch_one(&mut **tx).await?;
    Ok(Some(CancellationInspection {
        order_id: order.into(),
        requested_at: row.requested_at,
        request_digest: digest,
    }))
}
