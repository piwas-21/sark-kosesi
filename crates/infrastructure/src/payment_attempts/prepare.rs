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
    let mut scope = guest_baskets::mutation_scope(store, key, csrf).await?;
    if !valid_command_id(effect) || intent.validate().is_err() {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    if let Some(frozen) = stored::by_effect(scope.transaction(), &owner, effect).await? {
        if frozen.snapshot().binding.order_id != intent.order_id {
            return Err(PaymentStoreError::Conflict);
        }
        scope.checked_at()?;
        scope.rollback().await?;
        return Ok(PreparationReceipt {
            frozen,
            replayed: true,
        });
    }
    if stored::by_order(scope.transaction(), &owner, &intent.order_id)
        .await?
        .is_some()
    {
        return Err(PaymentStoreError::Conflict);
    }
    let order =
        checkout_orders::snapshot_in_transaction(scope.transaction(), &owner, &intent.order_id)
            .await?;
    let clock = scope.clock();
    let mut proof = eligibility::claim(scope.transaction(), clock.as_ref(), &order).await?;
    let created_at = scope.checked_at()?;
    proof.check_at(&order, created_at)?;
    let current = configuration.identity();
    let binding = AttemptBinding {
        id: scope.new_id()?,
        order_id: order.identity.id.clone(),
        basket_id: owner,
        stock_group_id: order.identity.stock_group_id.clone(),
        profile: current.profile,
        configuration_sha256: current.sha256,
        currency: commerce_domain::Currency::EUR,
        amount_minor: order.quote.total.gross_minor,
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
    )
    .await?;
    proof.check_at(&order, scope.checked_at()?)?;
    scope.commit().await?;
    Ok(PreparationReceipt {
        frozen,
        replayed: false,
    })
}
