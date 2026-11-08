use super::*;
use commerce_domain::payment_attempts::PreparationIntent;
pub struct PreparationReceipt {
    pub frozen: FrozenAttempt,
    pub replayed: bool,
}
/// Trusted server configuration only; accepts no customer amount/profile/URL/stock claim.
/// A receipt schedules work but grants no provider dispatch or payment success.
pub async fn prepare_attempt(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    effect: &str,
    intent: &PreparationIntent,
    configuration: &Configuration,
) -> Result<PreparationReceipt, PaymentStoreError> {
    prepare(store, key, csrf, None, effect, intent, configuration).await
}

/// Prepare a private payment only while both account and basket authority are held.
pub async fn prepare_verified_attempt(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: &crate::customer_sessions::CustomerSessionToken,
    effect: &str,
    intent: &PreparationIntent,
    configuration: &Configuration,
) -> Result<PreparationReceipt, PaymentStoreError> {
    prepare(
        store,
        key,
        csrf,
        Some(customer),
        effect,
        intent,
        configuration,
    )
    .await
}

async fn prepare(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: Option<&crate::customer_sessions::CustomerSessionToken>,
    effect: &str,
    intent: &PreparationIntent,
    configuration: &Configuration,
) -> Result<PreparationReceipt, PaymentStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) =
            guest_baskets::verified_scope(store, key, Some(csrf), customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::mutation_scope(store, key, csrf).await?, None)
    };
    let account = session.as_ref().map(|s| s.account_id());
    if !valid_command_id(effect) || intent.validate().is_err() {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    if let Some(frozen) = stored::by_effect(scope.transaction(), &owner, effect, account).await? {
        if frozen.snapshot().binding.order_id != intent.order_id {
            return Err(PaymentStoreError::Conflict);
        }
        crate::checkout_cancellations::ensure_not_canceled(scope.transaction(), &intent.order_id)
            .await?;
        let at = scope.checked_at()?;
        check_session(session.as_ref(), at)?;
        scope.rollback().await?;
        return Ok(PreparationReceipt {
            frozen,
            replayed: true,
        });
    }
    let order = checkout_orders::snapshot_for_account_in_transaction(
        scope.transaction(),
        &owner,
        &intent.order_id,
        account,
    )
    .await?;
    crate::checkout_cancellations::ensure_not_canceled(scope.transaction(), &intent.order_id)
        .await?;
    if stored::by_order(scope.transaction(), &owner, &intent.order_id, account)
        .await?
        .is_some()
    {
        return Err(PaymentStoreError::Conflict);
    }
    let clock = scope.clock();
    let mut proof = eligibility::claim(scope.transaction(), clock.as_ref(), &order).await?;
    let created_at = scope.checked_at()?;
    proof.check_at(created_at)?;
    let current = configuration.identity();
    let binding = AttemptBinding {
        id: scope.new_id()?,
        order_id: order.identity.id.clone(),
        basket_id: owner.clone(),
        stock_group_id: order.identity.stock_group_id.clone(),
        profile: current.profile,
        configuration_sha256: current.sha256,
        currency: commerce_domain::Currency::EUR,
        amount_minor: order.payable_total().gross_minor,
        created_at,
        dispatch_before: order.identity.hold_expires_at,
    };
    let frozen = FrozenAttempt::capture(binding, configuration)?;
    let event_id = scope.new_id()?;
    stored::insert(
        scope.transaction(),
        &frozen,
        configuration,
        intent,
        effect,
        &event_id,
        account,
    )
    .await?;
    // Restoration verifies the canonical body against the retained snapshot.
    let retained = stored::by_effect(scope.transaction(), &owner, effect, account).await?;
    if retained.as_ref().map(FrozenAttempt::snapshot) != Some(frozen.snapshot()) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    proof.check_at(at)?;
    scope.commit().await?;
    Ok(PreparationReceipt {
        frozen,
        replayed: false,
    })
}

pub(super) fn check_session(
    session: Option<&crate::customer_sessions::HeldSession>,
    at: i64,
) -> Result<(), PaymentStoreError> {
    if let Some(session) = session {
        session
            .check_at(at)
            .map_err(|_| checkout_orders::OrderStoreError::Denied)?;
    }
    Ok(())
}
