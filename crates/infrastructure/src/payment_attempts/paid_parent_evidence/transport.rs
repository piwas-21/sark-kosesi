use super::*;
use crate::mollie::{MollieError, VerifiedReversalTotals};

/// This capability is deliberately neither cloneable nor serializable.
pub struct Permit {
    pub(super) operation: Operation,
    pub(super) attempt: AttemptSnapshot,
}
/// This value can only be produced by consuming a permit and completing its GET.
pub struct Fetched {
    pub(super) operation: Operation,
    pub(super) verified: VerifiedReversalTotals,
    pub(super) fetched_at: i64,
}
/// Consume the permit before I/O. Losing it or an uncertain GET never grants a retry.
pub async fn fetch(
    permit: Permit,
    clock: &dyn Clock,
    client: &VerifiedReconciliationClient,
) -> Result<Fetched, MollieError> {
    let at = now(clock).map_err(MollieError::Custody)?;
    if at < permit.operation.started_at {
        return Err(MollieError::Custody(PaymentStoreError::ClockUnavailable));
    }
    if at >= permit.operation.fetch_before {
        return Err(MollieError::Custody(PaymentStoreError::Unavailable));
    }
    if (client.profile_id(), client.credential_fingerprint())
        != (
            permit.operation.profile_id.as_str(),
            permit.operation.credential_fingerprint.as_str(),
        )
    {
        return Err(MollieError::Custody(PaymentStoreError::Conflict));
    }
    let verified = client
        .fetch_reversal_totals(&permit.attempt, &permit.operation.payment_id)
        .await?;
    let fetched_at = now(clock).map_err(MollieError::Custody)?;
    if fetched_at < at {
        return Err(MollieError::Custody(PaymentStoreError::ClockUnavailable));
    }
    Ok(Fetched {
        operation: permit.operation,
        verified,
        fetched_at,
    })
}
