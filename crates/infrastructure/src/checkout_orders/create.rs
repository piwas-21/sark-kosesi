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
    session: Option<&crate::customer_sessions::HeldSession>,
) -> Result<OrderReceipt, OrderStoreError> {
    let mut scope = scope;
    if existing.1 != *intent {
        return Err(OrderStoreError::Conflict);
    }
    let at = scope.checked_at()?;
    check_session(session, at)?;
    scope.rollback().await?;
    Ok(OrderReceipt {
        snapshot: existing.0,
        replayed: true,
    })
}
pub(super) async fn create(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: Option<&crate::customer_sessions::CustomerSessionToken>,
    effect: &str,
    intent: &OrderIntent,
    seller: &SellerPolicy,
) -> Result<OrderReceipt, OrderStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) =
            guest_baskets::verified_scope(store, key, Some(csrf), customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::mutation_scope(store, key, csrf).await?, None)
    };
    let account = session.as_ref().map(|s| s.account_id());
    if !valid_command_id(effect) || intent.validate().is_err() {
        return Err(OrderStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    if let Some(prior) = stored::by_effect(scope.transaction(), &owner, effect, account).await? {
        return replay(scope, prior, intent, session.as_ref()).await;
    }
    if stored::by_quote(scope.transaction(), &owner, &intent.quote_id, account)
        .await?
        .is_some()
    {
        return Err(OrderStoreError::Conflict);
    }
    if intent.expected_basket_revision != scope.revision() {
        return Err(OrderStoreError::Conflict);
    }
    let captured =
        crate::checkout_quotes::by_id_for(scope.transaction(), &owner, &intent.quote_id, account)
            .await?;
    let (quote, promoted) = match captured {
        // The quote reader permits an original snapshot only with an unbound owner.
        crate::checkout_quotes::StoredQuote::Original(quote) => (*quote, None),
        crate::checkout_quotes::StoredQuote::Promoted(promoted, historical) => {
            check_promoted(&promoted, &historical, account, scope.checked_at()?)?;
            (promoted.original.clone(), Some(*promoted))
        }
    };
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
    let current_promotion = redemption::claim(&mut scope, promoted.as_ref(), account).await?;
    let clock = scope.clock();
    let mut proof = if let Some(p) = &current_promotion {
        quotes::prepare_promoted_order_in_transaction(
            scope.transaction(),
            clock.as_ref(),
            locale,
            &input,
            std::slice::from_ref(&policy.policy),
            &p.promotion,
        )
        .await?
    } else {
        quotes::prepare_order_in_transaction(
            scope.transaction(),
            clock.as_ref(),
            locale,
            &input,
            std::slice::from_ref(&policy.policy),
        )
        .await?
    };
    let mut binding: QuoteBinding = quote.binding.clone();
    binding.policy_id = policy.policy.id.clone();
    binding.policy_revision = policy.policy.revision;
    binding.policy_review_revision = policy.review_revision;
    let current = QuoteSnapshot::capture(binding, &proof.original.totals, &proof.bom_revisions())?;
    if current != quote {
        return Err(OrderStoreError::QuoteChanged);
    }
    // claim() has reconstructed the promoted allocation from this exact original
    // and current reviewed policy. The stock-locked evaluation above reprices
    // the same original and policy; the equality check retains both claims.
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
        schema_version: if promoted.is_some() {
            2
        } else {
            ORDER_SNAPSHOT_VERSION
        },
        identity: OrderIdentity {
            id,
            basket_id: owner,
            quote_id: intent.quote_id.clone(),
            stock_group_id,
            created_at,
            hold_expires_at,
        },
        quote,
        promoted_quote: promoted,
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
    redemption::insert(scope.transaction(), &snapshot, account).await?;
    let retained = stored::by_id_for(
        scope.transaction(),
        &snapshot.identity.basket_id,
        &snapshot.identity.id,
        account,
    )
    .await?;
    if retained != snapshot {
        return Err(OrderStoreError::InvalidStoredData);
    }
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    proof.check_at(at)?;
    snapshot.quote.check_at(at)?;
    if let (Some(p), Some(policy)) = (&snapshot.promoted_quote, &current_promotion) {
        check_promoted(p, policy, account, at)?;
    }
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
