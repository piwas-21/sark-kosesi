//! Owner-scoped point-in-time dispatch custody; no payment or disposition authority.
use super::*;
use crate::customer_sessions::CustomerSessionToken;

/// A validated local start and its reconciliation event were recorded.
/// This does not assert that a request reached the provider or that payment occurred.
#[derive(Debug, PartialEq, Eq)]
pub struct RecordedDispatch;

// The caller holds the exact basket and has already checked the order account.
pub(crate) async fn claim_owned_attempt(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    order_id: &str,
) -> Result<Option<String>, PaymentStoreError> {
    let attempts: Vec<(String, String)> = sqlx::query_as("SELECT pg_catalog.left(id,33),pg_catalog.left(basket_id,33) FROM public.payment_attempts WHERE order_id OPERATOR(pg_catalog.=) $1 LIMIT 2 FOR UPDATE /* owned dispatch status */")
        .bind(order_id).fetch_all(&mut **tx).await?;
    match attempts.as_slice() {
        [] => Ok(None),
        [(id, basket)] if identifier(id) && basket == owner => Ok(Some(id.clone())),
        _ => Err(PaymentStoreError::InvalidStoredData),
    }
}

pub(super) async fn reconciliation_event(
    tx: &mut Transaction<'_, Postgres>,
    order_id: &str,
    attempt_id: &str,
    start_id: &str,
    started_at: i64,
) -> Result<(), PaymentStoreError> {
    // Read at most two rows so damaged duplicate custody cannot be treated as valid.
    let events: Vec<(String, i64, bool)> = sqlx::query_as("SELECT pg_catalog.left(id,33),created_at,payload OPERATOR(pg_catalog.=) pg_catalog.jsonb_build_object('schema_version',1,'order_id',$1::pg_catalog.text,'attempt_id',$2::pg_catalog.text,'dispatch_id',$3::pg_catalog.text) FROM public.commerce_outbox WHERE aggregate_id OPERATOR(pg_catalog.=) $1 AND kind OPERATOR(pg_catalog.=) 'payment.reconciliation_required.v1' LIMIT 2")
        .bind(order_id).bind(attempt_id).bind(start_id)
        .fetch_all(&mut **tx).await?;
    if !matches!(events.as_slice(), [(id, at, true)] if identifier(id) && *at == started_at) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}

pub async fn owned_dispatch_status(
    store: &GuestSessionStore,
    key: &GuestCredential,
    order_id: &str,
) -> Result<RecordedDispatch, PaymentStoreError> {
    inspect(store, key, None, order_id).await
}

pub async fn owned_verified_dispatch_status(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &CustomerSessionToken,
    order_id: &str,
) -> Result<RecordedDispatch, PaymentStoreError> {
    inspect(store, key, Some(customer), order_id).await
}

async fn inspect(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&CustomerSessionToken>,
    order_id: &str,
) -> Result<RecordedDispatch, PaymentStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    if !identifier(order_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let result: Result<i64, PaymentStoreError> = async {
        let owner = scope.basket_id().to_owned();
        let account = session.as_ref().map(|s| s.account_id());
        checkout_orders::snapshot_for_account_in_transaction(
            scope.transaction(), &owner, order_id, account,
        ).await?;
        // Basket admission and cancellation already hold the basket claim. Dispatch
        // locks the attempt, so read its start only after this same claim completes.
        let attempt_id = claim_owned_attempt(scope.transaction(), &owner, order_id).await?
            .ok_or(PaymentStoreError::Unavailable)?;
        // The bounded locked ID is used for the exact source read; the decoder
        // checks that same row ID against the validated frozen snapshot.
        let frozen = stored::by_id(scope.transaction(), &owner, &attempt_id, checkout_orders::OrderAccess::Owned(account)).await?;
        let (start_id, started_at) = evidence::started(scope.transaction(), &frozen).await?;
        let canceled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.checkout_order_cancellations WHERE order_id OPERATOR(pg_catalog.=) $1)")
            .bind(order_id).fetch_one(&mut **scope.transaction()).await?;
        if canceled {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        reconciliation_event(scope.transaction(), order_id, &attempt_id, &start_id, started_at).await?;
        Ok(started_at)
    }.await;
    // A lock wait or damaged source must never reveal facts after either held
    // authority has expired. No database read follows this combined check.
    let at = scope.checked_at()?;
    prepare::check_session(session.as_ref(), at)?;
    let started_at = result?;
    if at < started_at {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    scope.rollback().await?;
    Ok(RecordedDispatch)
}
