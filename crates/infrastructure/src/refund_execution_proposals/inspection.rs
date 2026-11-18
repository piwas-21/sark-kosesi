//! Exact staff inspection of historical proposal custody, without provider access.
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub struct ProposalInspection {
    pub id: String,
    pub order_id: String,
    pub reservation_id: String,
    pub recognition_id: String,
    pub through_observation: i64,
    pub allocation: RefundAllocation,
    pub proposed_at: i64,
}

impl std::fmt::Debug for ProposalInspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProposalInspection")
            .field("id", &self.id)
            .field("order_id", &self.order_id)
            .field("reservation_id", &self.reservation_id)
            .field("recognition_id", &self.recognition_id)
            .field("through_observation", &self.through_observation)
            .field("allocation", &"[redacted]")
            .field("proposed_at", &self.proposed_at)
            .finish_non_exhaustive()
    }
}

/// Inspect one historical proposal under current staff authority. This detached
/// result does not authorize provider requests or any financial transition.
pub async fn inspect(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    id: &str,
) -> Result<Option<ProposalInspection>, ProposalError> {
    if !identifier(id) {
        return Err(ProposalError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::RefundExecutionPrepare)
        .await
        .map_err(staff)?;
    // Do not expose state-dependent outcomes until fresh authority is rechecked.
    let result = inspect_locked(&mut access.tx, id).await;
    access.check_time(&observed).map_err(staff)?;
    // Dropping the read transaction schedules rollback without another await
    // between the final authority check and returning the detached result.
    drop(access);
    result
}

async fn inspect_locked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &str,
) -> Result<Option<ProposalInspection>, ProposalError> {
    let Some(attempt) = store::attempt_for_id(tx, id).await? else {
        return Ok(None);
    };
    if !identifier(&attempt) {
        return Err(ProposalError::InvalidStoredData);
    }
    crate::order_stock::source::claim_attempt(tx, &attempt)
        .await
        .map_err(|_| ProposalError::Database)?;
    let row = store::by_id(tx, id)
        .await?
        .ok_or(ProposalError::InvalidStoredData)?;
    if row.attempt_id != attempt {
        return Err(ProposalError::InvalidStoredData);
    }
    store::validate(tx, id).await?;
    let receipt = row.receipt()?;
    Ok(Some(ProposalInspection {
        id: receipt.id,
        order_id: receipt.order_id,
        reservation_id: receipt.reservation_id,
        recognition_id: receipt.recognition_id,
        through_observation: receipt.through_observation,
        allocation: receipt.allocation,
        proposed_at: receipt.proposed_at,
    }))
}
