use super::*;

#[derive(sqlx::FromRow)]
struct Sources {
    identity_key: String,
    bom_key: String,
    specification_actor: String,
    registration_actor: String,
    ingestion_actor: String,
    rights_actor: String,
    change: String,
    sha256: Vec<u8>,
    intact: bool,
}
impl Sources {
    fn rights(&self, asset: &MediaAssetDraft) -> Result<MediaRightsChange, ImageReviewError> {
        if !self.intact
            || self.sha256 != asset.sha256
            || [
                &self.specification_actor,
                &self.registration_actor,
                &self.ingestion_actor,
                &self.rights_actor,
            ]
            .iter()
            .any(|actor| !valid_command_id(actor))
        {
            return Err(ImageReviewError::InvalidStoredData);
        }
        let rights: MediaRightsChange =
            serde_json::from_str(&self.change).map_err(|_| ImageReviewError::InvalidStoredData)?;
        rights
            .validate(asset)
            .map_err(|_| ImageReviewError::InvalidStoredData)?;
        Ok(rights)
    }
}
pub(super) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    target: &ImageTarget,
) -> Result<Option<ImageReviewBundle>, ImageReviewError> {
    let row=sqlx::query_as::<_,Sources>("SELECT i.creation_effect_key AS identity_key,b.creation_effect_key AS bom_key,s.actor AS specification_actor,m.actor AS registration_actor,o.actor AS ingestion_actor,r.actor AS rights_actor,r.change::text,o.sha256,(o.sha256=sha256(o.bytes) AND NOT EXISTS(SELECT 1 FROM catalogue_image_renditions d WHERE d.asset_id=o.asset_id AND d.recipe=o.recipe AND d.sha256<>sha256(d.bytes))) AS intact FROM catalogue_items i JOIN catalogue_boms b ON b.sku=i.sku AND b.revision=$2 AND b.sealed JOIN catalogue_specification_revisions s ON s.sku=i.sku AND s.revision=$3 AND s.bom_revision=b.revision CROSS JOIN catalogue_media_assets m JOIN catalogue_image_originals o ON o.asset_id=m.asset_id AND o.recipe='image-v1' JOIN catalogue_media_rights r ON r.asset_id=m.asset_id AND r.revision=$5 WHERE i.sku=$1 AND NOT i.fixture AND m.asset_id=$4")
        .bind(sku).bind(target.bom_revision).bind(target.specification_revision).bind(&target.asset_id).bind(target.rights_revision).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let asset = media::asset(tx, &target.asset_id)
        .await?
        .ok_or(ImageReviewError::InvalidStoredData)?;
    let rights = row.rights(&asset)?;
    let identity = catalogue_authoring::load_event(tx, &row.identity_key)
        .await?
        .ok_or(ImageReviewError::InvalidStoredData)?;
    let bom = catalogue_authoring::load_event(tx, &row.bom_key)
        .await?
        .ok_or(ImageReviewError::InvalidStoredData)?;
    let specification = specifications::load(tx, sku, target.specification_revision)
        .await?
        .ok_or(ImageReviewError::InvalidStoredData)?;
    let image = image_ingestion::load_receipt(tx, &target.asset_id, false)
        .await?
        .ok_or(ImageReviewError::InvalidStoredData)?;
    let mut bundle = ImageReviewBundle {
        sku: sku.into(),
        target: target.clone(),
        identity,
        bom,
        specification,
        specification_actor: row.specification_actor,
        asset,
        registration_actor: row.registration_actor,
        ingestion_actor: row.ingestion_actor,
        rights_actor: row.rights_actor,
        rights,
        image,
        review: None,
    };
    let review=sqlx::query_as::<_,StoredReview>("SELECT sku,asset_id,bom_revision,specification_revision,rights_revision,revision,decision,evidence_ref,qualification_ref,actor,reason FROM catalogue_image_reviews WHERE sku=$1 AND asset_id=$2 AND bom_revision=$3 AND specification_revision=$4 AND rights_revision=$5 ORDER BY revision DESC LIMIT 1")
        .bind(sku).bind(&target.asset_id).bind(target.bom_revision).bind(target.specification_revision).bind(target.rights_revision).fetch_optional(&mut **tx).await?;
    if let Some(review) = review {
        bundle.review = Some(review.validated(&bundle)?);
    }
    Ok(Some(bundle))
}
/// Exact historical sources for inspection; this does not assert current eligibility.
pub async fn image_review_bundle(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    target: &ImageTarget,
) -> Result<Option<ImageReviewBundle>, ImageReviewError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueReview).await?;
    if !merchant_code(sku) || !target.valid() {
        return Err(ImageReviewError::InvalidRequest);
    }
    let result = load(&mut access.tx, sku, target).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(result)
}
/// Only sanitized private derivatives; the reviewer grant never grants authoring/original access.
pub async fn image_for_review(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    target: &ImageTarget,
    edge: i32,
) -> Result<Option<PrivateRendition>, ImageReviewError> {
    if image_review_bundle(pool, token, clock, sku, target)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    Ok(image_ingestion::rendition(
        pool,
        token,
        clock,
        &target.asset_id,
        edge,
        Capability::CatalogueReview,
    )
    .await?)
}

#[cfg(test)]
#[path = "../../tests/support/image_review_sources.rs"]
mod tests;
