//! Exact pending intent and recorded packing assertions, held under one attempt claim.
use super::{
    identifier,
    inspection::{self, CancellationInspection, InspectionError},
};
use crate::{
    packing_custody,
    staff_access::{SessionToken, lock_staff},
};
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use sqlx::PgPool;

#[derive(Debug, PartialEq, Eq)]
pub enum PackingEvidence {
    /// The existing bounded source and complete assertion history validated.
    Validated(packing_custody::Inspection),
    /// The required current source is not available; this is not an empty history.
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CancellationPackingEvidence {
    pub request: CancellationInspection,
    pub packing: PackingEvidence,
}

/// No result grants cancellation, shipment or provider authority.
pub async fn inspect_cancellation_packing(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order: &str,
) -> Result<Option<CancellationPackingEvidence>, InspectionError> {
    if !identifier(order) {
        return Err(InspectionError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    // Fixed capability order: cancellation.review, then warehouse.manage.
    let mut access = lock_staff(pool, token, &observed, Capability::CancellationReview)
        .await
        .map_err(inspection::staff)?;
    access
        .require_additional(Capability::WarehouseManage, &observed)
        .await
        .map_err(inspection::staff)?;
    let result = async {
        // Keep the two deep, sequential validators out of the caller's future.
        // Both retain the same transaction and attempt claim across the await.
        let Some((request, attempt)) =
            Box::pin(inspection::inspect_bound_locked(&mut access.tx, order)).await?
        else {
            return Ok(None);
        };
        let (packing, source_at) = match Box::pin(packing_custody::inspect_locked(
            &mut access.tx,
            order,
            &attempt,
        ))
        .await
        {
            Ok(value) => (
                PackingEvidence::Validated(value.inspection),
                Some(value.source_at),
            ),
            Err(packing_custody::Error::Unavailable) => (PackingEvidence::Unavailable, None),
            Err(packing_custody::Error::Database) => return Err(InspectionError::Database),
            Err(_) => return Err(InspectionError::InvalidStoredData),
        };
        Ok(Some((
            CancellationPackingEvidence { request, packing },
            source_at,
        )))
    }
    .await;
    // Fence success, absence and all source failures after every held-row wait.
    let at = access.authorized_at(&observed).map_err(inspection::staff)?;
    if result.as_ref().is_ok_and(|row| {
        row.as_ref().is_some_and(|(value, source_at)| {
            at < value.request.requested_at
                || source_at.is_some_and(|source_at| at < source_at)
                || match &value.packing {
                    PackingEvidence::Validated(packing) => packing
                        .history
                        .last()
                        .is_some_and(|entry| at < entry.recorded_at),
                    PackingEvidence::Unavailable => false,
                }
        })
    }) {
        return Err(InspectionError::ClockUnavailable);
    }
    drop(access);
    result.map(|row| row.map(|(value, _)| value))
}
