use super::*;
async fn quota(scope: &mut BasketScope) -> Result<(), QuoteStoreError> {
    let at = scope.checked_at()?;
    let owner = scope.basket_id().to_owned();
    sqlx::query("INSERT INTO guest_quote_quota(basket_id,rate_window,issued) VALUES($1,$2,0) ON CONFLICT DO NOTHING").bind(&owner).bind(at/60).execute(&mut **scope.transaction()).await?;
    let (window, issued): (i64, i32) = sqlx::query_as(
        "SELECT rate_window,issued FROM guest_quote_quota WHERE basket_id=$1 FOR UPDATE",
    )
    .bind(&owner)
    .fetch_one(&mut **scope.transaction())
    .await?;
    let current = scope.checked_at()? / 60;
    if window < 0 || !(0..=10).contains(&issued) {
        return Err(QuoteStoreError::InvalidStoredData);
    }
    if current < window {
        return Err(QuoteStoreError::ClockUnavailable);
    }
    let count = if current == window { issued } else { 0 };
    if count >= 10 {
        return Err(QuoteStoreError::RateLimited);
    }
    sqlx::query("UPDATE guest_quote_quota SET rate_window=$2,issued=$3 WHERE basket_id=$1")
        .bind(owner)
        .bind(current)
        .bind(
            count
                .checked_add(1)
                .ok_or(QuoteStoreError::InvalidStoredData)?,
        )
        .execute(&mut **scope.transaction())
        .await?;
    Ok(())
}
pub async fn issue_quote(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    locale: Locale,
    request: &GuestQuoteRequest,
) -> Result<QuoteReceipt, QuoteStoreError> {
    let mut scope = guest_baskets::mutation_scope(store, key, csrf).await?;
    request.validate()?;
    let owner = scope.basket_id().to_owned();
    if let Some((snapshot, _)) =
        stored::by_effect(scope.transaction(), &owner, &request.effect_key).await?
    {
        let b = &snapshot.binding;
        if b.basket_revision != request.expected_basket_revision
            || b.locale != locale.internal()
            || b.country != request.destination.country
            || b.postcode != request.destination.postcode
        {
            return Err(QuoteStoreError::Conflict);
        }
        snapshot.check_at(scope.checked_at()?)?;
        scope.rollback().await?;
        return Ok(QuoteReceipt {
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
    quota(&mut scope).await?;
    let policy: market_policies::ReviewedPolicy =
        market_policies::reviewed_policy_in_transaction(scope.transaction(), &input).await?;
    let clock = scope.clock();
    let mut proof = quotes::prepare_in_transaction(
        scope.transaction(),
        clock.as_ref(),
        locale,
        &input,
        std::slice::from_ref(&policy.policy),
    )
    .await?;
    let created_at = scope.checked_at()?;
    proof.check_at(created_at)?;
    let expires_at = created_at
        .checked_add(MAX_QUOTE_LIFETIME_SECONDS)
        .ok_or(QuoteStoreError::ClockUnavailable)?;
    let binding = QuoteBinding {
        quote_id: scope.new_id()?,
        basket_id: owner,
        basket_revision: scope.revision(),
        locale: locale.internal().into(),
        created_at,
        expires_at,
        country: input.destination.country,
        postcode: input.destination.postcode,
        policy_id: policy.policy.id,
        policy_revision: policy.policy.revision,
        policy_review_revision: policy.review_revision,
    };
    let snapshot = QuoteSnapshot::capture(binding, &proof.original.totals, &proof.bom_revisions())?;
    stored::insert(scope.transaction(), &snapshot, &request.effect_key).await?;
    // One final sample validates every retained source, identity and stored readback deadline.
    let at = scope.checked_at()?;
    proof.check_at(at)?;
    snapshot.check_at(at)?;
    scope.commit().await?;
    Ok(QuoteReceipt {
        snapshot,
        replayed: false,
    })
}
pub async fn owned_quote(
    store: &GuestSessionStore,
    key: &GuestCredential,
    id: &str,
) -> Result<QuoteSnapshot, QuoteStoreError> {
    let mut scope = guest_baskets::read_scope(store, key).await?;
    if !identifier(id) {
        return Err(QuoteStoreError::InvalidRequest);
    }
    let owner = scope.basket_id().to_owned();
    let snapshot = stored::by_id(scope.transaction(), &owner, id).await?;
    snapshot.check_at(scope.checked_at()?)?;
    scope.rollback().await?;
    Ok(snapshot)
}
