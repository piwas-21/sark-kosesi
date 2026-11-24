//! Owner-scoped status of a recorded post-dispatch request.
use super::{
    OrderStoreError, check_session, checkout_orders, guest_baskets, identifier,
    inspection::{self, InspectionError},
};
use crate::{
    customer_sessions::{CustomerSessionToken, HeldSession},
    guest_baskets::{GuestCredential, GuestSessionStore},
};
use sqlx::{Postgres, Transaction};

/// The timestamp of the recorded request. Disposition remains separate.
#[derive(Debug, PartialEq, Eq)]
pub struct PendingCancellationStatus {
    pub requested_at: i64,
}

pub(crate) async fn pending_cancellation_locked(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<Option<PendingCancellationStatus>, OrderStoreError> {
    let request = Box::pin(inspection::inspect_locked(tx, order))
        .await
        .map_err(|error| match error {
            InspectionError::Database => OrderStoreError::Database,
            _ => OrderStoreError::InvalidStoredData,
        })?;
    Ok(request.map(|request| PendingCancellationStatus {
        requested_at: request.requested_at,
    }))
}

pub async fn owned_pending_cancellation(
    store: &GuestSessionStore,
    key: &GuestCredential,
    order: &str,
) -> Result<Option<PendingCancellationStatus>, OrderStoreError> {
    read(store, key, None, order).await
}

pub async fn owned_verified_pending_cancellation(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &CustomerSessionToken,
    order: &str,
) -> Result<Option<PendingCancellationStatus>, OrderStoreError> {
    read(store, key, Some(customer), order).await
}

async fn read(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&CustomerSessionToken>,
    order: &str,
) -> Result<Option<PendingCancellationStatus>, OrderStoreError> {
    if !identifier(order) {
        return Err(OrderStoreError::InvalidRequest);
    }
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    let result: Result<_, OrderStoreError> = async {
        let owner = scope.basket_id().to_owned();
        let account = session.as_ref().map(HeldSession::account_id);
        checkout_orders::snapshot_for_account_in_transaction(
            scope.transaction(),
            &owner,
            order,
            account,
        )
        .await?;
        pending_cancellation_locked(scope.transaction(), order).await
    }
    .await;
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    let result = result?;
    if result
        .as_ref()
        .is_some_and(|request| at < request.requested_at)
    {
        return Err(OrderStoreError::ClockUnavailable);
    }
    drop(scope);
    Ok(result)
}
