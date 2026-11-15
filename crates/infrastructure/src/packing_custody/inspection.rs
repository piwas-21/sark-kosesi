//! Bounded historical packing assertions, without shipment or payment authority.
use super::*;

#[derive(Debug, PartialEq, Eq)]
pub struct InspectionLine {
    pub sku: String,
    pub ordered: u32,
    pub active_asserted: u32,
    pub not_asserted: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InspectionEntry {
    pub id: String,
    pub action: String,
    pub target_id: Option<String>,
    pub recorded_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Inspection {
    pub lines: Vec<InspectionLine>,
    pub history: Vec<InspectionEntry>,
}

/// Inspect assertions only. Unasserted units are not proven unshipped or available.
pub async fn inspect(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order_id: &str,
) -> Result<Inspection, Error> {
    if !commerce_domain::order_stock::identifier(order_id) {
        return Err(Error::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::WarehouseManage)
        .await
        .map_err(staff)?;
    let result = async {
        let attempt = crate::order_stock::source::attempt(&mut access.tx, order_id)
            .await
            .map_err(|_| Error::Unavailable)?;
        crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
            .await
            .map_err(|_| Error::Database)?;
        let history = store::history(&mut access.tx, order_id).await?;
        // An existing assertion binds its historical received recognition. An
        // empty history has no such proof and requires current source evidence.
        let historical = history.first().map(|record| record.recognition_id.as_str());
        let source = shipment_planning::source(&mut access.tx, order_id, &attempt, historical)
            .await
            .map_err(source)?;
        let (active, _) = model::validate(&source, &history)?;
        // Validation preserves one occupancy entry per ordered line in the same order.
        let lines = source
            .order
            .quote
            .lines
            .iter()
            .zip(active)
            .map(|(line, asserted)| {
                Ok(InspectionLine {
                    sku: line.sku.clone(),
                    ordered: line.quantity,
                    active_asserted: asserted.units,
                    not_asserted: line
                        .quantity
                        .checked_sub(asserted.units)
                        .ok_or(Error::InvalidStoredData)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let history = history
            .into_iter()
            .map(|record| InspectionEntry {
                id: record.id,
                action: record.action,
                target_id: record.target_id,
                recorded_at: record.recorded_at,
            })
            .collect();
        Ok(Inspection { lines, history })
    }
    .await;
    // All outcomes after a lock wait, including errors, cross the same fence.
    access.check_time(&observed).map_err(staff)?;
    // Dropping the transaction schedules rollback; no awaited cleanup can
    // expose a stale-authority database result after the final check.
    drop(access);
    result
}
