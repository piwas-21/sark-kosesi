//! Durable exact-prefix identity decisions, separate from financial transitions.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::identities::{IdentitySummary, summarize};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
pub(crate) mod rows;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    pub id: String,
    pub attempt_id: String,
    pub effect_key: String,
    pub revision: i64,
    pub through_observation: i64,
    pub recorded_at: i64,
    pub summary: IdentitySummary,
}
pub struct ResolutionReceipt {
    pub resolution: Resolution,
    pub replayed: bool,
}
/// Resolves all fetched observations visible under the original attempt claim.
/// A single matched identity is evidence, never payment/fulfilment authority.
pub async fn resolve(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt_id: &str,
    effect: &str,
) -> Result<ResolutionReceipt, PaymentStoreError> {
    input(attempt_id, effect)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, true).await?;
    if let Some(resolution) = rows::load(&mut tx, &frozen, started_at, Some(effect)).await? {
        tx.rollback().await?;
        return Ok(ResolutionReceipt {
            resolution,
            replayed: true,
        });
    }
    let previous = rows::load(&mut tx, &frozen, started_at, None).await?;
    let observations =
        super::observations::rows::prefix(&mut tx, &frozen, started_at, i64::MAX).await?;
    let summary = summarize(frozen.snapshot(), started_at, &observations)
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    let through_observation = observations.last().map_or(0, |v| v.revision);
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    if at < started_at
        || observations.last().is_some_and(|v| at < v.recorded_at)
        || previous.as_ref().is_some_and(|v| at < v.recorded_at)
    {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let revision = previous
        .map_or(Some(1), |v| v.revision.checked_add(1))
        .filter(|v| *v <= commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
        .ok_or(PaymentStoreError::Unavailable)?;
    let resolution = Resolution {
        id: fresh_id(ids)?,
        attempt_id: attempt_id.into(),
        effect_key: effect.into(),
        revision,
        through_observation,
        recorded_at: at,
        summary,
    };
    rows::insert(
        &mut tx,
        &resolution,
        &frozen.snapshot().binding.order_id,
        &fresh_id(ids)?,
    )
    .await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(ResolutionReceipt {
        resolution,
        replayed: false,
    })
}
/// Historical lookup; old receipts do not claim to include later observations.
pub async fn read_recorded(
    pool: &PgPool,
    attempt_id: &str,
    effect: &str,
) -> Result<Option<Resolution>, PaymentStoreError> {
    input(attempt_id, effect)?;
    read(pool, attempt_id, Some(effect)).await
}
pub async fn latest(
    pool: &PgPool,
    attempt_id: &str,
) -> Result<Option<Resolution>, PaymentStoreError> {
    if !identifier(attempt_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    read(pool, attempt_id, None).await
}
async fn read(
    pool: &PgPool,
    attempt_id: &str,
    effect: Option<&str>,
) -> Result<Option<Resolution>, PaymentStoreError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, false).await?;
    let result = rows::load(&mut tx, &frozen, started_at, effect).await?;
    tx.rollback().await?;
    Ok(result)
}
