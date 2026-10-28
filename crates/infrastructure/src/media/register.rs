use super::*;
pub struct RegisterMediaRequest<'a> {
    pub asset_id: &'a str,
    pub effect_key: &'a str,
    pub draft: &'a MediaAssetDraft,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(sqlx::FromRow)]
struct Prior {
    asset_id: String,
    payload: String,
    actor: String,
    reason: String,
}
pub async fn register_media(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: RegisterMediaRequest<'_>,
) -> Result<MediaReceipt, MediaError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_envelope(
        request.asset_id,
        request.effect_key,
        request.reason,
        request.confirmed,
    ) {
        return Err(MediaError::InvalidRequest);
    }
    request
        .draft
        .validate()
        .map_err(|_| MediaError::InvalidRequest)?;
    let value = serde_json::to_value(request.draft).map_err(|_| MediaError::InvalidRequest)?;
    lock_effect(&mut access.tx, "media-register", request.effect_key).await?;
    let prior=sqlx::query_as::<_,Prior>("SELECT asset_id,payload::text,actor,reason FROM catalogue_media_assets WHERE effect_key=$1")
        .bind(request.effect_key).fetch_optional(&mut *access.tx).await?;
    if let Some(prior) = prior {
        let stored: serde_json::Value =
            serde_json::from_str(&prior.payload).map_err(|_| MediaError::InvalidStoredData)?;
        if prior.asset_id != request.asset_id
            || stored != value
            || prior.actor != access.actor
            || prior.reason != request.reason
        {
            return Err(MediaError::Conflict);
        }
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(MediaReceipt {
            revision: 0,
            replayed: true,
        });
    }
    let inserted:Option<(String,)>=sqlx::query_as("INSERT INTO catalogue_media_assets(asset_id,effect_key,payload,actor,reason) VALUES($1,$2,$3::jsonb,$4,$5) ON CONFLICT DO NOTHING RETURNING asset_id")
        .bind(request.asset_id).bind(request.effect_key).bind(value.to_string()).bind(&access.actor).bind(request.reason).fetch_optional(&mut *access.tx).await?;
    if inserted.is_none() {
        return Err(MediaError::Conflict);
    }
    access.check_time(clock)?;
    access.tx.commit().await?;
    Ok(MediaReceipt {
        revision: 0,
        replayed: false,
    })
}
