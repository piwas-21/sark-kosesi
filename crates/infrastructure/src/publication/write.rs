use super::*;

pub async fn record_publication(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: PublicationRequest<'_>,
) -> Result<PublicationReceipt, PublicationError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !request.confirmed
        || !merchant_code(request.sku)
        || !valid_selection_locale(request.choice, request.locale)
        || !(0..i32::MAX).contains(&request.expected_revision)
        || !valid_command_id(request.effect_key)
        || !text(request.evidence_ref)
        || !text(request.reason)
    {
        return Err(PublicationError::InvalidRequest);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("catalogue-publication:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior=sqlx::query_as::<_,Stored>("SELECT sku,locale,revision,choice::text,evidence_ref,actor,reason,evaluated_at FROM catalogue_publication_revisions WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        if prior.sku != request.sku || prior.locale != request.locale.internal() {
            return Err(PublicationError::Conflict);
        }
        let prior = prior.decode(request.sku, request.locale)?;
        if prior.revision.checked_sub(1) != Some(request.expected_revision)
            || prior.choice != *request.choice
            || prior.evidence_ref != request.evidence_ref
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(PublicationError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(PublicationReceipt {
            revision: prior.revision,
            replayed: true,
        });
    }
    let exists: Option<String> = sqlx::query_scalar(
        "UPDATE catalogue_variants SET stock_policy=stock_policy WHERE sku=$1 RETURNING sku",
    )
    .bind(request.sku)
    .fetch_optional(&mut *access.tx)
    .await?;
    exists.ok_or(PublicationError::NotFound)?;
    let revision:i32=sqlx::query_scalar("SELECT coalesce(max(revision),0) FROM catalogue_publication_revisions WHERE sku=$1 AND locale=$2").bind(request.sku).bind(request.locale.internal()).fetch_one(&mut *access.tx).await?;
    if revision != request.expected_revision {
        return Err(PublicationError::Conflict);
    }
    let rights = if let PublicationChoice::Activate { selection } = request.choice {
        let claim: Option<String> = sqlx::query_scalar(
            "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=$1 FOR UPDATE",
        )
        .bind(&selection.image.asset_id)
        .fetch_optional(&mut *access.tx)
        .await?;
        claim.ok_or(PublicationError::NotEligible)?;
        let sources = sources::load(&mut access.tx, request.sku, request.locale, selection).await?;
        if !sources.approved(selection) {
            return Err(PublicationError::NotEligible);
        }
        let conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalogue_family_routes WHERE locale=$1 AND ((family_code=$2 AND slug<>$3) OR (slug=$3 AND family_code<>$2)))")
            .bind(request.locale.internal()).bind(&sources.family_code).bind(&selection.family_slug).fetch_one(&mut *access.tx).await?;
        if conflict {
            return Err(PublicationError::Conflict);
        }
        Some((sources.image.rights, sources.image.asset))
    } else {
        None
    };
    let now = access.authorized_at(clock)?;
    if matches!(request.choice, PublicationChoice::Activate { .. })
        && !current(
            &mut access.tx,
            request.sku,
            request.locale,
            request.choice,
            now,
        )
        .await?
    {
        return Err(PublicationError::NotEligible);
    }
    let next = revision
        .checked_add(1)
        .ok_or(PublicationError::InvalidRequest)?;
    let encoded =
        serde_json::to_string(request.choice).map_err(|_| PublicationError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_publication_revisions(sku,locale,revision,effect_key,choice,evidence_ref,actor,reason,evaluated_at) VALUES($1,$2,$3,$4,$5::jsonb,$6,$7,$8,$9)")
        .bind(request.sku).bind(request.locale.internal()).bind(next).bind(request.effect_key).bind(encoded).bind(request.evidence_ref).bind(&access.actor).bind(request.reason).bind(now).execute(&mut *access.tx).await?;
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
        .execute(&mut *access.tx)
        .await?;
    // Every source writer conflicts with the held variant/asset claims. After
    // the last DB await, only session and licence time can change eligibility.
    let final_now = access.authorized_at(clock)?;
    if let Some((rights, asset)) = rights
        && !rights.product_rights_active(&asset, final_now)
    {
        return Err(PublicationError::NotEligible);
    }
    access.tx.commit().await?;
    Ok(PublicationReceipt {
        revision: next,
        replayed: false,
    })
}
