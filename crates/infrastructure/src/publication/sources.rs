use super::*;

pub(crate) struct Sources {
    pub family_code: String,
    pub facts: FactBundle,
    pub image: ImageReviewBundle,
    pub copy: TranslationRecord,
    pub source: Option<TranslationRecord>,
}
impl Sources {
    pub fn approved(&self, selection: &ApprovedSelection) -> bool {
        self.facts.review.as_ref().is_some_and(|r| {
            r.decision == FactDecision::Approved && r.revision == selection.factual_review_revision
        }) && self.image.review.as_ref().is_some_and(|r| {
            r.decision == ImageDecision::Approved && r.revision == selection.image_review_revision
        }) && self.copy.specification_revision == selection.facts.specification_revision
            && self.copy.review.as_ref().is_some_and(|r| {
                r.decision == TranslationDecision::Approved
                    && r.revision == selection.translation_review_revision
            })
            && self
                .source
                .as_ref()
                .map(|source| {
                    source.review.as_ref().is_some_and(|r| {
                        r.decision == TranslationDecision::Approved
                            && Some(r.revision) == selection.source_review_revision
                    })
                })
                .unwrap_or(selection.source_review_revision.is_none())
            && self.facts.price.minor_units() <= 9_007_199_254_740_991
    }
}
pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    sku: &str,
    locale: Locale,
    selection: &ApprovedSelection,
) -> Result<Sources, PublicationError> {
    let facts = factual_review::load_bundle(tx, sku, selection.facts)
        .await?
        .ok_or(PublicationError::NotEligible)?;
    let image = image_review::load_bundle(tx, sku, &selection.image)
        .await?
        .ok_or(PublicationError::NotEligible)?;
    let copy = translations::load_bundle(tx, sku, locale, selection.translation_revision)
        .await?
        .ok_or(PublicationError::NotEligible)?;
    let source = if let Some(revision) = copy.source_revision {
        Some(
            translations::load_bundle(tx, sku, Locale::Nl, revision)
                .await?
                .ok_or(PublicationError::InvalidStoredData)?,
        )
    } else {
        None
    };
    let family_code = if let CatalogueCommand::Variant { family_code, .. } = &facts.identity.command
    {
        family_code.clone()
    } else {
        return Err(PublicationError::InvalidStoredData);
    };
    Ok(Sources {
        family_code,
        facts,
        image,
        copy,
        source,
    })
}
