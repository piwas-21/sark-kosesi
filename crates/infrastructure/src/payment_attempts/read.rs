use super::*;
pub async fn owned_attempt(
    store: &GuestSessionStore,
    key: &GuestCredential,
    id: &str,
) -> Result<FrozenAttempt, PaymentStoreError> {
    let mut scope = guest_baskets::read_scope(store, key).await?;
    if !identifier(id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let frozen = stored::by_id(scope.transaction(), &owner, id).await?;
    scope.checked_at()?;
    scope.rollback().await?;
    Ok(frozen)
}
