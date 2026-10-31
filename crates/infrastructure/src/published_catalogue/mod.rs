//! Safe approved product projections. HTTP preview authentication is a separate boundary.
use super::{
    image_ingestion::{self, ImageError, PrivateRendition},
    publication,
};
use commerce_application::{Locale, request_context::Clock};
use commerce_domain::{
    Money,
    catalogue_authoring::{BomLineDraft, CatalogueCommand, merchant_code},
    inventory::StockBalance,
    publication::{ApprovedSelection, PublicationChoice},
    specifications::{DimensionsMm, FibrePart, MaterialSpecification},
    translations::TranslationDraft,
};
use sqlx::{PgPool, Postgres, Transaction};
mod image;
mod product;
mod read;
mod stock;
pub use image::published_image;
pub(crate) use read::load_eligible;
pub use read::{published_page, published_product};
pub const PAGE_SIZE: usize = 25;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedProduct {
    pub sku: String,
    pub locale: Locale,
    pub publication_revision: i32,
    pub family_code: String,
    pub family_slug: String,
    pub size_code: String,
    pub textile_code: String,
    pub fill: String,
    pub price: Money,
    pub copy: TranslationDraft,
    pub footprint: DimensionsMm,
    pub components: Vec<PublishedComponent>,
    pub parcels: Vec<PublishedParcel>,
    pub image: PublishedImage,
    pub in_stock: bool,
}
/// Internal eligibility context retained only while the caller holds source claims.
/// Kept separate from the customer projection and never exposed by HTTP.
pub(crate) struct EligibleProduct {
    pub product: PublishedProduct,
    pub(crate) bom_revision: i32,
    rights: commerce_domain::media::MediaRightsChange,
    asset: commerce_domain::media::MediaAssetDraft,
}
impl EligibleProduct {
    pub(crate) fn rights_active(&self, now: i64) -> bool {
        self.rights.product_rights_active(&self.asset, now)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedComponent {
    pub code: String,
    pub quantity: u32,
    pub dimensions: DimensionsMm,
    pub material: PublishedMaterial,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishedMaterial {
    Cover { fibres: Vec<FibrePart> },
    Foam { density_g_per_m3: u32 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedParcel {
    pub dimensions: DimensionsMm,
    pub gross_weight_g: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedImage {
    pub renditions: Vec<PublishedRendition>,
    pub(crate) asset_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedRendition {
    pub edge: i32,
    pub width: i32,
    pub height: i32,
    pub(crate) sha256: Vec<u8>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct PublishedPage {
    pub items: Vec<PublishedProduct>,
    pub next_after: Option<String>,
}
#[derive(Debug)]
pub enum PublishedError {
    InvalidRequest,
    InvalidStoredData,
    ClockUnavailable,
    Database(sqlx::Error),
    Publication(publication::PublicationError),
    Image(ImageError),
}
impl From<sqlx::Error> for PublishedError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl From<publication::PublicationError> for PublishedError {
    fn from(e: publication::PublicationError) -> Self {
        Self::Publication(e)
    }
}
impl From<ImageError> for PublishedError {
    fn from(e: ImageError) -> Self {
        Self::Image(e)
    }
}
fn now(clock: &dyn Clock) -> Result<i64, PublishedError> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| PublishedError::ClockUnavailable)?,
    )
    .map_err(|_| PublishedError::ClockUnavailable)
}
pub(crate) async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>, PublishedError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='1s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
