use super::*;

pub async fn record_factual_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: FactReviewRequest<'_>,
) -> Result<FactReceipt, FactError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueReview).await?;
    if !request.confirmed
        || !merchant_code(request.sku)
        || !request.target.valid()
        || !(0..i32::MAX).contains(&request.expected_review_revision)
        || !valid_command_id(request.effect_key)
        || !text(request.evidence_ref)
        || !text(request.qualification_ref)
        || !text(request.reason)
    {
        return Err(FactError::InvalidRequest);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("catalogue-facts:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior=sqlx::query_as::<_,StoredReview>("SELECT sku,bom_revision,specification_revision,price_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM catalogue_factual_reviews WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        if prior.sku != request.sku
            || prior.target() != request.target
            || prior.revision.checked_sub(1) != Some(request.expected_review_revision)
            || prior.decision != request.decision.code()
            || prior.evidence_ref != request.evidence_ref
            || prior.qualification_ref != request.qualification_ref
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(FactError::Conflict);
        }
        let bundle = read::load(&mut access.tx, request.sku, request.target)
            .await?
            .ok_or(FactError::InvalidStoredData)?;
        let receipt = FactReceipt {
            revision: prior.validated(&bundle)?.revision,
            replayed: true,
        };
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(receipt);
    }
    let exists: Option<String> = sqlx::query_scalar(
        "UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=$1 RETURNING sku",
    )
    .bind(request.sku)
    .fetch_optional(&mut *access.tx)
    .await?;
    exists.ok_or(FactError::NotFound)?;
    let bundle = read::load(&mut access.tx, request.sku, request.target)
        .await?
        .ok_or(FactError::NotFound)?;
    if bundle.authored_by(&access.actor) {
        return Err(FactError::SelfReview);
    }
    let target = request.target;
    let (review,bom,specification,price):(i32,Option<i32>,Option<i32>,Option<i32>)=sqlx::query_as("SELECT (SELECT coalesce(max(revision),0) FROM catalogue_factual_reviews WHERE sku=$1 AND bom_revision=$2 AND specification_revision=$3 AND price_revision=$4),(SELECT max(revision) FROM catalogue_boms WHERE sku=$1 AND sealed),(SELECT max(revision) FROM catalogue_specification_revisions WHERE sku=$1),(SELECT max(revision) FROM catalogue_price_revisions WHERE sku=$1)")
        .bind(request.sku).bind(target.bom_revision).bind(target.specification_revision).bind(target.price_revision).fetch_one(&mut *access.tx).await?;
    if review != request.expected_review_revision
        || (request.decision == FactDecision::Approved
            && (bom != Some(target.bom_revision)
                || specification != Some(target.specification_revision)
                || price != Some(target.price_revision)))
    {
        return Err(FactError::Conflict);
    }
    let revision = review.checked_add(1).ok_or(FactError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_factual_reviews(sku,bom_revision,specification_revision,price_revision,revision,effect_key,decision,evidence_ref,qualification_ref,actor,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(request.sku).bind(target.bom_revision).bind(target.specification_revision).bind(target.price_revision).bind(revision).bind(request.effect_key).bind(request.decision.code()).bind(request.evidence_ref).bind(request.qualification_ref).bind(&access.actor).bind(request.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(FactReceipt {
        revision,
        replayed: false,
    })
}
