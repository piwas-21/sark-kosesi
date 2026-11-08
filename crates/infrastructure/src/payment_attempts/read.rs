use super::*;
pub async fn owned_attempt(
    store: &GuestSessionStore,
    key: &GuestCredential,
    id: &str,
) -> Result<FrozenAttempt, PaymentStoreError> {
    owned(store, key, None, id).await
}

pub async fn owned_verified_attempt(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &crate::customer_sessions::CustomerSessionToken,
    id: &str,
) -> Result<FrozenAttempt, PaymentStoreError> {
    owned(store, key, Some(customer), id).await
}

async fn owned(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&crate::customer_sessions::CustomerSessionToken>,
    id: &str,
) -> Result<FrozenAttempt, PaymentStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    if !identifier(id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let account = session.as_ref().map(|s| s.account_id());
    let frozen = stored::by_id(
        scope.transaction(),
        &owner,
        id,
        checkout_orders::OrderAccess::Owned(account),
    )
    .await?;
    let at = scope.checked_at()?;
    prepare::check_session(session.as_ref(), at)?;
    scope.rollback().await?;
    Ok(frozen)
}
