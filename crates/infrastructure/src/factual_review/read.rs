use super::*;

#[derive(sqlx::FromRow)]
struct Sources {
    identity_key: String,
    bom_key: String,
    specification_actor: String,
    specification_reason: String,
    minor_units: i64,
    currency: String,
    price_actor: String,
    price_evidence_ref: String,
    price_reason: String,
}
impl Sources {
    fn validated_price(&self) -> Result<Money, FactError> {
        if !valid_command_id(&self.specification_actor)
            || !text(&self.specification_reason)
            || !valid_command_id(&self.price_actor)
            || !bounded_text(&self.price_evidence_ref, 512)
            || self.price_evidence_ref.contains('\0')
            || !text(&self.price_reason)
            || self.currency != "EUR"
        {
            return Err(FactError::InvalidStoredData);
        }
        Money::eur(self.minor_units).map_err(|_| FactError::InvalidStoredData)
    }
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    target: FactTarget,
) -> Result<Option<FactBundle>, FactError> {
    let row=sqlx::query_as::<_,Sources>("SELECT i.creation_effect_key AS identity_key,b.creation_effect_key AS bom_key,s.actor AS specification_actor,s.reason AS specification_reason,p.minor_units,p.currency,p.actor AS price_actor,p.evidence_ref AS price_evidence_ref,p.reason AS price_reason FROM catalogue_items i JOIN catalogue_boms b ON b.sku=i.sku AND b.revision=$2 AND b.sealed JOIN catalogue_specification_revisions s ON s.sku=i.sku AND s.revision=$3 AND s.bom_revision=b.revision JOIN catalogue_price_revisions p ON p.sku=i.sku AND p.revision=$4 WHERE i.sku=$1 AND NOT i.fixture")
        .bind(sku).bind(target.bom_revision).bind(target.specification_revision).bind(target.price_revision).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let price = row.validated_price()?;
    let identity = catalogue_authoring::load_event(tx, &row.identity_key)
        .await?
        .ok_or(FactError::InvalidStoredData)?;
    let bom = catalogue_authoring::load_event(tx, &row.bom_key)
        .await?
        .ok_or(FactError::InvalidStoredData)?;
    let specification = specifications::load(tx, sku, target.specification_revision)
        .await?
        .ok_or(FactError::InvalidStoredData)?;
    Ok(Some(FactBundle {
        sku: sku.to_owned(),
        target,
        identity,
        bom,
        specification,
        specification_actor: row.specification_actor,
        specification_reason: row.specification_reason,
        price,
        price_actor: row.price_actor,
        price_evidence_ref: row.price_evidence_ref,
        price_reason: row.price_reason,
        review: None,
    }))
}
pub(crate) async fn assembled(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    target: FactTarget,
) -> Result<Option<FactBundle>, FactError> {
    let mut bundle = load(tx, sku, target).await?;
    if let Some(bundle) = &mut bundle {
        let review=sqlx::query_as::<_,StoredReview>("SELECT sku,bom_revision,specification_revision,price_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM catalogue_factual_reviews WHERE sku=$1 AND bom_revision=$2 AND specification_revision=$3 AND price_revision=$4 ORDER BY revision DESC LIMIT 1")
            .bind(sku).bind(target.bom_revision).bind(target.specification_revision).bind(target.price_revision).fetch_optional(&mut **tx).await?;
        if let Some(review) = review {
            bundle.review = Some(review.validated(bundle)?);
        }
    }
    Ok(bundle)
}
/// A private inspection record, not a current publication-eligibility assertion.
pub async fn facts_for_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    target: FactTarget,
) -> Result<Option<FactBundle>, FactError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueReview).await?;
    if !merchant_code(sku) || !target.valid() {
        return Err(FactError::InvalidRequest);
    }
    let bundle = assembled(&mut access.tx, sku, target).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(bundle)
}

#[cfg(test)]
#[path = "../../tests/support/fact_sources.rs"]
mod tests;
