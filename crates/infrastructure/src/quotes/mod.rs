//! Trusted non-selling quote consumer. No HTTP route, persisted cart or provider I/O.
use crate::{
    publication,
    published_catalogue::{self, PublishedError, PublishedProduct},
};
use commerce_application::{Locale, request_context::Clock};
use commerce_domain::quotes::{
    MarketPolicy, PricedItem, QuoteError, QuoteRequest, QuoteTotals, calculate,
};
use sqlx::PgPool;
mod stock;

#[derive(Debug)]
pub enum QuoteFailure {
    Policy(QuoteError),
    Catalogue(PublishedError),
    ClockUnavailable,
    InvalidStoredData,
    Database(sqlx::Error),
}
impl From<QuoteError> for QuoteFailure {
    fn from(e: QuoteError) -> Self {
        Self::Policy(e)
    }
}
impl From<PublishedError> for QuoteFailure {
    fn from(e: PublishedError) -> Self {
        Self::Catalogue(e)
    }
}
impl From<sqlx::Error> for QuoteFailure {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct ServerQuote {
    pub policy_id: String,
    pub policy_revision: i32,
    pub evaluated_at: i64,
    pub totals: QuoteTotals,
}
fn now(clock: &dyn Clock) -> Result<i64, QuoteFailure> {
    i64::try_from(
        clock
            .unix_seconds()
            .map_err(|_| QuoteFailure::ClockUnavailable)?,
    )
    .map_err(|_| QuoteFailure::ClockUnavailable)
}
/// The policy is reviewed server configuration, never a customer request value.
/// No policies are installed. A quote is indicative; checkout must revalidate and
/// reserve atomically before any charge. Discounts are deliberately unsupported.
pub async fn server_quote(
    pool: &PgPool,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
) -> Result<ServerQuote, QuoteFailure> {
    request.validate()?;
    if policies.len() > 64 {
        return Err(QuoteError::InvalidPolicy.into());
    }
    let started = now(clock)?;
    let mut matching = policies
        .iter()
        .filter(|p| p.destination == request.destination);
    let policy = matching.next().ok_or(QuoteError::MarketUnavailable)?;
    if matching.next().is_some() {
        return Err(QuoteError::InvalidPolicy.into());
    }
    policy.permits(&request.destination, started)?;
    let mut skus: Vec<_> = request.lines.iter().map(|l| l.sku.clone()).collect();
    skus.sort();
    let mut tx = published_catalogue::begin(pool).await?;
    let products = published_catalogue::load(&mut tx, clock, locale, &skus).await?;
    if products.len() != skus.len() {
        return Err(QuoteError::ProductUnavailable.into());
    }
    let mut priced = Vec::with_capacity(products.len());
    let mut parcels = 0_u64;
    let mut weight = 0_u64;
    for product in &products {
        let quantity = request
            .lines
            .iter()
            .find(|l| l.sku == product.sku)
            .ok_or(QuoteFailure::InvalidStoredData)?
            .quantity;
        let rule = policy.product(&product.sku, product.publication_revision, quantity)?;
        shipping(product, quantity, policy, &mut parcels, &mut weight)?;
        priced.push(PricedItem {
            sku: product.sku.clone(),
            publication_revision: product.publication_revision,
            quantity,
            unit_gross: product.price,
            vat_basis_points: rule.vat_basis_points,
        });
    }
    stock::check(&mut tx, &products, &request.lines).await?;
    let totals = calculate(
        &priced,
        policy.shipping.gross,
        policy.shipping.vat_basis_points,
    )?;
    // Variant and asset claims still span all reads. Evaluate every source's timed
    // eligibility after stock waits, using one final clock for the whole basket.
    let mut rights = Vec::with_capacity(products.len());
    for product in &products {
        let record = publication::latest_record(&mut tx, &product.sku, locale)
            .await
            .map_err(PublishedError::from)?
            .ok_or(QuoteFailure::InvalidStoredData)?;
        let commerce_domain::publication::PublicationChoice::Activate { selection } = record.choice
        else {
            return Err(QuoteFailure::InvalidStoredData);
        };
        let sources = publication::load_sources(&mut tx, &product.sku, locale, &selection)
            .await
            .map_err(PublishedError::from)?;
        rights.push((sources.image.rights, sources.image.asset));
    }
    let evaluated_at = now(clock)?;
    if evaluated_at < started {
        return Err(QuoteFailure::ClockUnavailable);
    }
    policy.permits(&request.destination, evaluated_at)?;
    if rights
        .iter()
        .any(|(right, asset)| !right.product_rights_active(asset, evaluated_at))
    {
        return Err(QuoteError::ProductUnavailable.into());
    }
    tx.rollback().await?;
    Ok(ServerQuote {
        policy_id: policy.id.clone(),
        policy_revision: policy.revision,
        evaluated_at,
        totals,
    })
}
fn shipping(
    product: &PublishedProduct,
    quantity: u32,
    policy: &MarketPolicy,
    parcels: &mut u64,
    weight: &mut u64,
) -> Result<(), QuoteFailure> {
    for parcel in &product.parcels {
        let d = &parcel.dimensions;
        if [d.width_mm, d.depth_mm, d.height_mm].iter().any(|edge| {
            edge.checked_add(d.tolerance_mm)
                .is_none_or(|v| v > policy.shipping.max_edge_mm)
        }) {
            return Err(QuoteError::ShippingUnavailable.into());
        }
        *parcels = parcels
            .checked_add(u64::from(quantity))
            .ok_or(QuoteError::ShippingUnavailable)?;
        *weight = weight
            .checked_add(u64::from(parcel.gross_weight_g) * u64::from(quantity))
            .ok_or(QuoteError::ShippingUnavailable)?;
    }
    if *parcels > u64::from(policy.shipping.max_parcels)
        || *weight > policy.shipping.max_total_weight_g
    {
        return Err(QuoteError::ShippingUnavailable.into());
    }
    Ok(())
}
