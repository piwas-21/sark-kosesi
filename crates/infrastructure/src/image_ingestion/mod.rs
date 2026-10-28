//! Verified private image ingestion; there is no public delivery/upload route.
use super::{
    media,
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{request_context::Clock, staff_access::Capability};
use commerce_domain::{media::valid_command_id, specifications::bounded_text};
use sqlx::PgPool;
mod process;
mod store;
static INGESTIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
fn ingestion_slot() -> Result<tokio::sync::SemaphorePermit<'static>, ImageError> {
    INGESTIONS.try_acquire().map_err(|_| ImageError::Busy)
}
pub const RECIPE: &str = "image-v1";
pub const MAX_ORIGINAL_BYTES: usize = 20 * 1024 * 1024;
#[derive(Debug)]
pub enum ImageError {
    Staff(StaffError),
    Media(media::MediaError),
    Database(sqlx::Error),
    InvalidRequest,
    NotFound,
    Conflict,
    MetadataMismatch,
    InvalidImage,
    UnsupportedImage,
    ResourceLimit,
    Busy,
    ProcessingUnavailable,
    InvalidStoredData,
}
impl From<StaffError> for ImageError {
    fn from(value: StaffError) -> Self {
        Self::Staff(value)
    }
}
impl From<media::MediaError> for ImageError {
    fn from(value: media::MediaError) -> Self {
        Self::Media(value)
    }
}
impl From<sqlx::Error> for ImageError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}
impl From<image::ImageError> for ImageError {
    fn from(_: image::ImageError) -> Self {
        Self::InvalidImage
    }
}
pub struct IngestImageRequest<'a> {
    pub asset_id: &'a str,
    pub effect_key: &'a str,
    pub bytes: &'a [u8],
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ImageReceipt {
    pub asset_id: String,
    pub recipe: &'static str,
    pub display_width: i32,
    pub display_height: i32,
    pub renditions: Vec<RenditionMetadata>,
    pub replayed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct RenditionMetadata {
    pub edge: i32,
    pub width: i32,
    pub height: i32,
    pub image_type: String,
    pub sha256: Vec<u8>,
    pub byte_length: i32,
}
#[derive(Debug)]
pub struct PrivateRendition {
    pub image_type: String,
    pub width: i32,
    pub height: i32,
    pub bytes: Vec<u8>,
}
/// Authorization before CPU work and again before effects; no DB transaction spans decoding.
pub async fn ingest_image(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    request: IngestImageRequest<'_>,
) -> Result<ImageReceipt, ImageError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !request.confirmed
        || !valid_command_id(request.asset_id)
        || !valid_command_id(request.effect_key)
        || !bounded_text(request.reason, 500)
        || request.bytes.is_empty()
        || request.bytes.len() > MAX_ORIGINAL_BYTES
    {
        return Err(ImageError::InvalidRequest);
    }
    let metadata = media::asset(&mut access.tx, request.asset_id)
        .await?
        .ok_or(ImageError::NotFound)?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    let _pipeline = ingestion_slot()?;
    let decoded = process::run(metadata, request.bytes.to_vec()).await?;
    store::persist(pool, token, clock, request, decoded).await
}
pub async fn image_receipt(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    asset_id: &str,
) -> Result<Option<ImageReceipt>, ImageError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_command_id(asset_id) {
        return Err(ImageError::InvalidRequest);
    }
    let receipt = store::receipt(&mut access.tx, asset_id, false).await?;
    access.check_time(clock)?;
    access.tx.rollback().await?;
    Ok(receipt)
}
#[derive(sqlx::FromRow)]
struct StoredRendition {
    image_type: String,
    width_px: i32,
    height_px: i32,
    bytes: Vec<u8>,
    sha256: Vec<u8>,
}
pub async fn private_rendition(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    asset_id: &str,
    edge: i32,
) -> Result<Option<PrivateRendition>, ImageError> {
    let mut access = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    if !valid_command_id(asset_id) || ![320, 640, 960].contains(&edge) {
        return Err(ImageError::InvalidRequest);
    }
    let row=sqlx::query_as::<_,StoredRendition>("SELECT image_type,width_px,height_px,bytes,sha256 FROM catalogue_image_renditions WHERE asset_id=$1 AND recipe='image-v1' AND edge=$2")
        .bind(asset_id).bind(edge).fetch_optional(&mut *access.tx).await?;
    let Some(StoredRendition {
        image_type,
        width_px: width,
        height_px: height,
        bytes,
        sha256: digest,
    }) = row
    else {
        access.check_time(clock)?;
        access.tx.rollback().await?;
        return Ok(None);
    };
    access.check_time(clock)?;
    access.tx.rollback().await?;
    let result = process::validate_output(image_type, width, height, bytes, digest).await?;
    let fresh = lock_staff(pool, token, clock, Capability::CatalogueManage).await?;
    fresh.check_time(clock)?;
    fresh.tx.rollback().await?;
    Ok(Some(result))
}
