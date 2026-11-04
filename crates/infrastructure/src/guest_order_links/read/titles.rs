use super::super::*;
use super::valid_timestamp;
use commerce_domain::{
    checkout_orders::OrderSnapshot, publication::PublicationChoice, translations::TranslationDraft,
};

#[derive(sqlx::FromRow)]
struct HistoricalPublication {
    choice: Option<String>,
    evaluated_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct HistoricalCopy {
    payload: Option<String>,
    approved: bool,
    source_payload: Option<String>,
}

// The accepted snapshot is already bounded to at most 20 validated products.
// Resolve exact immutable historical revisions, never current publication state.
pub(super) async fn validate_titles(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &OrderSnapshot,
) -> Result<(), LinkError> {
    let locale = &snapshot.quote.binding.locale;
    let skus: Vec<&str> = snapshot.products.iter().map(|p| p.sku.as_str()).collect();
    let revisions: Vec<i32> = snapshot
        .products
        .iter()
        .map(|p| p.publication_revision)
        .collect();
    let rows: Vec<HistoricalPublication> = sqlx::query_as(
        "SELECT CASE WHEN octet_length(p.choice::text) <= 8192 THEN p.choice::text END AS choice,
                p.evaluated_at
         FROM unnest($1::text[], $2::integer[]) WITH ORDINALITY AS requested(sku, revision, position)
         LEFT JOIN catalogue_publication_revisions p
           ON p.sku = requested.sku AND p.locale = $3 AND p.revision = requested.revision
         ORDER BY requested.position",
    )
    .bind(&skus)
    .bind(&revisions)
    .bind(locale)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    if rows.len() != snapshot.products.len() {
        return Err(LinkError::Unavailable);
    }
    let mut copy_revisions = Vec::with_capacity(rows.len());
    let mut review_revisions = Vec::with_capacity(rows.len());
    let mut specifications = Vec::with_capacity(rows.len());
    let mut source_review_revisions = Vec::with_capacity(rows.len());
    for (row, product) in rows.into_iter().zip(&snapshot.products) {
        let choice = row.choice.ok_or(LinkError::Unavailable)?;
        let choice: PublicationChoice =
            serde_json::from_str(&choice).map_err(|_| LinkError::Unavailable)?;
        if !choice.valid() || !row.evaluated_at.is_some_and(valid_timestamp) {
            return Err(LinkError::Unavailable);
        }
        let PublicationChoice::Activate { selection } = choice else {
            return Err(LinkError::Unavailable);
        };
        if selection.facts.bom_revision != product.bom_revision
            || (selection.source_review_revision.is_none() != (locale == "nl"))
        {
            return Err(LinkError::Unavailable);
        }
        copy_revisions.push(selection.translation_revision);
        review_revisions.push(selection.translation_review_revision);
        specifications.push(selection.facts.specification_revision);
        source_review_revisions.push(selection.source_review_revision.unwrap_or(0));
    }
    let copies: Vec<HistoricalCopy> = sqlx::query_as(
        "SELECT CASE WHEN octet_length(t.payload::text) <= 131072 THEN t.payload::text END AS payload,
                CASE WHEN octet_length(src.payload::text) <= 131072 THEN src.payload::text END AS source_payload,
                COALESCE(r.decision = 'approved' AND r.actor <> t.actor AND (
                    ($6 = 'nl' AND t.source_revision IS NULL AND requested.source_review_revision = 0)
                    OR ($6 <> 'nl' AND t.source_revision > 0 AND src.revision = t.source_revision
                        AND src.source_revision IS NULL AND sr.decision = 'approved' AND sr.actor <> src.actor)
                ), false) AS approved
         FROM unnest($1::text[], $2::integer[], $3::integer[], $4::integer[], $5::integer[])
              WITH ORDINALITY AS requested(sku, copy_revision, review_revision, specification, source_review_revision, position)
         LEFT JOIN catalogue_translation_revisions t
           ON t.sku = requested.sku AND t.locale = $6 AND t.revision = requested.copy_revision
          AND t.specification_revision = requested.specification
         LEFT JOIN catalogue_translation_reviews r
           ON r.sku = t.sku AND r.locale = t.locale AND r.translation_revision = t.revision
          AND r.revision = requested.review_revision
         LEFT JOIN catalogue_translation_revisions src
           ON src.sku = requested.sku AND src.locale = 'nl' AND src.revision = t.source_revision
          AND src.specification_revision = requested.specification
         LEFT JOIN catalogue_translation_reviews sr
           ON sr.sku = src.sku AND sr.locale = src.locale AND sr.translation_revision = src.revision
          AND sr.revision = requested.source_review_revision
         ORDER BY requested.position",
    )
    .bind(&skus)
    .bind(&copy_revisions)
    .bind(&review_revisions)
    .bind(&specifications)
    .bind(&source_review_revisions)
    .bind(locale)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| LinkError::Unavailable)?;
    if copies.len() != snapshot.products.len() {
        return Err(LinkError::Unavailable);
    }
    for (row, product) in copies.into_iter().zip(&snapshot.products) {
        let payload = row.payload.ok_or(LinkError::Unavailable)?;
        let copy: TranslationDraft =
            serde_json::from_str(&payload).map_err(|_| LinkError::Unavailable)?;
        if !row.approved || copy.validate().is_err() || copy.title != product.title {
            return Err(LinkError::Unavailable);
        }
        if locale != "nl" {
            let payload = row.source_payload.ok_or(LinkError::Unavailable)?;
            let source: TranslationDraft =
                serde_json::from_str(&payload).map_err(|_| LinkError::Unavailable)?;
            source.validate().map_err(|_| LinkError::Unavailable)?;
        }
    }
    Ok(())
}
