//! Private callback work budgets and custody headers, never financial authority.
use super::{PaymentError, hex, opaque};
use crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS;
use serde::{Deserialize, Serialize};

pub const LEASE_SECONDS: i64 = 60;
pub const MAX_BATCH: u16 = 25;
pub const MAX_GENERATIONS: i64 = 5;
pub const GLOBAL_CAPACITY: i64 = 200;
pub const PROFILE_CAPACITY: i64 = 50;
pub const GLOBAL_PER_MINUTE: i64 = 120;
pub const PROFILE_PER_MINUTE: i64 = 30;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeSnapshot {
    pub schema_version: u16,
    pub id: String,
    pub profile_id: String,
    pub provider_id: String,
    pub received_at: i64,
}
impl IntakeSnapshot {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.schema_version != 1
            || !hex(&self.id, 32)
            || !opaque(&self.profile_id, "pfl_")
            || !opaque(&self.provider_id, "tr_")
            || !(0..=MAX_QUOTE_UNIX_SECONDS).contains(&self.received_at)
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Pending,
    Leased,
    Done,
    NeedsReview,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    RetryExhausted,
    CustodyInvalid,
    Unmatched,
    GenerationLimit,
    PaidObserved,
    Ambiguous,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackJob {
    pub intake_id: String,
    pub source_event_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub due_at: i64,
    pub generation: i64,
    pub lease_until: Option<i64>,
    pub state: JobState,
    pub review_reason: Option<ReviewReason>,
    /// A terminal job releases capacity only after every generation is resolved.
    pub settled: bool,
}
impl CallbackJob {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if !hex(&self.intake_id, 32)
            || !hex(&self.source_event_id, 32)
            || !(0..=MAX_QUOTE_UNIX_SECONDS).contains(&self.created_at)
            || !(self.created_at..=MAX_QUOTE_UNIX_SECONDS).contains(&self.updated_at)
            || !(self.created_at..=MAX_QUOTE_UNIX_SECONDS).contains(&self.due_at)
            || !(0..=MAX_GENERATIONS).contains(&self.generation)
            || (self.state == JobState::Leased) != self.lease_until.is_some()
            || self.lease_until.is_some_and(|until| {
                self.generation == 0
                    || until <= self.updated_at
                    || until > MAX_QUOTE_UNIX_SECONDS
                    || self.updated_at.checked_add(LEASE_SECONDS) != Some(until)
            })
            || (self.state == JobState::NeedsReview) != self.review_reason.is_some()
            || (self.state == JobState::Pending && self.due_at < self.updated_at)
            || (self.state == JobState::Pending && self.generation == MAX_GENERATIONS)
            || (self.generation == 0
                && (self.state != JobState::Pending
                    || self.updated_at != self.created_at
                    || self.due_at != self.created_at))
            || (self.settled && self.state != JobState::Done)
            || (matches!(self.state, JobState::Done | JobState::NeedsReview)
                && self.due_at != self.updated_at)
            || (matches!(
                self.review_reason,
                Some(ReviewReason::RetryExhausted | ReviewReason::GenerationLimit)
            ) && self.generation != MAX_GENERATIONS)
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}

/// Fixed bounded retry schedule. A fifth generation must stop, not schedule I/O.
pub fn retry_delay(generation: i64) -> Result<i64, PaymentError> {
    if !(1..MAX_GENERATIONS).contains(&generation) {
        return Err(PaymentError::InvalidRequest);
    }
    let shift = u32::try_from(generation - 1).map_err(|_| PaymentError::InvalidRequest)?;
    LEASE_SECONDS
        .checked_shl(shift)
        .ok_or(PaymentError::InvalidRequest)
}
