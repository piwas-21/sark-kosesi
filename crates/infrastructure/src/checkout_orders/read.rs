use super::*;
pub async fn owned_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    id: &str,
) -> Result<OrderSnapshot, OrderStoreError> {
    let mut scope = guest_baskets::read_scope(store, key).await?;
    if !identifier(id) {
        return Err(OrderStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let snapshot = stored::by_id(scope.transaction(), &owner, id).await?;
    scope.checked_at()?;
    scope.rollback().await?;
    Ok(snapshot)
}
