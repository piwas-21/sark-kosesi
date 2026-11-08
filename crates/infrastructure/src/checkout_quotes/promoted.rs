//! Owned private indicative promotion receipts; no claim or customer route.
use super::*;
use commerce_domain::{catalogue_authoring::merchant_code, checkout_quotes::PromotedQuoteSnapshot};

pub struct PromotedQuoteReceipt {
    pub snapshot: PromotedQuoteSnapshot,
    pub replayed: bool,
}

/// The trusted caller selects a promotion identity, never discount terms or amounts.
pub async fn issue_promoted_quote(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    locale: Locale,
    request: &GuestQuoteRequest,
    promotion_id: &str,
) -> Result<PromotedQuoteReceipt, QuoteStoreError> {
    let mut scope = guest_baskets::mutation_scope(store, key, csrf).await?;
    request.validate()?;
    if !merchant_code(promotion_id) {
        return Err(QuoteStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    if let Some((existing, _)) =
        stored::by_effect(scope.transaction(), &owner, &request.effect_key).await?
    {
        let (snapshot, policy) = match existing {
            stored::StoredQuote::Original(_) => return Err(QuoteStoreError::Conflict),
            stored::StoredQuote::Promoted(snapshot, policy) => (*snapshot, *policy),
        };
        let binding = &snapshot.original.binding;
        if binding.basket_revision != request.expected_basket_revision
            || binding.locale != locale.internal()
            || binding.country != request.destination.country
            || binding.postcode != request.destination.postcode
            || snapshot.promotion.id != promotion_id
        {
            return Err(QuoteStoreError::Conflict);
        }
        snapshot.check_at(
            &policy,
            snapshot.promotion.review_revision,
            scope.checked_at()?,
        )?;
        scope.rollback().await?;
        return Ok(PromotedQuoteReceipt {
            snapshot,
            replayed: true,
        });
    }
    if scope.revision() != request.expected_basket_revision {
        return Err(QuoteStoreError::Conflict);
    }
    let input = commerce_domain::quotes::QuoteRequest {
        destination: request.destination.clone(),
        lines: scope.lines().to_vec(),
    };
    input
        .validate()
        .map_err(|_| QuoteStoreError::InvalidRequest)?;
    issue::quota(&mut scope).await?;
    // Hold both reviewed anchors in the global market -> promotion order.
    let market =
        market_policies::reviewed_policy_in_transaction(scope.transaction(), &input).await?;
    let (promotion, review_revision) =
        promotion_policies::current_for_quote(scope.transaction(), promotion_id).await?;
    if promotion.limits.per_customer_redemptions.is_some() {
        return Err(QuoteStoreError::InvalidRequest);
    }
    let clock = scope.clock();
    let mut proof = quotes::prepare_promoted_in_transaction(
        scope.transaction(),
        clock.as_ref(),
        locale,
        &input,
        std::slice::from_ref(&market.policy),
        &promotion.promotion,
    )
    .await?;
    let created_at = scope.checked_at()?;
    proof.check_at(created_at)?;
    let expires_at = created_at
        .checked_add(MAX_QUOTE_LIFETIME_SECONDS)
        .ok_or(QuoteStoreError::ClockUnavailable)?
        .min(promotion.promotion.expires_at);
    let binding = QuoteBinding {
        quote_id: scope.new_id()?,
        basket_id: owner.clone(),
        basket_revision: scope.revision(),
        locale: locale.internal().into(),
        created_at,
        expires_at,
        country: input.destination.country,
        postcode: input.destination.postcode,
        policy_id: market.policy.id,
        policy_revision: market.policy.revision,
        policy_review_revision: market.review_revision,
    };
    let original = QuoteSnapshot::capture(binding, &proof.original.totals, &proof.bom_revisions())?;
    let snapshot = PromotedQuoteSnapshot::capture(original, &promotion, review_revision)?;
    stored::insert_promoted(scope.transaction(), &snapshot, &request.effect_key).await?;
    let loaded = stored::by_id_any(
        scope.transaction(),
        &owner,
        &snapshot.original.binding.quote_id,
    )
    .await?;
    match loaded {
        stored::StoredQuote::Promoted(ref stored, ref exact_policy)
            if **stored == snapshot && **exact_policy == promotion => {}
        _ => return Err(QuoteStoreError::InvalidStoredData),
    }
    let at = scope.checked_at()?;
    proof.check_at(at)?;
    snapshot.check_at(&promotion, review_revision, at)?;
    scope.commit().await?;
    Ok(PromotedQuoteReceipt {
        snapshot,
        replayed: false,
    })
}

pub async fn owned_promoted_quote(
    store: &GuestSessionStore,
    key: &GuestCredential,
    id: &str,
) -> Result<PromotedQuoteSnapshot, QuoteStoreError> {
    let mut scope = guest_baskets::read_scope(store, key).await?;
    if !identifier(id) {
        return Err(QuoteStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let (snapshot, policy) = match stored::by_id_any(scope.transaction(), &owner, id).await? {
        stored::StoredQuote::Original(_) => return Err(QuoteStoreError::NotFound),
        stored::StoredQuote::Promoted(snapshot, policy) => (*snapshot, *policy),
    };
    snapshot.check_at(
        &policy,
        snapshot.promotion.review_revision,
        scope.checked_at()?,
    )?;
    scope.rollback().await?;
    Ok(snapshot)
}
