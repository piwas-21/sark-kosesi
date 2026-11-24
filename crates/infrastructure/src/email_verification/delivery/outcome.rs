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
    /// Historical callback classification recorded before authenticated binding.
    LegacyRecorded(TransportOutcome),
    Recorded(TransportOutcome),
}

/// Read local custody only. Unknown is not authority to open again or resend.
pub async fn inspect(
    pool: &PgPool,
    verifier: &[u8; 32],
) -> Result<DeliveryState, VerificationError> {
    let mut tx = begin(pool).await?;
    let row: (bool, bool, Option<String>, Option<i16>) =
        sqlx::query_as(include_str!("outcome_inspect.sql"))
            .bind(verifier.as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| VerificationError::Unavailable)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    match row {
        (false, false, None, None) => Ok(DeliveryState::NotStarted),
        (true, false, None, None) => Ok(DeliveryState::Unknown),
        (true, true, Some(value), Some(0)) => Ok(DeliveryState::LegacyRecorded(
            TransportOutcome::parse(&value)?,
        )),
        (true, true, Some(value), Some(1)) => {
            Ok(DeliveryState::Recorded(TransportOutcome::parse(&value)?))
        }
        _ => Err(VerificationError::Unavailable),
    }
}

/// Only the private fresh-opening permit can enter this function.
pub(super) async fn record(
    pool: &PgPool,
    permit: &opening::OutcomePermit,
    key: &VerificationDeliveryKey,
    clock: &ObservedClock<&dyn Clock>,
    outcome: TransportOutcome,
) -> Result<(), VerificationError> {
    let mut tx = begin_durable(pool).await?;
    require_custody(&mut tx, permit, key).await?;
    let at = bounded_now(clock)?;
    let verifier = permit.start().verifier.as_slice();
    let start_xid = permit.start().transaction_id();
    let digest = permit.ciphertext_sha256().as_slice();
    let inserted = sqlx::query("INSERT INTO email_verification_delivery_outcomes(verifier,start_transaction_id,outcome,observed_at,evidence_version,ciphertext_sha256) VALUES($1,$2::text::xid8,$3,$4,1,$5)")
        .bind(verifier).bind(start_xid).bind(outcome.stored()).bind(at).bind(digest)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(VerificationError::Unavailable);
    }
    sqlx::query("SET CONSTRAINTS require_email_verification_delivery_outcome IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_verification_delivery_outcomes WHERE verifier=$1 AND start_transaction_id::text=$2 AND outcome=$3 AND observed_at=$4 AND evidence_version=1 AND ciphertext_sha256=$5 AND transaction_id=pg_current_xact_id())")
        .bind(verifier).bind(start_xid).bind(outcome.stored()).bind(at).bind(digest)
        .fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if !exact {
        return Err(VerificationError::Unavailable);
    }
    require_custody(&mut tx, permit, key).await?;
    require_durable(&mut tx).await?;
    bounded_now(clock)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)
}

async fn require_custody(
    tx: &mut Transaction<'_, Postgres>,
    permit: &opening::OutcomePermit,
    key: &VerificationDeliveryKey,
) -> Result<(), VerificationError> {
    if !permit.start().matches_start(tx, key).await? {
        return Err(VerificationError::Unavailable);
    }
    let matches: Option<bool> = sqlx::query_scalar(
        "SELECT CASE WHEN e.envelope_version=1 AND octet_length(e.ciphertext)=48
          THEN sha256(e.ciphertext)=$2 ELSE false END
         FROM email_verification_delivery_envelopes e
         JOIN email_verification_delivery_starts s
          ON s.verifier=e.verifier AND s.key_id=e.key_id AND s.nonce_counter=e.nonce_counter
         WHERE e.verifier=$1 FOR SHARE OF e",
    )
    .bind(permit.start().verifier.as_slice())
    .bind(permit.ciphertext_sha256().as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    if matches != Some(true) {
        return Err(VerificationError::Unavailable);
    }
    Ok(())
}
