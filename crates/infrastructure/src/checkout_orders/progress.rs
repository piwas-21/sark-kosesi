//! An exact-order private read over the original checkout and payment custody.
use super::*;
use crate::{
    checkout_cancellations::{pending_cancellation_locked, pre_dispatch_canceled_at_locked},
    customer_sessions::{CustomerSessionToken, HeldSession},
    payment_attempts::{self, GrossPaymentSource, PaymentStoreError, recognition::State},
};

#[derive(Debug, PartialEq, Eq)]
pub enum OwnerOrderLifecycle {
    Accepted,
    CancelledBeforeDispatch,
}

/// Point-in-time evidence only. A gross receipt has no net or shipment meaning.
#[derive(Debug, PartialEq, Eq)]
pub enum OwnerGrossPaymentProgress {
    /// The local start evidence is absent.
    NoRecordedDispatch,
    /// A local start exists and no gross receipt has been recorded.
    NoRecognition,
    Recognized(State),
}

#[derive(Debug, PartialEq, Eq)]
pub struct OwnerOrderProgress {
    pub order: OwnerOrderLifecycle,
    /// Records an intent after dispatch. It leaves the order accepted.
    pub pending_cancellation: bool,
    pub gross_payment: OwnerGrossPaymentProgress,
}

pub async fn owned_order_progress(
    store: &GuestSessionStore,
    key: &GuestCredential,
    order: &str,
) -> Result<OwnerOrderProgress, OrderStoreError> {
    read(store, key, None, order).await
}

pub async fn owned_verified_order_progress(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &CustomerSessionToken,
    order: &str,
) -> Result<OwnerOrderProgress, OrderStoreError> {
    read(store, key, Some(customer), order).await
}

async fn read(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&CustomerSessionToken>,
    order: &str,
) -> Result<OwnerOrderProgress, OrderStoreError> {
    if !identifier(order) {
        return Err(OrderStoreError::InvalidRequest);
    }
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    let owner = scope.basket_id().to_owned();
    let account = session.as_ref().map(HeldSession::account_id);
    let result = Box::pin(read_locked(scope.transaction(), &owner, order, account)).await;
    // Fence source outcomes, including absence and errors, after every lock wait.
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    let (progress, source_at) = result?;
    if at < source_at {
        return Err(OrderStoreError::ClockUnavailable);
    }
    scope.rollback().await?;
    Ok(progress)
}

async fn read_locked(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    order: &str,
    account: Option<i64>,
) -> Result<(OwnerOrderProgress, i64), OrderStoreError> {
    let order = snapshot_for_account_in_transaction(tx, owner, order, account)
        .await?
        .identity;
    let attempt = payment_attempts::claim_owned_attempt(tx, owner, &order.id)
        .await
        .map_err(payment)?;
    let events: Vec<bool> = sqlx::query_scalar(
        "SELECT id ~ '^[a-f0-9]{32}$' AND created_at=$2
         AND payload=jsonb_build_object('schema_version',1,'order_id',$1::text)
         FROM public.commerce_outbox WHERE aggregate_id=$1 AND kind='checkout.order_created.v1' LIMIT 2",
    )
    .bind(&order.id)
    .bind(order.created_at)
    .fetch_all(&mut **tx)
    .await?;
    if events.as_slice() != [true] {
        return Err(OrderStoreError::InvalidStoredData);
    }
    let canceled = pre_dispatch_canceled_at_locked(tx, &order, account).await?;
    let pending = pending_cancellation_locked(tx, &order.id).await?;
    let gross = Box::pin(payment_attempts::owner_gross_progress_locked(
        tx,
        &order,
        account,
        attempt.as_deref(),
    ))
    .await
    .map_err(payment)?;
    // The locked cancellation readers already prove dispatch absence for a
    // completed cancellation and the exact dispatch start for a pending request.
    let (gross_payment, payment_source_at) = match gross {
        GrossPaymentSource::NoDispatch { prepared_at } => {
            (OwnerGrossPaymentProgress::NoRecordedDispatch, prepared_at)
        }
        GrossPaymentSource::Dispatch { source_at, state } => (
            state.map_or(
                OwnerGrossPaymentProgress::NoRecognition,
                OwnerGrossPaymentProgress::Recognized,
            ),
            Some(source_at),
        ),
    };
    let latest = [
        payment_source_at,
        canceled,
        pending.as_ref().map(|r| r.requested_at),
    ]
    .into_iter()
    .flatten()
    .fold(order.created_at, i64::max);
    Ok((
        OwnerOrderProgress {
            order: if canceled.is_some() {
                OwnerOrderLifecycle::CancelledBeforeDispatch
            } else {
                OwnerOrderLifecycle::Accepted
            },
            pending_cancellation: pending.is_some(),
            gross_payment,
        },
        latest,
    ))
}

fn payment(error: PaymentStoreError) -> OrderStoreError {
    match error {
        PaymentStoreError::Boundary(OrderStoreError::Database) | PaymentStoreError::Database => {
            OrderStoreError::Database
        }
        PaymentStoreError::Unavailable => OrderStoreError::Unavailable,
        _ => OrderStoreError::InvalidStoredData,
    }
}
