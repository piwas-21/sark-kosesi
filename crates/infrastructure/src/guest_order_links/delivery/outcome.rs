//! Redacted local callback classifications, never proof of delivery.
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
    fn parse(value: &str) -> Result<Self, LinkError> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "explicit_rejection" => Ok(Self::ExplicitRejection),
            "uncertain" => Ok(Self::Uncertain),
            _ => Err(LinkError::Unavailable),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryState {
    NotStarted,
    Unknown,
    Recorded(TransportOutcome),
}

type Inspection = (bool, bool, Option<String>, bool, bool);

async fn read(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
) -> Result<Inspection, LinkError> {
    sqlx::query_as(include_str!("outcome_inspect.sql"))
        .bind(verifier.as_slice())
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| LinkError::Unavailable)
}

async fn matches_ciphertext(
    tx: &mut Transaction<'_, Postgres>,
    permit: &opening::Permit,
) -> Result<bool, LinkError> {
    let digest: Option<Option<Vec<u8>>> = sqlx::query_scalar("SELECT CASE WHEN octet_length(ciphertext)=48 THEN sha256(ciphertext) END FROM guest_link_delivery_envelopes WHERE verifier=$1 FOR SHARE")
        .bind(permit.verifier.as_slice()).fetch_optional(&mut **tx).await.map_err(|_| LinkError::Unavailable)?;
    Ok(digest.flatten().as_deref() == Some(permit.ciphertext_sha256().as_slice()))
}

/// Inspect immutable history only; an unknown start never grants another opening.
pub async fn inspect(pool: &PgPool, verifier: &[u8; 32]) -> Result<DeliveryState, LinkError> {
    let mut tx = super::super::super::begin(pool)
        .await
        .map_err(LinkError::from)?;
    let row = read(&mut tx, verifier).await?;
    tx.rollback().await.map_err(|_| LinkError::Unavailable)?;
    match row {
        (false, false, None, false, false) => Ok(DeliveryState::NotStarted),
        (true, false, None, false, true) => Ok(DeliveryState::Unknown),
        (true, true, Some(value), true, true) => {
            Ok(DeliveryState::Recorded(TransportOutcome::parse(&value)?))
        }
        _ => Err(LinkError::Unavailable),
    }
}

/// Only the fresh in-process opener can hold this private permit.
pub(super) async fn record(
    pool: &PgPool,
    permit: &opening::Permit,
    clock: &dyn Clock,
    outcome: TransportOutcome,
) -> Result<(), LinkError> {
    let mut tx = begin_durable(pool).await?;
    if !permit.matches_start(&mut tx).await?
        || !matches_ciphertext(&mut tx, permit).await?
        || read(&mut tx, &permit.verifier).await? != (true, false, None, false, true)
    {
        return Err(LinkError::Unavailable);
    }
    let at = now(clock)?;
    let inserted = sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) VALUES($1,$2::text::xid8,$3,$4,$5)")
        .bind(permit.verifier.as_slice()).bind(permit.transaction_id()).bind(permit.ciphertext_sha256().as_slice()).bind(outcome.stored()).bind(at)
        .execute(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    if inserted.rows_affected() != 1 {
        return Err(LinkError::Unavailable);
    }
    sqlx::query("SET CONSTRAINTS require_guest_link_delivery_outcome IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| LinkError::Unavailable)?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guest_link_delivery_outcomes WHERE verifier=$1 AND start_transaction_id::text=$2 AND outcome=$3 AND observed_at=$4 AND ciphertext_sha256=$5 AND transaction_id=pg_current_xact_id())")
        .bind(permit.verifier.as_slice()).bind(permit.transaction_id()).bind(outcome.stored()).bind(at).bind(permit.ciphertext_sha256().as_slice())
        .fetch_one(&mut *tx).await.map_err(|_| LinkError::Unavailable)?;
    if !exact
        || !permit.matches_start(&mut tx).await?
        || !matches_ciphertext(&mut tx, permit).await?
        || read(&mut tx, &permit.verifier).await?
            != (true, true, Some(outcome.stored().to_owned()), true, true)
    {
        return Err(LinkError::Unavailable);
    }
    durable(&mut tx).await?;
    now(clock)?;
    tx.commit().await.map_err(|_| LinkError::Unavailable)
}
