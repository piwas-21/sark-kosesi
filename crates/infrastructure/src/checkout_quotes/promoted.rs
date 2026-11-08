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
    issue(store, key, csrf, None, locale, request, promotion_id).await
}

/// Issue a private indicative quote while holding both owner and customer authority.
pub async fn issue_verified_promoted_quote(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: &crate::customer_sessions::CustomerSessionToken,
    locale: Locale,
    request: &GuestQuoteRequest,
    promotion_id: &str,
) -> Result<PromotedQuoteReceipt, QuoteStoreError> {
    issue(
        store,
        key,
        csrf,
        Some(customer),
        locale,
        request,
        promotion_id,
    )
    .await
}
async fn issue(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    customer: Option<&crate::customer_sessions::CustomerSessionToken>,
    locale: Locale,
    request: &GuestQuoteRequest,
    promotion_id: &str,
) -> Result<PromotedQuoteReceipt, QuoteStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) =
            guest_baskets::verified_scope(store, key, Some(csrf), customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::mutation_scope(store, key, csrf).await?, None)
    };
    let account = session.as_ref().map(|s| s.account_id());
    request.validate()?;
    if !merchant_code(promotion_id) {
        return Err(QuoteStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    if let Some((existing, _)) =
        stored::by_effect_for(scope.transaction(), &owner, &request.effect_key, account).await?
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
        let at = scope.checked_at()?;
        check_session(session.as_ref(), at)?;
        check_snapshot(&snapshot, &policy, account, at)?;
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
    if account.is_none() && promotion.limits.per_customer_redemptions.is_some() {
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
    let snapshot = if account.is_some() {
        PromotedQuoteSnapshot::capture_verified(original, &promotion, review_revision)?
    } else {
        PromotedQuoteSnapshot::capture(original, &promotion, review_revision)?
    };
    if let Some(account) = account {
        stored::insert_verified_promoted(
            scope.transaction(),
            &snapshot,
            &request.effect_key,
            account,
        )
        .await?;
    } else {
        stored::insert_promoted(scope.transaction(), &snapshot, &request.effect_key).await?;
    }
    let loaded = stored::by_id_for(
        scope.transaction(),
        &owner,
        &snapshot.original.binding.quote_id,
        account,
    )
    .await?;
    match loaded {
        stored::StoredQuote::Promoted(ref stored, ref exact_policy)
            if **stored == snapshot && **exact_policy == promotion => {}
        _ => return Err(QuoteStoreError::InvalidStoredData),
    }
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    proof.check_at(at)?;
    check_snapshot(&snapshot, &promotion, account, at)?;
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
    owned(store, key, None, id).await
}

pub async fn owned_verified_promoted_quote(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: &crate::customer_sessions::CustomerSessionToken,
    id: &str,
) -> Result<PromotedQuoteSnapshot, QuoteStoreError> {
    owned(store, key, Some(customer), id).await
}
async fn owned(
    store: &GuestSessionStore,
    key: &GuestCredential,
    customer: Option<&crate::customer_sessions::CustomerSessionToken>,
    id: &str,
) -> Result<PromotedQuoteSnapshot, QuoteStoreError> {
    let (mut scope, session) = if let Some(customer) = customer {
        let (scope, session) = guest_baskets::verified_scope(store, key, None, customer).await?;
        (scope, Some(session))
    } else {
        (guest_baskets::read_scope(store, key).await?, None)
    };
    let account = session.as_ref().map(|s| s.account_id());
    if !identifier(id) {
        return Err(QuoteStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let (snapshot, policy) =
        match stored::by_id_for(scope.transaction(), &owner, id, account).await? {
            stored::StoredQuote::Original(_) => return Err(QuoteStoreError::NotFound),
            stored::StoredQuote::Promoted(snapshot, policy) => (*snapshot, *policy),
        };
    let at = scope.checked_at()?;
    check_session(session.as_ref(), at)?;
    check_snapshot(&snapshot, &policy, account, at)?;
    scope.rollback().await?;
    Ok(snapshot)
}
fn check_session(
    session: Option<&crate::customer_sessions::HeldSession>,
    at: i64,
) -> Result<(), QuoteStoreError> {
    if let Some(session) = session {
        session
            .check_at(at)
            .map_err(|_| QuoteStoreError::Guest(GuestError::Denied))?;
    }
    Ok(())
}
fn check_snapshot(
    snapshot: &PromotedQuoteSnapshot,
    policy: &commerce_domain::promotion_policies::PromotionPolicy,
    account: Option<i64>,
    at: i64,
) -> Result<(), QuoteStoreError> {
    if account.is_some() {
        snapshot.check_at_verified(policy, snapshot.promotion.review_revision, at)?;
    } else {
        snapshot.check_at(policy, snapshot.promotion.review_revision, at)?;
    }
    Ok(())
}
