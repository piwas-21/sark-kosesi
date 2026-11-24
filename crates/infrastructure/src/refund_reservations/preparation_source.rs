//! Current received recognition for constructing explicit refund preparation intent.
use super::{RefundError, identifier, lock_staff, source, staff, store};
use crate::staff_access::SessionToken;
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationPhase {
    Reservation,
    Proposal,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PreparationSource {
    pub order_id: String,
    pub recognition_id: String,
    pub through_observation: Option<i64>,
}

/// Read the source binding under the same grant required to prepare that phase.
/// The detached result may become stale and grants no subsequent authority.
pub async fn inspect_preparation_source(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order_id: &str,
    phase: PreparationPhase,
) -> Result<Option<PreparationSource>, RefundError> {
    if !identifier(order_id) {
        return Err(RefundError::InvalidRequest);
    }
    let capability = match phase {
        PreparationPhase::Reservation => Capability::RefundPrepare,
        PreparationPhase::Proposal => Capability::RefundExecutionPrepare,
    };
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, capability)
        .await
        .map_err(staff)?;
    let result = async {
        let attempt = store::claim_attempt_for_order(&mut access.tx, order_id).await?;
        // The locked row binds the requested order. Source restoration checks
        // the frozen attempt and recognition against that same row and order.
        Box::pin(source(&mut access.tx, &attempt, None))
            .await
            .map(|(_, received)| received)
    }
    .await;
    let at = access.authorized_at(&observed).map_err(staff)?;
    drop(access);
    match result {
        Ok(received) if at >= received.recorded_at => Ok(Some(PreparationSource {
            order_id: received.order_id,
            recognition_id: received.id,
            through_observation: (phase == PreparationPhase::Proposal)
                .then_some(received.through_observation),
        })),
        Ok(_) => Err(RefundError::ClockUnavailable),
        Err(RefundError::Unavailable) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "../../tests/support/refund_preparation_source.rs"]
mod tests;
