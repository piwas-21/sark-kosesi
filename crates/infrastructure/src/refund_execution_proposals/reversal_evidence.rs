//! One detached refund-review snapshot of the original paid identity's local custody.
use super::*;
use crate::{
    order_stock::{SettlementError, source::claim_order_attempt},
    payment_attempts::{PaymentStoreError, paid_parent_evidence, recognition, reversals},
};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};

#[derive(Debug, PartialEq, Eq)]
pub struct ReversalEvidence {
    pub order_id: String,
    pub recognition_id: String,
    pub through_observation: i64,
    pub parent_operations: usize,
    pub parent_pending: usize,
    pub parent_complete: usize,
    pub child_observations: usize,
    pub digest: String,
}

/// Read current received recognition and both bounded evidence sets under the same
/// attempt claim. This result conveys local custody only and grants no authority.
pub async fn inspect_reversal_evidence(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order_id: &str,
) -> Result<Option<ReversalEvidence>, ProposalError> {
    if !identifier(order_id) {
        return Err(ProposalError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::RefundExecutionPrepare)
        .await
        .map_err(staff)?;
    let result = inspect_claimed(&mut access.tx, order_id).await;
    let at = access.authorized_at(&observed).map_err(staff)?;
    // Keep authority held through the final freshness check; drop rolls back the read.
    drop(access);
    match result {
        Ok((evidence, recorded_at)) if at >= recorded_at => Ok(Some(evidence)),
        Ok(_) => Err(ProposalError::ClockUnavailable),
        Err(ProposalError::Unavailable) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn inspect_claimed(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
) -> Result<(ReversalEvidence, i64), ProposalError> {
    let attempt = claim_order_attempt(tx, order)
        .await
        .map_err(|error| match error {
            SettlementError::NotFound => ProposalError::Unavailable,
            SettlementError::Database => ProposalError::Database,
            _ => ProposalError::InvalidStoredData,
        })?;
    // The locked row binds the order. Recognition restores that exact attempt and
    // proves its current prefix, including the frozen order and identity claims.
    let (frozen, current) = recognition::current_in_transaction(tx, &attempt)
        .await
        .map_err(payment_error)?;
    let current = current.ok_or(ProposalError::Unavailable)?;
    let parent_id = paid_parent_evidence::paid_identity(tx, &frozen, &current)
        .await
        .map_err(payment_error)?;
    let (parent, parent_recorded_at) =
        paid_parent_evidence::inspect_claimed(tx, &current, &parent_id)
            .await
            .map_err(payment_error)?;
    let (child, child_recorded_at) = reversals::inspect_claimed(tx, &frozen, &parent_id)
        .await
        .map_err(payment_error)?;
    let mut digest = Sha256::new();
    digest.update(b"piwas:refund-reversal-evidence:v1\0");
    digest.update(order.as_bytes());
    digest.update(parent.digest.as_bytes());
    digest.update(child.digest.as_bytes());
    Ok((
        ReversalEvidence {
            order_id: current.order_id,
            recognition_id: current.id,
            through_observation: current.through_observation,
            parent_operations: parent.operations,
            parent_pending: parent.pending,
            parent_complete: parent.complete,
            child_observations: child.observations,
            digest: format!("{:x}", digest.finalize()),
        },
        parent_recorded_at.max(child_recorded_at),
    ))
}

fn payment_error(error: PaymentStoreError) -> ProposalError {
    match error {
        PaymentStoreError::Unavailable => ProposalError::Unavailable,
        PaymentStoreError::Conflict => ProposalError::Conflict,
        PaymentStoreError::Database
        | PaymentStoreError::Boundary(crate::checkout_orders::OrderStoreError::Database) => {
            ProposalError::Database
        }
        _ => ProposalError::InvalidStoredData,
    }
}
