use super::*;

pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    locale: Locale,
    revision: i32,
) -> Result<Option<TranslationRecord>, TranslationError> {
    let row = sqlx::query_as::<_,StoredDraft>("SELECT sku,locale,revision,specification_revision,source_revision,payload::text,actor,reason FROM catalogue_translation_revisions WHERE sku=$1 AND locale=$2 AND revision=$3")
        .bind(sku).bind(locale.internal()).bind(revision).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = decode(row)?;
    let references:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalogue_specification_revisions WHERE sku=$1 AND revision=$2) AND ($3='nl' OR EXISTS(SELECT 1 FROM catalogue_translation_revisions WHERE sku=$1 AND locale='nl' AND revision=$4 AND specification_revision=$2))")
        .bind(&record.sku).bind(record.specification_revision).bind(record.locale.internal()).bind(record.source_revision).fetch_one(&mut **tx).await?;
    if !references {
        return Err(TranslationError::InvalidStoredData);
    }
    if let Some(source) = record.source_revision {
        validate_source_copy(tx, &record.sku, source, record.specification_revision).await?;
    }
    Ok(Some(record))
}
pub(super) async fn validate_source_copy(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    revision: i32,
    specification: i32,
) -> Result<(), TranslationError> {
    let payload:Option<String>=sqlx::query_scalar("SELECT payload::text FROM catalogue_translation_revisions WHERE sku=$1 AND locale='nl' AND revision=$2 AND specification_revision=$3 AND source_revision IS NULL")
        .bind(sku).bind(revision).bind(specification).fetch_optional(&mut **tx).await?;
    decode_content(&payload.ok_or(TranslationError::InvalidStoredData)?)?;
    Ok(())
}
fn decode_content(payload: &str) -> Result<TranslationDraft, TranslationError> {
    if payload.len() > 131072 {
        return Err(TranslationError::InvalidStoredData);
    }
    let draft: TranslationDraft =
        serde_json::from_str(payload).map_err(|_| TranslationError::InvalidStoredData)?;
    draft
        .validate()
        .map_err(|_| TranslationError::InvalidStoredData)?;
    Ok(draft)
}
fn decode(row: StoredDraft) -> Result<TranslationRecord, TranslationError> {
    let locale = Locale::from_internal(&row.locale).ok_or(TranslationError::InvalidStoredData)?;
    if !valid_sku(&row.sku)
        || row.revision <= 0
        || row.specification_revision <= 0
        || !valid_source(locale, row.source_revision)
        || !valid_command_id(&row.actor)
        || !text(&row.reason)
    {
        return Err(TranslationError::InvalidStoredData);
    }
    let draft = decode_content(&row.payload)?;
    Ok(TranslationRecord {
        sku: row.sku,
        locale,
        revision: row.revision,
        specification_revision: row.specification_revision,
        source_revision: row.source_revision,
        draft,
        actor: row.actor,
        reason: row.reason,
        review: None,
    })
}

#[cfg(test)]
#[path = "../../tests/support/translation_rows.rs"]
mod tests;
/// Authoring access; a draft or review record does not activate public copy.
pub async fn translation(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    locale: Locale,
    revision: i32,
) -> Result<Option<TranslationRecord>, TranslationError> {
    inspect(
        pool,
        token,
        clock,
        sku,
        locale,
        revision,
        Capability::CatalogueManage,
    )
    .await
}
/// Reviewers have their own read capability and need no authoring grant.
pub async fn translation_for_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    locale: Locale,
    revision: i32,
) -> Result<Option<TranslationRecord>, TranslationError> {
    inspect(
        pool,
        token,
        clock,
        sku,
        locale,
        revision,
        Capability::CatalogueReview,
    )
    .await
}
pub(crate) async fn assembled(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    locale: Locale,
    revision: i32,
) -> Result<Option<TranslationRecord>, TranslationError> {
    let mut record = load(tx, sku, locale, revision).await?;
    if let Some(draft) = &mut record {
        let row = sqlx::query_as::<_,StoredReview>("SELECT sku,locale,translation_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM catalogue_translation_reviews WHERE sku=$1 AND locale=$2 AND translation_revision=$3 ORDER BY revision DESC LIMIT 1")
            .bind(sku).bind(locale.internal()).bind(revision).fetch_optional(&mut **tx).await?;
        if let Some(review) = row {
            draft.review = Some(decode_review(review, &draft.actor)?);
        }
    }
    Ok(record)
}
async fn inspect(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    locale: Locale,
    revision: i32,
    capability: Capability,
) -> Result<Option<TranslationRecord>, TranslationError> {
    let mut access = lock_staff(pool, token, clock, capability).await?;
    if !valid_sku(sku) || revision <= 0 {
        return Err(TranslationError::InvalidRequest);
    }
    let record = assembled(&mut access.tx, sku, locale, revision).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(record)
}
fn decode_review(
    review: StoredReview,
    author: &str,
) -> Result<TranslationReview, TranslationError> {
    if review.revision <= 0
        || !valid_command_id(&review.actor)
        || review.actor == author
        || !text(&review.evidence_ref)
        || !text(&review.qualification_ref)
        || !text(&review.reason)
    {
        return Err(TranslationError::InvalidStoredData);
    }
    let decision: TranslationDecision =
        serde_json::from_value(serde_json::Value::String(review.decision))
            .map_err(|_| TranslationError::InvalidStoredData)?;
    Ok(TranslationReview {
        revision: review.revision,
        decision,
        actor: review.actor,
        evidence_ref: review.evidence_ref,
        qualification_ref: review.qualification_ref,
        reason: review.reason,
    })
}
