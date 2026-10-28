//! Guarded media metadata and rights consumers; no upload or public route.
use super::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{
    media::{MediaAssetDraft, MediaRightsChange, valid_command_id},
    specifications::bounded_text,
};
use sqlx::{PgPool, Postgres, Transaction};
mod register;
mod rights;
pub use register::{RegisterMediaRequest, register_media};
pub use rights::{MediaRightsRequest, record_media_rights};
#[derive(Debug)]
pub enum MediaError {
    Staff(StaffError),
    InvalidRequest,
    NotFound,
    Conflict,
    InvalidStoredData,
    Database(sqlx::Error),
}
impl From<StaffError> for MediaError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<sqlx::Error> for MediaError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct MediaReceipt {
    pub revision: i32,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct MediaInspection {
    pub asset: MediaAssetDraft,
    pub rights_revision: Option<i32>,
    pub product_rights_active: bool,
    pub requires_generated_label: bool,
    pub evaluated_at: i64,
}
fn valid_envelope(asset_id: &str, effect_key: &str, reason: &str, confirmed: bool) -> bool {
    confirmed
        && valid_command_id(asset_id)
        && valid_command_id(effect_key)
        && bounded_text(reason, 500)
}
async fn lock_effect(
    tx: &mut Transaction<'_, Postgres>,
    namespace: &str,
    key: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("{namespace}:{key}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn asset(
    tx: &mut Transaction<'_, Postgres>,
    asset_id: &str,
) -> Result<Option<MediaAssetDraft>, MediaError> {
    let payload: Option<(String,)> =
        sqlx::query_as("SELECT payload::text FROM catalogue_media_assets WHERE asset_id=$1")
            .bind(asset_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((payload,)) = payload else {
        return Ok(None);
    };
    let value: MediaAssetDraft =
        serde_json::from_str(&payload).map_err(|_| MediaError::InvalidStoredData)?;
    value
        .validate()
        .map_err(|_| MediaError::InvalidStoredData)?;
    Ok(Some(value))
}
pub async fn inspect_media(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    asset_id: &str,
) -> Result<Option<MediaInspection>, MediaError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_command_id(asset_id) {
        return Err(MediaError::InvalidRequest);
    }
    // SHARE conflicts with the trigger's serialization-row update until return.
    let claim: Option<(String,)> = sqlx::query_as(
        "SELECT asset_id FROM catalogue_media_serialization WHERE asset_id=$1 FOR SHARE",
    )
    .bind(asset_id)
    .fetch_optional(&mut *access.tx)
    .await?;
    let Some(asset) = asset(&mut access.tx, asset_id).await? else {
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(None);
    };
    if claim.is_none() {
        return Err(MediaError::InvalidStoredData);
    }
    let row:Option<(i32,String)>=sqlx::query_as("SELECT revision,change::text FROM catalogue_media_rights WHERE asset_id=$1 ORDER BY revision DESC LIMIT 1")
        .bind(asset_id).fetch_optional(&mut *access.tx).await?;
    let now = access.authorized_at(clock)?;
    let (revision, active) = if let Some((revision, change)) = row {
        let change: MediaRightsChange =
            serde_json::from_str(&change).map_err(|_| MediaError::InvalidStoredData)?;
        change
            .validate(&asset)
            .map_err(|_| MediaError::InvalidStoredData)?;
        (Some(revision), change.product_rights_active(&asset, now))
    } else {
        (None, false)
    };
    let requires_generated_label =
        asset.kind == commerce_domain::media::MediaKind::GeneratedInspiration;
    access.tx.rollback().await?;
    Ok(Some(MediaInspection {
        asset,
        rights_revision: revision,
        product_rights_active: active,
        requires_generated_label,
        evaluated_at: now,
    }))
}
