use super::*;
pub struct MediaRightsRequest<'a> {
    pub asset_id: &'a str,
    pub expected_revision: i32,
    pub effect_key: &'a str,
    pub change: &'a MediaRightsChange,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(sqlx::FromRow)]
struct Prior {
    asset_id: String,
    revision: i32,
    change: String,
    actor: String,
    reason: String,
}
pub async fn record_media_rights(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: MediaRightsRequest<'_>,
) -> Result<MediaReceipt, MediaError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_envelope(
        request.asset_id,
        request.effect_key,
        request.reason,
        request.confirmed,
    ) || !(0..i32::MAX).contains(&request.expected_revision)
    {
        return Err(MediaError::InvalidRequest);
    }
    lock_effect(&mut access.tx, "media-rights", request.effect_key).await?;
    let claim: Option<(String,)> = sqlx::query_as(
        "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=$1 FOR UPDATE",
    )
    .bind(request.asset_id)
    .fetch_optional(&mut *access.tx)
    .await?;
    let metadata = asset(&mut access.tx, request.asset_id)
        .await?
        .ok_or(MediaError::NotFound)?;
    if claim.is_none() {
        return Err(MediaError::InvalidStoredData);
    }
    request
        .change
        .validate(&metadata)
        .map_err(|_| MediaError::InvalidRequest)?;
    let value = serde_json::to_value(request.change).map_err(|_| MediaError::InvalidRequest)?;
    let prior=sqlx::query_as::<_,Prior>("SELECT asset_id,revision,change::text,actor,reason FROM catalogue_media_rights WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        let stored: serde_json::Value =
            serde_json::from_str(&prior.change).map_err(|_| MediaError::InvalidStoredData)?;
        if prior.asset_id != request.asset_id
            || prior.revision - 1 != request.expected_revision
            || stored != value
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(MediaError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(MediaReceipt {
            revision: prior.revision,
            replayed: true,
        });
    }
    let (current,): (i32,) = sqlx::query_as(
        "SELECT coalesce(max(revision),0) FROM catalogue_media_rights WHERE asset_id=$1",
    )
    .bind(request.asset_id)
    .fetch_one(&mut *access.tx)
    .await?;
    if current != request.expected_revision {
        return Err(MediaError::Conflict);
    }
    let revision = current.checked_add(1).ok_or(MediaError::InvalidRequest)?;
    sqlx::query("INSERT INTO catalogue_media_rights(asset_id,revision,effect_key,change,actor,reason) VALUES($1,$2,$3,$4::jsonb,$5,$6)")
        .bind(request.asset_id).bind(revision).bind(request.effect_key).bind(value.to_string()).bind(&access.actor).bind(request.reason).execute(&mut *access.tx).await?;
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(MediaReceipt {
        revision,
        replayed: false,
    })
}
