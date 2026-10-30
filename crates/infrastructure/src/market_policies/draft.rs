use super::*;

pub struct DraftRequest<'a> {
    pub policy: &'a MarketPolicy,
    pub audit: Audit<'a>,
}
pub async fn record_draft(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: DraftRequest<'_>,
) -> Result<Receipt, MarketError> {
    let mut access = lock_staff(pool, token, clock, Capability::MarketManage).await?;
    let audit = request.audit;
    let mut policy = request.policy.clone();
    if !audit.valid()
        || policy.validate().is_err()
        || policy.revision.checked_sub(1) != Some(audit.expected_revision)
    {
        return Err(MarketError::InvalidRequest);
    }
    policy.products.sort_by(|a, b| a.sku.cmp(&b.sku));
    let payload = serde_json::to_value(&policy).map_err(|_| MarketError::InvalidRequest)?;
    effect_lock(&mut access.tx, "draft", audit.effect_key).await?;
    let prior: Option<(String, i32, String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT id,revision,actor,reason,policy FROM market_policy_drafts WHERE effect_key=$1",
    )
    .bind(audit.effect_key)
    .fetch_optional(&mut *access.tx)
    .await?;
    if let Some((id, revision, actor, reason, stored)) = prior {
        if id != policy.id
            || revision != policy.revision
            || actor != access.actor
            || reason != audit.reason
            || stored != payload
        {
            return Err(MarketError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(Receipt {
            revision,
            replayed: true,
        });
    }
    // Unique exact destination prevents ambiguous sibling policy fallback.
    sqlx::query("INSERT INTO market_policy_anchors(id,country,postcode) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(&policy.id).bind(&policy.destination.country).bind(&policy.destination.postcode)
        .execute(&mut *access.tx).await?;
    let current = claim(&mut access.tx, &policy.id).await?;
    let destination: (String, String) =
        sqlx::query_as("SELECT country,postcode FROM market_policy_anchors WHERE id=$1")
            .bind(&policy.id)
            .fetch_one(&mut *access.tx)
            .await?;
    if current != audit.expected_revision
        || destination
            != (
                policy.destination.country.clone(),
                policy.destination.postcode.clone(),
            )
    {
        return Err(MarketError::Conflict);
    }
    sqlx::query("INSERT INTO market_policy_drafts(id,revision,policy,effect_key,actor,reason) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&policy.id).bind(policy.revision).bind(payload).bind(audit.effect_key)
        .bind(&access.actor).bind(audit.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(Receipt {
        revision: policy.revision,
        replayed: false,
    })
}
