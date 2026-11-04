//! Explicit, bounded staff decisions for retained scarcity cases.
use super::*;
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::staff_access::Capability;
mod store;

pub struct RetryRequest<'a> {
    pub case_id: &'a str,
    pub case_digest: &'a str,
    pub effect_key: &'a str,
    pub expected_generation: i32,
    pub reason: &'a str,
    pub confirmed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum DecisionOutcome {
    Allocated { allocation_id: String },
    StillScarce,
}
#[derive(PartialEq, Eq)]
pub struct DecisionReceipt {
    pub id: String,
    pub case_id: String,
    pub case_digest: String,
    pub order_id: String,
    pub attempt_id: String,
    pub recognition_id: String,
    pub actor_id: String,
    pub effect_key: String,
    pub generation: i32,
    pub reason: String,
    pub recorded_at: i64,
    pub outcome: DecisionOutcome,
    pub replayed: bool,
}
impl std::fmt::Debug for DecisionReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecisionReceipt")
            .field("id", &self.id)
            .field("case_id", &self.case_id)
            .field("generation", &self.generation)
            .field("reason", &"[redacted]")
            .field("outcome", &self.outcome)
            .field("replayed", &self.replayed)
            .finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub enum DispositionError {
    InvalidRequest,
    Staff(StaffError),
    Custody(LateStockError),
    Conflict,
}
impl From<LateStockError> for DispositionError {
    fn from(error: LateStockError) -> Self {
        Self::Custody(error)
    }
}
impl From<sqlx::Error> for DispositionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Custody(error.into())
    }
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_request(request: &RetryRequest<'_>) -> Result<(), DispositionError> {
    if !identifier(request.case_id)
        || !hex(request.case_digest, 64)
        || !hex(request.effect_key, 32)
        || !(1..=3).contains(&request.expected_generation)
        || request.reason.is_empty()
        || request.reason.len() > 240
        || request.reason.contains('\0')
        || request.reason.trim() != request.reason
        || !request.confirmed
    {
        return Err(DispositionError::InvalidRequest);
    }
    Ok(())
}
fn require_stored_ids(ids: &[&str]) -> Result<(), LateStockError> {
    if !ids.iter().all(|id| identifier(id)) {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
fn replay_in_history(
    prior: &DecisionReceipt,
    chain: &[DecisionReceipt],
) -> Result<(), LateStockError> {
    if !chain.iter().any(|decision| decision == prior) {
        return Err(LateStockError::InvalidStoredData);
    }
    Ok(())
}
fn validate_replay_intent(
    prior: &DecisionReceipt,
    request: &RetryRequest<'_>,
    actor: &str,
) -> Result<(), DispositionError> {
    if prior.case_id != request.case_id
        || prior.case_digest != request.case_digest
        || prior.generation != request.expected_generation
        || prior.reason != request.reason
        || prior.actor_id != actor
    {
        return Err(DispositionError::Conflict);
    }
    Ok(())
}
fn validate_case_binding(
    case: &ReviewCase,
    request_case: &str,
    attempt: &str,
) -> Result<(), DispositionError> {
    if case.id != request_case
        || case.attempt_id != attempt
        || case.reason != ReviewReason::ComponentScarcity
    {
        return Err(DispositionError::Conflict);
    }
    Ok(())
}
fn validate_next_generation(
    prior: &[DecisionReceipt],
    generation: i32,
) -> Result<(), DispositionError> {
    if prior
        .iter()
        .any(|item| matches!(item.outcome, DecisionOutcome::Allocated { .. }))
        || i32::try_from(prior.len())
            .ok()
            .and_then(|n| n.checked_add(1))
            != Some(generation)
    {
        return Err(DispositionError::Conflict);
    }
    Ok(())
}
fn require_open_review(outcome: Option<Outcome>, case_id: &str) -> Result<(), DispositionError> {
    match outcome {
        Some(Outcome::NeedsReview(retained)) if retained.id == case_id => Ok(()),
        Some(Outcome::Late(_)) => Err(DispositionError::Conflict),
        _ => Err(LateStockError::InvalidStoredData.into()),
    }
}
pub async fn retry(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    request: RetryRequest<'_>,
) -> Result<DecisionReceipt, DispositionError> {
    validate_request(&request)?;
    let mut access = lock_staff(pool, token, clock, Capability::StockRecover)
        .await
        .map_err(DispositionError::Staff)?;
    // A global effect-key lock serializes cross-case retries as well as same-case requests.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("late-stock-disposition:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior_attempt: Option<String> = sqlx::query_scalar(
        "SELECT left(attempt_id,33) FROM late_stock_dispositions WHERE effect_key=$1",
    )
    .bind(request.effect_key)
    .fetch_optional(&mut *access.tx)
    .await?;
    if let Some(attempt) = prior_attempt {
        require_stored_ids(&[&attempt])?;
        crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
            .await
            .map_err(LateStockError::from)?;
        let mut prior = store::by_key(&mut access.tx, request.effect_key)
            .await?
            .ok_or(LateStockError::InvalidStoredData)?;
        if prior.attempt_id != attempt {
            return Err(LateStockError::InvalidStoredData.into());
        }
        let chain = store::validate_history(&mut access.tx, &prior.case_id).await?;
        replay_in_history(&prior, &chain)?;
        if !matches!(
            rows::read(&mut access.tx, &prior.order_id).await?,
            Some(Outcome::NeedsReview(_) | Outcome::Late(_))
        ) {
            return Err(LateStockError::InvalidStoredData.into());
        }
        validate_replay_intent(&prior, &request, &access.actor)?;
        access.check_time(clock).map_err(DispositionError::Staff)?;
        prior.replayed = true;
        access.tx.commit().await?;
        return Ok(prior);
    }
    let (order, attempt): (String, String) = sqlx::query_as(
        "SELECT left(order_id,33),left(attempt_id,33) FROM late_stock_review_cases WHERE id=$1",
    )
    .bind(request.case_id)
    .fetch_optional(&mut *access.tx)
    .await?
    .ok_or(DispositionError::Custody(LateStockError::NotFound))?;
    require_stored_ids(&[&order, &attempt])?;
    crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(LateStockError::from)?;
    let case = rows::review_case(&mut access.tx, &order)
        .await?
        .ok_or(LateStockError::InvalidStoredData)?;
    validate_case_binding(&case, request.case_id, &attempt)?;
    require_open_review(rows::read(&mut access.tx, &order).await?, &case.id)?;
    history::review(&mut access.tx, &case).await?;
    let digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(s) FROM late_stock_review_cases s WHERE s.id=$1",
    )
    .bind(&case.id)
    .fetch_one(&mut *access.tx)
    .await?;
    if digest != request.case_digest {
        return Err(DispositionError::Conflict);
    }
    let prior = store::validate_history(&mut access.tx, &case.id).await?;
    validate_next_generation(&prior, request.expected_generation)?;
    let mut receipt = DecisionReceipt {
        id: fresh(ids)?,
        case_id: case.id,
        case_digest: digest,
        order_id: order,
        attempt_id: attempt,
        recognition_id: String::new(),
        actor_id: access.actor.clone(),
        effect_key: request.effect_key.into(),
        generation: request.expected_generation,
        reason: request.reason.into(),
        recorded_at: 0,
        outcome: DecisionOutcome::StillScarce,
        replayed: false,
    };
    retry_engine::execute(&mut access.tx, clock, ids, &mut receipt).await?;
    access.check_time(clock).map_err(DispositionError::Staff)?;
    access.tx.commit().await?;
    Ok(receipt)
}
pub(super) use store::insert;
pub(super) use store::validate_history;

#[cfg(test)]
#[path = "../../../tests/support/late_stock_disposition_request_unit.rs"]
mod tests;
