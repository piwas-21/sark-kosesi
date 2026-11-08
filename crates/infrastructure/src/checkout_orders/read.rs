use super::*;
pub async fn owned_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    owned(store, key, None, id).await
}

pub async fn owned_verified_promoted_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &crate::customer_sessions::CustomerSessionToken,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    owned(store, key, Some(customer), id).await
}

async fn owned(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&crate::customer_sessions::CustomerSessionToken>,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    if !identifier(id) {
        return Err(OrderStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let account = session.as_ref().map(|s| s.account_id());
    let snapshot = stored::by_id_for(scope.transaction(), &owner, id, account).await?;
    let at = scope.checked_at()?;
    if let Some(session) = session {
        session.check_at(at).map_err(|_| OrderStoreError::Denied)?;
    }
    scope.rollback().await?;
    Ok(snapshot)
}
