use super::*;

pub async fn published_page(
    pool: &PgPool,
    clock: &dyn Clock,
    locale: Locale,
    after: Option<&str>,
) -> Result<PublishedPage, PublishedError> {
    if after.is_some_and(|value| !merchant_code(value)) {
        return Err(PublishedError::InvalidRequest);
    }
    let mut tx = begin(pool).await?;
    let mut candidates:Vec<String>=sqlx::query_scalar("SELECT DISTINCT sku FROM catalogue_publication_revisions WHERE locale=$1 AND sku>$2 ORDER BY sku LIMIT 26")
        .bind(locale.internal()).bind(after.unwrap_or("")).fetch_all(&mut *tx).await?;
    let more = candidates.len() > PAGE_SIZE;
    candidates.truncate(PAGE_SIZE);
    let next_after = if more {
        candidates.last().cloned()
    } else {
        None
    };
    let items = load(&mut tx, clock, locale, &candidates).await?;
    tx.rollback().await?;
    Ok(PublishedPage { items, next_after })
}
pub async fn published_product(
    pool: &PgPool,
    clock: &dyn Clock,
    locale: Locale,
    sku: &str,
) -> Result<Option<PublishedProduct>, PublishedError> {
    if !merchant_code(sku) {
        return Err(PublishedError::InvalidRequest);
    }
    let mut tx = begin(pool).await?;
    let mut products = load(&mut tx, clock, locale, &[sku.to_owned()]).await?;
    tx.rollback().await?;
    Ok(products.pop())
}
async fn load(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    skus: &[String],
) -> Result<Vec<PublishedProduct>, PublishedError> {
    // Claim every variant before any shared asset, avoiding a cross-SKU/asset cycle
    // with a publisher which owns one variant and is waiting on a shared photograph.
    sqlx::query("SELECT sku FROM catalogue_variants WHERE sku=ANY($1) ORDER BY sku FOR SHARE")
        .bind(skus)
        .execute(&mut **tx)
        .await?;
    let mut selections = Vec::new();
    for sku in skus {
        if let Some(record) = publication::latest_record(tx, sku, locale).await?
            && let PublicationChoice::Activate { selection } = record.choice
        {
            selections.push((sku, record.revision, selection));
        }
    }
    let assets: Vec<&str> = selections
        .iter()
        .map(|(_, _, s)| s.image.asset_id.as_str())
        .collect();
    let claims:Vec<String>=sqlx::query_scalar("SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=ANY($1) ORDER BY asset_id FOR SHARE").bind(&assets).fetch_all(&mut **tx).await?;
    if assets
        .iter()
        .any(|asset| !claims.iter().any(|claim| claim == asset))
    {
        return Err(PublishedError::InvalidStoredData);
    }
    let mut prepared = Vec::new();
    for (sku, revision, selection) in selections {
        let sources = match publication::load_sources(tx, sku, locale, &selection).await {
            Ok(sources) => sources,
            Err(publication::PublicationError::NotEligible) => continue,
            Err(error) => return Err(error.into()),
        };
        if !sources.approved(&selection) {
            continue;
        }
        let binding:Option<bool>=sqlx::query_scalar("SELECT publication_route_matches(r) FROM catalogue_family_routes r WHERE family_code=$1 AND locale=$2 AND slug=$3")
            .bind(&sources.family_code).bind(locale.internal()).bind(&selection.family_slug).fetch_optional(&mut **tx).await?;
        if binding != Some(true) {
            return Err(PublishedError::InvalidStoredData);
        }
        let evaluated = now(clock)?;
        if !publication::current(
            tx,
            sku,
            locale,
            &PublicationChoice::Activate {
                selection: selection.clone(),
            },
            evaluated,
        )
        .await?
        {
            continue;
        }
        let in_stock = stock::available(
            tx,
            sku,
            selection.facts.bom_revision,
            product::lines(&sources)?,
        )
        .await?;
        prepared.push((sku.to_string(), revision, selection, sources, in_stock));
    }
    let evaluated = now(clock)?;
    prepared
        .into_iter()
        .filter(|(_, _, _, s, _)| {
            s.image
                .rights
                .product_rights_active(&s.image.asset, evaluated)
        })
        .map(|(sku, revision, selection, sources, in_stock)| {
            product::assemble(sku, locale, revision, selection, sources, in_stock)
        })
        .collect()
}
