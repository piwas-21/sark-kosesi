//! Durable discovery pages, never absence, financial observations or create authority.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::VerifiedSearchPage;
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::searches::SearchSnapshot;
use sqlx::PgPool;
pub(crate) mod rows;
pub struct SearchReceipt {
    pub snapshot: SearchSnapshot,
    pub replayed: bool,
}
/// Call only after authenticated list I/O has ended. Lookup the operation receipt
/// before retrying that I/O; changed pages require a new operation effect.
pub async fn record_page(
    pool: &PgPool,
    clock: &dyn Clock,
    identifiers: &dyn Identifiers,
    effect: &str,
    previous_page_id: Option<&str>,
    verified: &VerifiedSearchPage,
) -> Result<SearchReceipt, PaymentStoreError> {
    let attempt_id = &verified.attempt().binding.id;
    input(attempt_id, effect)?;
    let intent = verified.intent(previous_page_id);
    intent
        .validate()
        .map_err(|_| PaymentStoreError::InvalidRequest)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, true).await?;
    if verified.attempt() != frozen.snapshot() {
        return Err(PaymentStoreError::Conflict);
    }
    if let Some(previous) = rows::load(&mut tx, &frozen, started_at, Some(effect), None).await? {
        if previous.intent != intent {
            return Err(PaymentStoreError::Conflict);
        }
        tx.rollback().await?;
        return Ok(SearchReceipt {
            snapshot: previous,
            replayed: true,
        });
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    let latest = rows::load(&mut tx, &frozen, started_at, None, None).await?;
    if at < started_at || latest.as_ref().is_some_and(|v| at < v.recorded_at) {
        return Err(PaymentStoreError::ClockUnavailable);
    }
    let revision = latest
        .map_or(Some(1), |v| v.revision.checked_add(1))
        .filter(|v| *v <= commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS)
        .ok_or(PaymentStoreError::Unavailable)?;
    let previous = match previous_page_id {
        Some(id) => Some(
            rows::load(&mut tx, &frozen, started_at, None, Some(id))
                .await?
                .ok_or(PaymentStoreError::NotFound)?,
        ),
        None => None,
    };
    let page_number = previous
        .as_ref()
        .map_or(Some(1), |v| v.page_number.checked_add(1))
        .ok_or(PaymentStoreError::Unavailable)?;
    let snapshot = SearchSnapshot {
        schema_version: 1,
        id: fresh_id(identifiers)?,
        attempt_id: attempt_id.clone(),
        effect_key: effect.into(),
        revision,
        page_number,
        recorded_at: at,
        intent,
    };
    snapshot
        .validate(frozen.snapshot(), started_at)
        .map_err(|_| PaymentStoreError::Unavailable)?;
    if previous.as_ref().is_some_and(|v| !snapshot.follows(v)) {
        return Err(PaymentStoreError::Conflict);
    }
    if let Some(id) = previous_page_id {
        let followed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM payment_search_pages WHERE previous_page_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if followed {
            return Err(PaymentStoreError::Conflict);
        }
    }
    rows::validate_chain(&mut tx, &frozen, started_at, &snapshot).await?;
    rows::insert(
        &mut tx,
        &snapshot,
        &frozen.snapshot().binding.order_id,
        &fresh_id(identifiers)?,
    )
    .await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(SearchReceipt {
        snapshot,
        replayed: false,
    })
}
/// Trusted receipt lookup before a list retry. No clock, entropy or provider call.
pub async fn read_recorded(
    pool: &PgPool,
    attempt_id: &str,
    effect: &str,
) -> Result<Option<SearchSnapshot>, PaymentStoreError> {
    input(attempt_id, effect)?;
    read(pool, attempt_id, Some(effect), None).await
}
/// Private consumer read. Candidate identities still require authenticated GETs.
pub async fn read_page(
    pool: &PgPool,
    attempt_id: &str,
    page_id: &str,
) -> Result<SearchSnapshot, PaymentStoreError> {
    if !identifier(attempt_id) || !identifier(page_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    read(pool, attempt_id, None, Some(page_id))
        .await?
        .ok_or(PaymentStoreError::NotFound)
}
async fn read(
    pool: &PgPool,
    attempt_id: &str,
    effect: Option<&str>,
    id: Option<&str>,
) -> Result<Option<SearchSnapshot>, PaymentStoreError> {
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started_at) = context(&mut tx, attempt_id, false).await?;
    let result = rows::load(&mut tx, &frozen, started_at, effect, id).await?;
    tx.rollback().await?;
    Ok(result)
}
