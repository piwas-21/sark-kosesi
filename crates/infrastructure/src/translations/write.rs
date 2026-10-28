use super::*;

pub async fn record_translation(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: TranslationRequest<'_>,
) -> Result<TranslationReceipt, TranslationError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !request.confirmed
        || !valid_sku(request.sku)
        || request.specification_revision <= 0
        || !valid_source(request.locale, request.source_revision)
        || !(0..i32::MAX).contains(&request.expected_revision)
        || !valid_command_id(request.effect_key)
        || !text(request.reason)
    {
        return Err(TranslationError::InvalidRequest);
    }
    request
        .draft
        .validate()
        .map_err(|_| TranslationError::InvalidRequest)?;
    let payload =
        serde_json::to_value(request.draft).map_err(|_| TranslationError::InvalidRequest)?;
    let encoded = payload.to_string();
    if encoded.len() > 65536 {
        return Err(TranslationError::InvalidRequest);
    }
    effect_lock(&mut access.tx, "draft", request.effect_key).await?;
    let prior = sqlx::query_as::<_,StoredDraft>("SELECT sku,locale,revision,specification_revision,source_revision,payload::text,actor,reason FROM catalogue_translation_revisions WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        let stored: serde_json::Value = serde_json::from_str(&prior.payload)
            .map_err(|_| TranslationError::InvalidStoredData)?;
        if prior.sku != request.sku
            || prior.locale != request.locale.internal()
            || prior.revision.checked_sub(1) != Some(request.expected_revision)
            || prior.specification_revision != request.specification_revision
            || prior.source_revision != request.source_revision
            || stored != payload
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(TranslationError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(TranslationReceipt {
            revision: prior.revision,
            replayed: true,
        });
    }
    lock_variant(&mut access.tx, request.sku).await?;
    let (current,):(i32,) = sqlx::query_as("SELECT coalesce(max(revision),0) FROM catalogue_translation_revisions WHERE sku=$1 AND locale=$2")
        .bind(request.sku).bind(request.locale.internal()).fetch_one(&mut *access.tx).await?;
    if current != request.expected_revision
        || !current_source(
            &mut access.tx,
            request.sku,
            request.locale,
            request.specification_revision,
            request.source_revision,
        )
        .await?
    {
        return Err(TranslationError::Conflict);
    }
    let revision = current
        .checked_add(1)
        .ok_or(TranslationError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_translation_revisions(sku,locale,revision,specification_revision,source_revision,effect_key,payload,actor,reason) VALUES($1,$2,$3,$4,$5,$6,$7::jsonb,$8,$9)")
        .bind(request.sku).bind(request.locale.internal()).bind(revision).bind(request.specification_revision).bind(request.source_revision).bind(request.effect_key).bind(encoded).bind(&access.actor).bind(request.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(TranslationReceipt {
        revision,
        replayed: false,
    })
}
