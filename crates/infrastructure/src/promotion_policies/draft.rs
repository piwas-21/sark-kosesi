use super::*;

pub struct DraftRequest<'a> {
    pub policy: &'a PromotionPolicy,
    pub audit: Audit<'a>,
}
pub async fn record_draft(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: DraftRequest<'_>,
) -> Result<Receipt, PolicyError> {
    let observed = ObservedClock::new(clock, 0);
    let clock: &dyn Clock = &observed;
    let mut access = lock_staff(pool, token, clock, Capability::PromotionManage).await?;
    let audit = request.audit;
    if !audit.valid()
        || request.policy.validate().is_err()
        || request.policy.promotion.revision.checked_sub(1) != Some(audit.expected_revision)
    {
        return Err(PolicyError::InvalidRequest);
    }
    let mut policy = request.policy.clone();
    policy.canonicalize();
    let payload = serde_json::to_value(&policy).map_err(|_| PolicyError::InvalidRequest)?;
    effect_lock(&mut access.tx, "draft", audit.effect_key).await?;
    let prior: Option<(String, i32, String, String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT left(id,66),revision,left(actor,501),left(reason,501),CASE WHEN octet_length(policy::text)<=32768 THEN policy END AS policy FROM promotion_policy_drafts WHERE effect_key=$1",
    )
    .bind(audit.effect_key)
    .fetch_optional(&mut *access.tx)
    .await?;
    if let Some((id, revision, actor, reason, stored)) = prior {
        let stored = stored.ok_or(PolicyError::InvalidStoredData)?;
        if id != policy.promotion.id
            || revision != policy.promotion.revision
            || actor != access.actor
            || reason != audit.reason
            || stored != payload
        {
            return Err(PolicyError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(Receipt {
            revision,
            replayed: true,
        });
    }
    sqlx::query("INSERT INTO promotion_policy_anchors(id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(&policy.promotion.id)
        .execute(&mut *access.tx)
        .await?;
    let current = claim(&mut access.tx, &policy.promotion.id).await?;
    if current != audit.expected_revision {
        return Err(PolicyError::Conflict);
    }
    sqlx::query("INSERT INTO promotion_policy_drafts(id,revision,policy,effect_key,actor,reason) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&policy.promotion.id).bind(policy.promotion.revision).bind(payload).bind(audit.effect_key)
        .bind(&access.actor).bind(audit.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(Receipt {
        revision: policy.promotion.revision,
        replayed: false,
    })
}
