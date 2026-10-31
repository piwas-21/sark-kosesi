//! Trusted non-selling quote consumer. No HTTP route, persisted cart or provider I/O.
use crate::published_catalogue::{self, PublishedError, PublishedProduct};
use commerce_application::{Locale, request_context::Clock};
use commerce_domain::quotes::{
    MarketPolicy, PricedItem, QuoteError, QuoteRequest, QuoteTotals, calculate,
};
use sqlx::PgPool;
mod proof;
mod stock;
#[derive(Clone, Copy)]
enum StockClaim {
    Indicative,
    Reservation,
}
pub(crate) use proof::QuoteProof;

#[derive(Debug)]
pub enum QuoteFailure {
    Policy(QuoteError),
    Promotion(commerce_domain::promotions::PromotionError),
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
    let mut tx = published_catalogue::begin(pool).await?;
    let quote = in_transaction(&mut tx, clock, locale, request, policies).await?;
    tx.rollback().await?;
    Ok(quote)
}
#[derive(Debug, PartialEq, Eq)]
pub struct PromotedServerQuote {
    pub policy_id: String,
    pub policy_revision: i32,
    pub evaluated_at: i64,
    pub totals: commerce_domain::promotions::PromotionQuote,
}
pub(crate) async fn in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
) -> Result<ServerQuote, QuoteFailure> {
    Ok(evaluate(
        tx,
        clock,
        locale,
        request,
        policies,
        None,
        Some(StockClaim::Indicative),
    )
    .await?
    .original)
}
pub(crate) async fn promoted_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
    promotion: &commerce_domain::promotions::Promotion,
) -> Result<PromotedServerQuote, QuoteFailure> {
    let evaluation = evaluate(
        tx,
        clock,
        locale,
        request,
        policies,
        Some(promotion),
        Some(StockClaim::Indicative),
    )
    .await?;
    let totals = evaluation.promoted.ok_or(QuoteFailure::InvalidStoredData)?;
    Ok(PromotedServerQuote {
        policy_id: evaluation.original.policy_id,
        policy_revision: evaluation.original.policy_revision,
        evaluated_at: evaluation.original.evaluated_at,
        totals,
    })
}
async fn evaluate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
    promotion: Option<&commerce_domain::promotions::Promotion>,
    stock_claim: Option<StockClaim>,
) -> Result<QuoteProof, QuoteFailure> {
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
    let products = published_catalogue::load_eligible(tx, clock, locale, &skus).await?;
    if products.len() != skus.len() {
        return Err(QuoteError::ProductUnavailable.into());
    }
    let mut priced = Vec::with_capacity(products.len());
    let mut parcels = 0_u64;
    let mut weight = 0_u64;
    for eligible in &products {
        let product = &eligible.product;
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
    if let Some(claim) = stock_claim {
        stock::check(tx, &products, &request.lines, claim).await?;
    }
    let totals = calculate(
        &priced,
        policy.shipping.gross,
        policy.shipping.vat_basis_points,
    )?;
    let promoted = promotion
        .map(|p| {
            commerce_domain::promotions::apply(&totals, p, &request.destination.country, started)
        })
        .transpose()
        .map_err(QuoteFailure::Promotion)?;
    let mut proof = QuoteProof {
        original: ServerQuote {
            policy_id: policy.id.clone(),
            policy_revision: policy.revision,
            evaluated_at: started,
            totals,
        },
        promoted,
        policy: policy.clone(),
        promotion: promotion.cloned(),
        products,
        checked_at: started,
    };
    let evaluated_at = now(clock)?;
    proof.check_at(evaluated_at)?;
    proof.original.evaluated_at = evaluated_at;
    Ok(proof)
}
pub(crate) async fn prepare_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
) -> Result<QuoteProof, QuoteFailure> {
    evaluate(
        tx,
        clock,
        locale,
        request,
        policies,
        None,
        Some(StockClaim::Indicative),
    )
    .await
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

pub(crate) async fn prepare_order_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
) -> Result<QuoteProof, QuoteFailure> {
    evaluate(
        tx,
        clock,
        locale,
        request,
        policies,
        None,
        Some(StockClaim::Reservation),
    )
    .await
}

/// Retain current commercial/source claims only. No stock or payment authority:
/// the caller must claim the complete existing reservation in this same transaction.
pub(crate) async fn prepare_captured_sources_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    clock: &dyn Clock,
    locale: Locale,
    request: &QuoteRequest,
    policies: &[MarketPolicy],
) -> Result<QuoteProof, QuoteFailure> {
    evaluate(tx, clock, locale, request, policies, None, None).await
}
