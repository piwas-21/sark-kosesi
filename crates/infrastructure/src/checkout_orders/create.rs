use super::*;
use commerce_domain::{
    holds::{HoldAction, HoldState, HoldWindow},
    quotes::QuoteRequest,
};
pub(crate) fn products(proof: &quotes::QuoteProof) -> Vec<ProductSnapshot> {
    let mut products: Vec<_> = proof
        .selected_products()
        .map(|(p, bom)| ProductSnapshot {
            sku: p.sku.clone(),
            publication_revision: p.publication_revision,
            bom_revision: bom,
            title: p.copy.title.clone(),
            family_code: p.family_code.clone(),
            size_code: p.size_code.clone(),
            textile_code: p.textile_code.clone(),
            fill: p.fill.clone(),
            footprint: p.footprint.clone(),
        })
        .collect();
    products.sort_by(|a, b| a.sku.cmp(&b.sku));
    products
}
async fn replay(
    scope: BasketScope,
    existing: (OrderSnapshot, OrderIntent),
    intent: &OrderIntent,
) -> Result<OrderReceipt, OrderStoreError> {
    let mut scope = scope;
    if existing.1 != *intent {
        return Err(OrderStoreError::Conflict);
    }
    scope.checked_at()?;
    scope.rollback().await?;
    Ok(OrderReceipt {
        snapshot: existing.0,
        replayed: true,
    })
}
/// Seller policy is trusted server configuration; no caller/customer total is accepted.
/// This private primitive grants no provider, public-route or fulfilment permission.
pub async fn create_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    effect: &str,
    intent: &OrderIntent,
    seller: &SellerPolicy,
) -> Result<OrderReceipt, OrderStoreError> {
    let mut scope = guest_baskets::mutation_scope(store, key, csrf).await?;
    if !valid_command_id(effect) || intent.validate().is_err() {
        return Err(OrderStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    if let Some(prior) = stored::by_effect(scope.transaction(), &owner, effect).await? {
        return replay(scope, prior, intent).await;
    }
    if stored::by_quote(scope.transaction(), &owner, &intent.quote_id)
        .await?
        .is_some()
    {
        return Err(OrderStoreError::Conflict);
    }
    if intent.expected_basket_revision != scope.revision() {
        return Err(OrderStoreError::Conflict);
    }
    let quote = crate::checkout_quotes::snapshot_in_transaction(
        scope.transaction(),
        &owner,
        &intent.quote_id,
    )
    .await?;
    quote.check_at(scope.checked_at()?)?;
    if quote.binding.basket_revision != scope.revision()
        || quote.binding.country != intent.shipping_address.country
        || quote.binding.postcode != intent.shipping_address.postcode
    {
        return Err(OrderStoreError::QuoteChanged);
    }
    let locale =
        Locale::from_internal(&quote.binding.locale).ok_or(OrderStoreError::InvalidStoredData)?;
    if seller.terms_version != intent.accepted_terms_version {
        return Err(OrderStoreError::Unavailable);
    }
    seller
        .permits(locale.internal(), scope.checked_at()?)
        .map_err(|_| OrderStoreError::Unavailable)?;
    let input = QuoteRequest {
        destination: commerce_domain::quotes::Destination {
            country: intent.shipping_address.country.clone(),
            postcode: intent.shipping_address.postcode.clone(),
        },
        lines: scope.lines().to_vec(),
    };
    let policy =
        market_policies::reviewed_policy_in_transaction(scope.transaction(), &input).await?;
    let clock = scope.clock();
    let mut proof = quotes::prepare_order_in_transaction(
        scope.transaction(),
        clock.as_ref(),
        locale,
        &input,
        std::slice::from_ref(&policy.policy),
    )
    .await?;
    let mut binding: QuoteBinding = quote.binding.clone();
    binding.policy_id = policy.policy.id.clone();
    binding.policy_revision = policy.policy.revision;
    binding.policy_review_revision = policy.review_revision;
    let current = QuoteSnapshot::capture(binding, &proof.original.totals, &proof.bom_revisions())?;
    if current != quote {
        return Err(OrderStoreError::QuoteChanged);
    }
    let created_at = scope.checked_at()?;
    quote.check_at(created_at)?;
    proof.check_at(created_at)?;
    let hold_expires_at = created_at
        .checked_add(900)
        .ok_or(OrderStoreError::ClockUnavailable)?;
    let id = scope.new_id()?;
    let stock_group_id = scope.new_id()?;
    let event_id = scope.new_id()?;
    let lines = quote
        .lines
        .iter()
        .map(|line| {
            Ok(holds::BasketHoldLine {
                id: scope.new_id()?,
                sku: line.sku.clone(),
                revision: line.bom_revision,
                units: u16::try_from(line.quantity)
                    .map_err(|_| OrderStoreError::InvalidStoredData)?,
            })
        })
        .collect::<Result<Vec<_>, OrderStoreError>>()?;
    let reserved = holds::basket::reserve_in_transaction(
        scope.transaction(),
        clock.as_ref(),
        holds::BasketHoldRequest {
            id: &stock_group_id,
            owner_id: &id,
            created_at,
            expires_at: hold_expires_at,
            lines: &lines,
        },
    )
    .await?;
    if reserved.replayed {
        return Err(OrderStoreError::Conflict);
    }
    let snapshot = OrderSnapshot {
        schema_version: ORDER_SNAPSHOT_VERSION,
        identity: OrderIdentity {
            id,
            basket_id: owner,
            quote_id: intent.quote_id.clone(),
            stock_group_id,
            created_at,
            hold_expires_at,
        },
        quote,
        buyer: intent.buyer.clone(),
        shipping_address: intent.shipping_address.clone(),
        billing_address: intent.billing_address.clone(),
        seller: seller.clone(),
        market: policy.policy,
        products: products(&proof),
    };
    snapshot
        .validate()
        .map_err(|_| OrderStoreError::InvalidStoredData)?;
    stored::insert(scope.transaction(), &snapshot, intent, effect, &event_id).await?;
    let at = scope.checked_at()?;
    proof.check_at(at)?;
    snapshot.quote.check_at(at)?;
    seller
        .permits(locale.internal(), at)
        .map_err(|_| OrderStoreError::Unavailable)?;
    HoldWindow::new(created_at, hold_expires_at)
        .and_then(|w| w.transition(HoldState::Active, HoldAction::Consume, at))
        .map_err(|_| OrderStoreError::Unavailable)?;
    scope.commit().await?;
    Ok(OrderReceipt {
        snapshot,
        replayed: false,
    })
}
