use super::*;

pub async fn record_image_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: ImageReviewRequest<'_>,
) -> Result<ImageReviewReceipt, ImageReviewError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueReview).await?;
    if !request.confirmed
        || !merchant_code(request.sku)
        || !request.target.valid()
        || !(0..i32::MAX).contains(&request.expected_revision)
        || !valid_command_id(request.effect_key)
        || !text(request.evidence_ref)
        || !text(request.qualification_ref)
        || !text(request.reason)
    {
        return Err(ImageReviewError::InvalidRequest);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("catalogue-image-review:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior=sqlx::query_as::<_,StoredReview>("SELECT sku,asset_id,bom_revision,specification_revision,rights_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM catalogue_image_reviews WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        if prior.sku != request.sku
            || prior.target() != *request.target
            || prior.revision.checked_sub(1) != Some(request.expected_revision)
            || prior.decision != request.decision.code()
            || prior.evidence_ref != request.evidence_ref
            || prior.qualification_ref != request.qualification_ref
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(ImageReviewError::Conflict);
        }
        let bundle = read::load(&mut access.tx, request.sku, request.target)
            .await?
            .ok_or(ImageReviewError::InvalidStoredData)?;
        let revision = prior.validated(&bundle)?.revision;
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(ImageReviewReceipt {
            revision,
            replayed: true,
        });
    }
    let exists: Option<String> = sqlx::query_scalar(
        "UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=$1 RETURNING sku",
    )
    .bind(request.sku)
    .fetch_optional(&mut *access.tx)
    .await?;
    exists.ok_or(ImageReviewError::NotFound)?;
    let claim: Option<String> = sqlx::query_scalar(
        "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=$1 FOR UPDATE",
    )
    .bind(&request.target.asset_id)
    .fetch_optional(&mut *access.tx)
    .await?;
    claim.ok_or(ImageReviewError::NotFound)?;
    let bundle = read::load(&mut access.tx, request.sku, request.target)
        .await?
        .ok_or(ImageReviewError::NotFound)?;
    if bundle.authored_by(&access.actor) {
        return Err(ImageReviewError::SelfReview);
    }
    let target = request.target;
    let (bom,spec,rights):(Option<i32>,Option<i32>,Option<i32>)=sqlx::query_as("SELECT (SELECT max(revision) FROM catalogue_boms WHERE sku=$1 AND sealed),(SELECT max(revision) FROM catalogue_specification_revisions WHERE sku=$1),(SELECT max(revision) FROM catalogue_media_rights WHERE asset_id=$2)")
        .bind(request.sku).bind(&target.asset_id).fetch_one(&mut *access.tx).await?;
    let current = bundle.review.as_ref().map_or(0, |review| review.revision);
    let now = access.authorized_at(clock)?;
    if current != request.expected_revision
        || (request.decision == ImageDecision::Approved
            && (bom != Some(target.bom_revision)
                || spec != Some(target.specification_revision)
                || rights != Some(target.rights_revision)
                || !bundle.rights.product_rights_active(&bundle.asset, now)))
    {
        return Err(ImageReviewError::Conflict);
    }
    let revision = current
        .checked_add(1)
        .ok_or(ImageReviewError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_image_reviews(sku,asset_id,bom_revision,specification_revision,rights_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason,evaluated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
        .bind(request.sku).bind(&target.asset_id).bind(target.bom_revision).bind(target.specification_revision).bind(target.rights_revision).bind(revision).bind(request.effect_key).bind(request.decision.code()).bind(request.evidence_ref).bind(request.qualification_ref).bind(&access.actor).bind(request.reason).bind(now).execute(&mut *access.tx).await?;
    let final_now = access.authorized_at(clock)?;
    if request.decision == ImageDecision::Approved
        && !bundle
            .rights
            .product_rights_active(&bundle.asset, final_now)
    {
        return Err(ImageReviewError::Conflict);
    }
    access.tx.commit().await?;
    Ok(ImageReviewReceipt {
        revision,
        replayed: false,
    })
}
