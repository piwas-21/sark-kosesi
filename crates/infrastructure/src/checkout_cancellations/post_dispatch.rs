//! Owner-authenticated pending intent after a durable payment start. No disposition.
use super::*;
use crate::payment_attempts::{PaymentStoreError, evidence};

#[derive(Debug, PartialEq, Eq)]
pub struct PendingCancellationReceipt {
    pub order_id: String,
    pub requested_at: i64,
    pub replayed: bool,
}

/// Records a pending request, not a cancellation or a guarantee to stop fulfilment.
pub async fn request_after_dispatch(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    effect: &str,
    order_id: &str,
) -> Result<PendingCancellationReceipt, OrderStoreError> {
    request(store, key, csrf, None, effect, order_id).await
}

pub async fn request_verified_after_dispatch(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: &CustomerSessionToken,
    effect: &str,
    order_id: &str,
) -> Result<PendingCancellationReceipt, OrderStoreError> {
    request(store, key, csrf, Some(customer), effect, order_id).await
}

fn payment(error: PaymentStoreError) -> OrderStoreError {
    match error {
        PaymentStoreError::Boundary(error) => error,
        PaymentStoreError::NotFound | PaymentStoreError::Unavailable => OrderStoreError::Conflict,
        PaymentStoreError::ClockUnavailable => OrderStoreError::ClockUnavailable,
        PaymentStoreError::Database => OrderStoreError::Database,
        _ => OrderStoreError::InvalidStoredData,
    }
}

#[derive(sqlx::FromRow, PartialEq, Eq)]
struct Saved {
    effect: String,
    account: Option<i64>,
    attempt_id: String,
    dispatch_id: String,
    requested_at: i64,
    event_id: String,
    current_transaction: bool,
    source_valid: bool,
    event_valid: bool,
}

async fn saved(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
    basket: &str,
    account: Option<i64>,
) -> Result<Option<Saved>, OrderStoreError> {
    let row: Option<Saved> = sqlx::query_as("SELECT left(r.effect_key,129) AS effect,r.promotion_account_id AS account,left(r.attempt_id,33) AS attempt_id,left(r.dispatch_id,33) AS dispatch_id,r.requested_at,left(r.event_id,33) AS event_id,COALESCE(r.transaction_id=pg_current_xact_id_if_assigned(),false) AS current_transaction,COALESCE(public.checkout_post_dispatch_request_source(r),false) AS source_valid,EXISTS(SELECT 1 FROM public.commerce_outbox e WHERE e.id=r.event_id AND e.aggregate_id=r.order_id AND e.kind='checkout.cancellation_requested.v1' AND e.created_at=r.requested_at AND e.payload=public.checkout_post_dispatch_request_event(r)) AS event_valid FROM public.checkout_post_dispatch_requests r WHERE r.order_id=$1 AND r.basket_id=$2")
        .bind(order).bind(basket).fetch_optional(&mut **tx).await?;
    if let Some(value) = &row
        && (value.account != account
            || !valid_command_id(&value.effect)
            || !identifier(&value.attempt_id)
            || !identifier(&value.dispatch_id)
            || !identifier(&value.event_id)
            || !(0..=commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
                .contains(&value.requested_at)
            || !value.source_valid
            || !value.event_valid)
    {
        return Err(OrderStoreError::InvalidStoredData);
    }
    Ok(row)
}

async fn request(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: Option<&CustomerSessionToken>,
    effect: &str,
    order_id: &str,
) -> Result<PendingCancellationReceipt, OrderStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) =
            guest_baskets::verified_scope(store, key, Some(csrf), customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::mutation_scope(store, key, csrf).await?, None)
    };
    if !valid_command_id(effect) || !identifier(order_id) {
        return Err(OrderStoreError::InvalidRequest);
    }
    let result: Result<PendingCancellationReceipt, OrderStoreError> = async {
        let owner = scope.basket_id().to_owned();
        let account = session.as_ref().map(HeldSession::account_id);
        let _order = checkout_orders::snapshot_for_account_in_transaction(
            scope.transaction(), &owner, order_id, account,
        ).await?;
        let prior_order: Option<String> = sqlx::query_scalar("SELECT left(order_id,33) FROM public.checkout_post_dispatch_requests WHERE basket_id=$1 AND effect_key=$2")
            .bind(&owner).bind(effect).fetch_optional(&mut **scope.transaction()).await?;
        if prior_order.as_deref().is_some_and(|value| value != order_id) {
            return Err(OrderStoreError::Conflict);
        }
        let attempts: Vec<String> = sqlx::query_scalar("SELECT left(id,33) FROM public.payment_attempts WHERE order_id=$1 LIMIT 2 FOR UPDATE /* pending cancellation attempt */")
            .bind(order_id).fetch_all(&mut **scope.transaction()).await?;
        let attempt = match attempts.as_slice() {
            [] => return Err(OrderStoreError::Conflict),
            [id] if identifier(id) => id,
            _ => return Err(OrderStoreError::InvalidStoredData),
        };
        let (_frozen, dispatch, started_at) = evidence::context_with_start(scope.transaction(), attempt, false)
            .await.map_err(payment)?;
        let recovery: Vec<(String, i64, bool)> = sqlx::query_as("SELECT pg_catalog.left(id,33),created_at,payload=pg_catalog.jsonb_build_object('schema_version',1,'order_id',$1::text,'attempt_id',$2::text,'dispatch_id',$3::text) FROM public.commerce_outbox WHERE aggregate_id=$1 AND kind='payment.reconciliation_required.v1' LIMIT 2")
            .bind(order_id).bind(attempt).bind(&dispatch).fetch_all(&mut **scope.transaction()).await?;
        if !matches!(recovery.as_slice(), [(id, at, true)] if identifier(id) && *at == started_at) {
            return Err(OrderStoreError::InvalidStoredData);
        }
        if let Some(old) = saved(scope.transaction(), order_id, &owner, account).await? {
            if old.effect != effect { return Err(OrderStoreError::Conflict); }
            return Ok(PendingCancellationReceipt { order_id: order_id.into(), requested_at: old.requested_at, replayed: true });
        }
        if prior_order.is_some() { return Err(OrderStoreError::InvalidStoredData); }
        let at = scope.checked_at()?;
        if at < started_at { return Err(OrderStoreError::ClockUnavailable); }
        let event = scope.new_id()?;
        sqlx::query("INSERT INTO public.checkout_post_dispatch_requests(order_id,basket_id,effect_key,promotion_account_id,attempt_id,dispatch_id,requested_at,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(order_id).bind(&owner).bind(effect).bind(account).bind(attempt).bind(&dispatch).bind(at).bind(&event)
            .execute(&mut **scope.transaction()).await?;
        sqlx::query("INSERT INTO public.commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT event_id,order_id,'checkout.cancellation_requested.v1',public.checkout_post_dispatch_request_event(r),requested_at FROM public.checkout_post_dispatch_requests r WHERE order_id=$1")
            .bind(order_id).execute(&mut **scope.transaction()).await?;
        sqlx::query("SET CONSTRAINTS require_checkout_post_dispatch_request IMMEDIATE")
            .execute(&mut **scope.transaction()).await?;
        let written = saved(scope.transaction(), order_id, &owner, account).await?.ok_or(OrderStoreError::InvalidStoredData)?;
        if written != (Saved { effect: effect.into(), account, attempt_id: attempt.clone(),
            dispatch_id: dispatch, requested_at: at, event_id: event, current_transaction: true,
            source_valid: true, event_valid: true }) {
            return Err(OrderStoreError::InvalidStoredData);
        }
        Ok(PendingCancellationReceipt { order_id: order_id.into(), requested_at: at, replayed: false })
    }.await;
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    let receipt = result?;
    if at < receipt.requested_at {
        return Err(OrderStoreError::ClockUnavailable);
    }
    if receipt.replayed {
        drop(scope);
    } else {
        scope.commit().await?;
    }
    Ok(receipt)
}
