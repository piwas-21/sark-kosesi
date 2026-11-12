//! Private cancellation before a payment dispatch has started.
use crate::{
    checkout_orders::{self, OrderStoreError, check_session, identifier},
    customer_sessions::{CustomerSessionToken, HeldSession},
    guest_baskets::{self, GuestCredential, GuestSessionStore},
};
use commerce_domain::media::valid_command_id;
use sqlx::{Postgres, Transaction};
mod rows;

#[derive(Debug, PartialEq, Eq)]
pub struct CancellationReceipt {
    pub order_id: String,
    pub canceled_at: i64,
    pub replayed: bool,
}

pub async fn cancel_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    effect: &str,
    order_id: &str,
) -> Result<CancellationReceipt, OrderStoreError> {
    cancel(store, key, csrf, None, effect, order_id).await
}

pub async fn cancel_verified_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: &CustomerSessionToken,
    effect: &str,
    order_id: &str,
) -> Result<CancellationReceipt, OrderStoreError> {
    cancel(store, key, csrf, Some(customer), effect, order_id).await
}

async fn cancel(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: Option<&CustomerSessionToken>,
    effect: &str,
    order_id: &str,
) -> Result<CancellationReceipt, OrderStoreError> {
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
    let owner = scope.basket_id().to_owned();
    let account = session.as_ref().map(HeldSession::account_id);
    let order = checkout_orders::snapshot_for_account_in_transaction(
        scope.transaction(),
        &owner,
        order_id,
        account,
    )
    .await?;
    if let Some(prior_order) = rows::order_by_effect(scope.transaction(), &owner, effect).await? {
        if prior_order != order_id {
            return Err(OrderStoreError::Conflict);
        }
        let prior = rows::by_order(scope.transaction(), &order.identity, account)
            .await?
            .ok_or(OrderStoreError::InvalidStoredData)?;
        let at = scope.checked_at()?;
        check_session(session.as_ref(), at)?;
        scope.rollback().await?;
        return Ok(CancellationReceipt {
            order_id: order_id.into(),
            canceled_at: prior.canceled_at,
            replayed: true,
        });
    }
    if rows::by_order(scope.transaction(), &order.identity, account)
        .await?
        .is_some()
    {
        return Err(OrderStoreError::Conflict);
    }
    let attempt = rows::claim_undispatched_attempt(scope.transaction(), order_id).await?;
    let canceled_at = scope.checked_at()?;
    if canceled_at < order.identity.created_at {
        return Err(OrderStoreError::ClockUnavailable);
    }
    if attempt
        .as_ref()
        .is_some_and(|(_, created_at)| canceled_at < *created_at)
    {
        return Err(OrderStoreError::ClockUnavailable);
    }
    let event = scope.new_id()?;
    rows::insert(
        scope.transaction(),
        &order.identity,
        effect,
        account,
        canceled_at,
        &event,
        attempt.as_ref().map(|(id, _)| id.as_str()),
    )
    .await?;
    sqlx::query("SET CONSTRAINTS require_complete_checkout_cancellation IMMEDIATE")
        .execute(&mut **scope.transaction())
        .await?;
    let saved = rows::by_order(scope.transaction(), &order.identity, account)
        .await?
        .ok_or(OrderStoreError::InvalidStoredData)?;
    if saved
        != (rows::Stored {
            effect: effect.into(),
            canceled_at,
            event_id: event,
            attempt_id: attempt.map(|(id, _)| id),
            current_transaction: true,
        })
    {
        return Err(OrderStoreError::InvalidStoredData);
    }
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    scope.commit().await?;
    Ok(CancellationReceipt {
        order_id: order_id.into(),
        canceled_at,
        replayed: false,
    })
}

pub async fn owned_cancellation(
    store: &GuestSessionStore,
    key: &GuestCredential,
    order_id: &str,
) -> Result<CancellationReceipt, OrderStoreError> {
    owned(store, key, None, order_id).await
}

pub async fn owned_verified_cancellation(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &CustomerSessionToken,
    order_id: &str,
) -> Result<CancellationReceipt, OrderStoreError> {
    owned(store, key, Some(customer), order_id).await
}

async fn owned(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&CustomerSessionToken>,
    order_id: &str,
) -> Result<CancellationReceipt, OrderStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    if !identifier(order_id) {
        return Err(OrderStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let account = session.as_ref().map(HeldSession::account_id);
    let order = checkout_orders::snapshot_for_account_in_transaction(
        scope.transaction(),
        &owner,
        order_id,
        account,
    )
    .await?;
    let saved = rows::by_order(scope.transaction(), &order.identity, account)
        .await?
        .ok_or(OrderStoreError::NotFound)?;
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    scope.rollback().await?;
    Ok(CancellationReceipt {
        order_id: order_id.into(),
        canceled_at: saved.canceled_at,
        replayed: true,
    })
}

/// Called only after payment preparation has verified ownership of this order.
pub(crate) async fn ensure_not_canceled(
    tx: &mut Transaction<'_, Postgres>,
    order_id: &str,
) -> Result<(), OrderStoreError> {
    let present: bool =
        sqlx::query_scalar("SELECT to_regclass('checkout_order_cancellations') IS NOT NULL")
            .fetch_one(&mut **tx)
            .await?;
    if present {
        let canceled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM checkout_order_cancellations WHERE order_id=$1)",
        )
        .bind(order_id)
        .fetch_one(&mut **tx)
        .await?;
        if canceled {
            return Err(OrderStoreError::Conflict);
        }
    }
    Ok(())
}
