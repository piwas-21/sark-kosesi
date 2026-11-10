//! Bounded local callback classifications. These do not establish mailbox delivery.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportOutcome {
    Accepted,
    ExplicitRejection,
    Uncertain,
}
impl TransportOutcome {
    fn stored(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::ExplicitRejection => "explicit_rejection",
            Self::Uncertain => "uncertain",
        }
    }
    fn parse(value: &str) -> Result<Self, VerificationError> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "explicit_rejection" => Ok(Self::ExplicitRejection),
            "uncertain" => Ok(Self::Uncertain),
            _ => Err(VerificationError::Unavailable),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryState {
    NotStarted,
    Unknown,
    Recorded(TransportOutcome),
}

/// Read local custody only. Unknown is not authority to open again or resend.
pub async fn inspect(
    pool: &PgPool,
    verifier: &[u8; 32],
) -> Result<DeliveryState, VerificationError> {
    let mut tx = begin(pool).await?;
    let row: Option<(Option<String>, bool)> = sqlx::query_as("SELECT left(o.outcome,20),COALESCE(o.start_transaction_id=s.transaction_id AND o.transaction_id<>s.transaction_id AND o.observed_at>=s.started_at AND o.observed_at<=9007199254740991,false) FROM email_verification_delivery_starts s LEFT JOIN email_verification_delivery_outcomes o USING(verifier) WHERE s.verifier=$1")
        .bind(verifier.as_slice()).fetch_optional(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    match row {
        None => Ok(DeliveryState::NotStarted),
        Some((None, false)) => Ok(DeliveryState::Unknown),
        Some((Some(value), true)) => Ok(DeliveryState::Recorded(TransportOutcome::parse(&value)?)),
        _ => Err(VerificationError::Unavailable),
    }
}

/// Only the private fresh-opening permit can enter this function.
pub(super) async fn record(
    pool: &PgPool,
    permit: &start::Permit,
    key: &VerificationDeliveryKey,
    clock: &ObservedClock<&dyn Clock>,
    outcome: TransportOutcome,
) -> Result<(), VerificationError> {
    let mut tx = begin_durable(pool).await?;
    if !permit.matches_start(&mut tx, key).await? {
        return Err(VerificationError::Unavailable);
    }
    let at = bounded_now(clock)?;
    let inserted = sqlx::query("INSERT INTO email_verification_delivery_outcomes(verifier,start_transaction_id,outcome,observed_at) VALUES($1,$2::text::xid8,$3,$4)")
        .bind(permit.verifier.as_slice()).bind(permit.transaction_id()).bind(outcome.stored()).bind(at)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(VerificationError::Unavailable);
    }
    sqlx::query("SET CONSTRAINTS require_email_verification_delivery_outcome IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_verification_delivery_outcomes WHERE verifier=$1 AND start_transaction_id::text=$2 AND outcome=$3 AND observed_at=$4 AND transaction_id=pg_current_xact_id())")
        .bind(permit.verifier.as_slice()).bind(permit.transaction_id()).bind(outcome.stored()).bind(at)
        .fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if !exact || !permit.matches_start(&mut tx, key).await? {
        return Err(VerificationError::Unavailable);
    }
    require_durable(&mut tx).await?;
    bounded_now(clock)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)
}
