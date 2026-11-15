//! Private projection of one proposed whole-variant batch, without shipment custody.
use crate::{
    late_stock::{self, LateStockError, Outcome},
    order_stock::{self, SettlementError},
    payment_attempts::{PaymentStoreError, recognition},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use commerce_domain::{
    order_shipment_components::{ExpectedComponent, expected_components},
    order_shipments::SkuUnits,
};
use sqlx::PgPool;

#[derive(Debug)]
pub enum PlanningError {
    InvalidRequest,
    Unavailable,
    InvalidStoredData,
    Staff(StaffError),
    Database(sqlx::Error),
}

/// Expected components of this proposal alone. No shipment, reservation, stock
/// movement or dispatch authority follows, and previous shipments are not known.
#[derive(Debug, PartialEq, Eq)]
pub struct Projection {
    pub components: Vec<ExpectedComponent>,
}

/// Project one bounded proposal against the original accepted order only.
/// Staff authority and complete payment/allocation custody are held through the
/// read; callers must not treat zero assumed prior units as unshipped stock.
pub async fn project(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order_id: &str,
    proposed: &[SkuUnits],
) -> Result<Projection, PlanningError> {
    if !commerce_domain::order_stock::identifier(order_id)
        || proposed.is_empty()
        || proposed.len() > 20
    {
        return Err(PlanningError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::WarehouseManage)
        .await
        .map_err(PlanningError::Staff)?;
    let attempt = order_stock::source::attempt(&mut access.tx, order_id)
        .await
        .map_err(settlement_error)?;
    order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(settlement_error)?;
    let (frozen, recognition) = recognition::current_in_transaction(&mut access.tx, &attempt)
        .await
        .map_err(payment_error)?;
    if !recognition.is_some_and(|r| r.state == recognition::State::Received) {
        return Err(PlanningError::Unavailable);
    }
    let order = crate::checkout_orders::snapshot_in_transaction(
        &mut access.tx,
        &frozen.snapshot().binding.basket_id,
        order_id,
    )
    .await
    .map_err(|_| PlanningError::InvalidStoredData)?;
    let members = match late_stock::read_in_transaction(&mut access.tx, order_id)
        .await
        .map_err(late_error)?
    {
        Some(Outcome::Original(s)) if s.attempt_id == attempt => s.footprint.members,
        Some(Outcome::Late(s)) if s.attempt_id == attempt => s
            .footprint
            .original
            .into_iter()
            .map(|member| member.member)
            .collect(),
        Some(Outcome::NeedsReview(_)) | None => return Err(PlanningError::Unavailable),
        _ => return Err(PlanningError::InvalidStoredData),
    };
    let mut boms = Vec::with_capacity(order.quote.lines.len());
    for line in &order.quote.lines {
        // Bound the existing loader before it allocates its result.
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM catalogue_bom_lines WHERE sku=$1 AND revision=$2 LIMIT 65) bounded")
            .bind(&line.sku).bind(line.bom_revision).fetch_one(&mut *access.tx).await
            .map_err(PlanningError::Database)?;
        if !(1..=64).contains(&count) {
            return Err(PlanningError::InvalidStoredData);
        }
        let bom = crate::bom::load(&mut *access.tx, &line.sku, line.bom_revision)
            .await
            .map_err(PlanningError::Database)?
            .ok_or(PlanningError::InvalidStoredData)?;
        if usize::try_from(count).ok() != Some(bom.components.len()) {
            return Err(PlanningError::InvalidStoredData);
        }
        boms.push(bom);
    }
    let prior: Vec<_> = order
        .quote
        .lines
        .iter()
        .map(|line| SkuUnits {
            sku: line.sku.clone(),
            units: 0,
        })
        .collect();
    let components = expected_components(&order, &prior, proposed, &members, &boms).map_err(
        |error| match error {
            commerce_domain::order_shipment_components::ShipmentComponentError::Quantities(
                commerce_domain::order_shipments::ShipmentQuantityError::InvalidQuantities,
            ) => PlanningError::InvalidRequest,
            _ => PlanningError::InvalidStoredData,
        },
    )?;
    access.check_time(&observed).map_err(PlanningError::Staff)?;
    access
        .tx
        .rollback()
        .await
        .map_err(PlanningError::Database)?;
    Ok(Projection { components })
}

fn settlement_error(error: SettlementError) -> PlanningError {
    match error {
        SettlementError::NotFound | SettlementError::Unavailable => PlanningError::Unavailable,
        _ => PlanningError::InvalidStoredData,
    }
}
fn payment_error(error: PaymentStoreError) -> PlanningError {
    match error {
        PaymentStoreError::Unavailable | PaymentStoreError::NotFound => PlanningError::Unavailable,
        _ => PlanningError::InvalidStoredData,
    }
}
fn late_error(error: LateStockError) -> PlanningError {
    match error {
        LateStockError::Unavailable | LateStockError::NotFound => PlanningError::Unavailable,
        _ => PlanningError::InvalidStoredData,
    }
}

#[cfg(test)]
#[path = "../tests/support/shipment_planning.rs"]
mod tests;
