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
use commerce_domain::{BillOfMaterials, checkout_orders::OrderSnapshot, order_stock::Member};
use commerce_domain::{
    order_shipment_components::{ExpectedComponent, expected_components},
    order_shipments::SkuUnits,
};
use sqlx::{PgPool, Postgres, Transaction};

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
    let result = async {
        let attempt = order_stock::source::attempt(&mut access.tx, order_id)
            .await
            .map_err(settlement_error)?;
        order_stock::source::claim_attempt(&mut access.tx, &attempt)
            .await
            .map_err(settlement_error)?;
        let source = source(&mut access.tx, order_id, &attempt, None).await?;
        let (order, members, boms) = (source.order, source.members, source.boms);
        let prior: Vec<_> = order
            .quote
            .lines
            .iter()
            .map(|line| SkuUnits {
                sku: line.sku.clone(),
                units: 0,
            })
            .collect();
        let components =
            expected_components(&order, &prior, proposed, &members, &boms).map_err(|error| {
                match error {
                commerce_domain::order_shipment_components::ShipmentComponentError::Quantities(
                    commerce_domain::order_shipments::ShipmentQuantityError::InvalidQuantities,
                ) => PlanningError::InvalidRequest,
                _ => PlanningError::InvalidStoredData,
            }
            })?;
        Ok(Projection { components })
    }
    .await;
    access.check_time(&observed).map_err(PlanningError::Staff)?;
    access
        .tx
        .rollback()
        .await
        .map_err(PlanningError::Database)?;
    result
}

/// Revalidate allocation and order while the caller holds the attempt claim.
pub(crate) struct Source {
    pub order: OrderSnapshot,
    pub members: Vec<Member>,
    pub boms: Vec<BillOfMaterials>,
    pub recognition_id: String,
    pub recognition_at: i64,
    pub allocation_kind: &'static str,
    pub allocation_id: String,
    pub allocation_at: i64,
}
pub(crate) async fn source(
    tx: &mut Transaction<'_, Postgres>,
    order_id: &str,
    attempt: &str,
    historical: Option<&str>,
) -> Result<Source, PlanningError> {
    let (frozen, recognition) = if let Some(id) = historical {
        let (frozen, recognition) = recognition::historical_in_transaction(&mut *tx, attempt, id)
            .await
            .map_err(payment_error)?;
        (frozen, Some(recognition))
    } else {
        recognition::current_in_transaction(&mut *tx, attempt)
            .await
            .map_err(payment_error)?
    };
    let recognition = recognition
        .filter(|r| r.state == recognition::State::Received)
        .ok_or(PlanningError::Unavailable)?;
    let order = crate::checkout_orders::snapshot_in_transaction(
        &mut *tx,
        &frozen.snapshot().binding.basket_id,
        order_id,
    )
    .await
    .map_err(|_| PlanningError::InvalidStoredData)?;
    let outcome = late_stock::read_in_transaction(&mut *tx, order_id)
        .await
        .map_err(late_error)?;
    // The allocation reader revalidates its exact receipt/attempt/order
    // binding; payment_attempts has a unique attempt for each order.
    let (members, allocation_kind, allocation_id, allocation_at) = match outcome {
        Some(Outcome::Original(s)) => (s.footprint.members, "original", s.id, s.settled_at),
        Some(Outcome::Late(s)) => (
            s.footprint
                .original
                .into_iter()
                .map(|member| member.member)
                .collect(),
            "late",
            s.id,
            s.allocated_at,
        ),
        Some(Outcome::NeedsReview(_)) | None => return Err(PlanningError::Unavailable),
    };
    let mut boms = Vec::with_capacity(order.quote.lines.len());
    for line in &order.quote.lines {
        // Bound the existing loader before it allocates its result.
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM catalogue_bom_lines WHERE sku=$1 AND revision=$2 LIMIT 65) bounded")
        .bind(&line.sku).bind(line.bom_revision).fetch_one(&mut **tx).await
        .map_err(PlanningError::Database)?;
        ensure_bom_count(count)?;
        let bom = crate::bom::load(&mut **tx, &line.sku, line.bom_revision)
            .await
            .map_err(PlanningError::Database)?
            .ok_or(PlanningError::InvalidStoredData)?;
        boms.push(bom);
    }
    Ok(Source {
        order,
        members,
        boms,
        recognition_at: recognition.recorded_at,
        recognition_id: recognition.id,
        allocation_kind,
        allocation_id,
        allocation_at,
    })
}

fn ensure_bom_count(count: i64) -> Result<(), PlanningError> {
    if !(1..=64).contains(&count) {
        return Err(PlanningError::InvalidStoredData);
    }
    Ok(())
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
