use super::*;

pub async fn publication_status(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    sku: &str,
    locale: Locale,
) -> Result<Option<PublicationStatus>, PublicationError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !merchant_code(sku) {
        return Err(PublicationError::InvalidRequest);
    }
    // Claim before reading the latest publication so a concurrent withdrawal cannot
    // commit between selection and validation of the previously active revision.
    sqlx::query("SELECT sku FROM catalogue_variants WHERE sku=$1 FOR SHARE")
        .bind(sku)
        .execute(&mut *access.tx)
        .await?;
    let row=sqlx::query_as::<_,Stored>("SELECT sku,locale,revision,choice::text,evidence_ref,actor,reason,evaluated_at FROM catalogue_publication_revisions WHERE sku=$1 AND locale=$2 ORDER BY revision DESC LIMIT 1")
        .bind(sku).bind(locale.internal()).fetch_optional(&mut *access.tx).await?;
    let Some(row) = row else {
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(None);
    };
    let record = row.decode(sku, locale)?;
    let (approved, rights) = if let PublicationChoice::Activate { selection } = &record.choice {
        let claim: Option<String> = sqlx::query_scalar(
            "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=$1 FOR SHARE",
        )
        .bind(&selection.image.asset_id)
        .fetch_optional(&mut *access.tx)
        .await?;
        claim.ok_or(PublicationError::InvalidStoredData)?;
        match sources::load(&mut access.tx, sku, locale, selection).await {
            Ok(sources) => {
                let bound:Option<bool>=sqlx::query_scalar("SELECT publication_route_matches(r) FROM catalogue_family_routes r WHERE family_code=$1 AND locale=$2 AND slug=$3")
                    .bind(&sources.family_code).bind(locale.internal()).bind(&selection.family_slug).fetch_optional(&mut *access.tx).await?;
                if bound != Some(true) {
                    return Err(PublicationError::InvalidStoredData);
                }
                (
                    sources.approved(selection),
                    Some((sources.image.rights, sources.image.asset)),
                )
            }
            Err(PublicationError::NotEligible) => (false, None),
            Err(e) => return Err(e),
        }
    } else {
        (false, None)
    };
    let now = access.authorized_at(clock)?;
    let eligible = approved && current(&mut access.tx, sku, locale, &record.choice, now).await?;
    let final_now = access.authorized_at(clock)?;
    let eligible = eligible
        && rights.is_some_and(|(rights, asset)| rights.product_rights_active(&asset, final_now));
    access.tx.rollback().await?;
    Ok(Some(PublicationStatus {
        record,
        eligible,
        evaluated_at: final_now,
    }))
}
