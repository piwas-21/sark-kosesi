use super::*;

pub async fn record_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: TranslationReviewRequest<'_>,
) -> Result<TranslationReceipt, TranslationError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueReview).await?;
    if !request.confirmed
        || !valid_sku(request.sku)
        || request.translation_revision <= 0
        || !(0..i32::MAX).contains(&request.expected_review_revision)
        || !valid_command_id(request.effect_key)
        || !text(request.evidence_ref)
        || !text(request.qualification_ref)
        || !text(request.reason)
    {
        return Err(TranslationError::InvalidRequest);
    }
    effect_lock(&mut access.tx, "review", request.effect_key).await?;
    let prior=sqlx::query_as::<_,StoredReview>("SELECT sku,locale,translation_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM catalogue_translation_reviews WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        if prior.sku != request.sku
            || prior.locale != request.locale.internal()
            || prior.translation_revision != request.translation_revision
            || prior.revision.checked_sub(1) != Some(request.expected_review_revision)
            || prior.decision != request.decision.code()
            || prior.evidence_ref != request.evidence_ref
            || prior.qualification_ref != request.qualification_ref
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
    let draft = read::load(
        &mut access.tx,
        request.sku,
        request.locale,
        request.translation_revision,
    )
    .await?
    .ok_or(TranslationError::NotFound)?;
    if draft.actor == access.actor {
        return Err(TranslationError::SelfReview);
    }
    let (current_translation,current_review):(Option<i32>,i32)=sqlx::query_as("SELECT (SELECT max(revision) FROM catalogue_translation_revisions WHERE sku=$1 AND locale=$2),(SELECT coalesce(max(revision),0) FROM catalogue_translation_reviews WHERE sku=$1 AND locale=$2 AND translation_revision=$3)")
        .bind(request.sku).bind(request.locale.internal()).bind(request.translation_revision).fetch_one(&mut *access.tx).await?;
    if current_translation != Some(request.translation_revision)
        || current_review != request.expected_review_revision
        || !current_source(
            &mut access.tx,
            request.sku,
            request.locale,
            draft.specification_revision,
            draft.source_revision,
        )
        .await?
    {
        return Err(TranslationError::Conflict);
    }
    let revision = current_review
        .checked_add(1)
        .ok_or(TranslationError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_translation_reviews(sku,locale,translation_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(request.sku).bind(request.locale.internal()).bind(request.translation_revision).bind(revision).bind(request.effect_key).bind(request.decision.code()).bind(request.evidence_ref).bind(request.qualification_ref).bind(&access.actor).bind(request.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(TranslationReceipt {
        revision,
        replayed: false,
    })
}
