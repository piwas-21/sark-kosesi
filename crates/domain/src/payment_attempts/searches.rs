//! Minimal discovery history. End pages never establish payment absence.
use super::*;
use crate::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, media::valid_command_id};
pub const MAX_SEARCH_PAGES: i64 = 100;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchIntent {
    pub previous_page_id: Option<String>,
    pub from: Option<String>,
    pub next: Option<String>,
    pub candidates: Vec<String>,
    pub credential_fingerprint: String,
}
impl SearchIntent {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.previous_page_id.as_ref().is_some_and(|v| !hex(v, 32))
            || self.previous_page_id.is_some() != self.from.is_some()
            || !hex(&self.credential_fingerprint, 64)
            || [&self.from, &self.next]
                .iter()
                .any(|v| v.as_ref().is_some_and(|v| !opaque(v, "tr_")))
            || (self.from.is_some() && self.from == self.next)
            || self.candidates.len() > 10
            || self.candidates.iter().any(|v| !opaque(v, "tr_"))
            || self
                .candidates
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.candidates.len()
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSnapshot {
    pub schema_version: u16,
    pub id: String,
    pub attempt_id: String,
    pub effect_key: String,
    pub revision: i64,
    pub page_number: i64,
    pub recorded_at: i64,
    pub intent: SearchIntent,
}
impl SearchSnapshot {
    pub fn validate(&self, attempt: &AttemptSnapshot, started_at: i64) -> Result<(), PaymentError> {
        DispatchState::Started { started_at }.validate(attempt)?;
        self.intent.validate()?;
        if self.schema_version != 1
            || !hex(&self.id, 32)
            || self.attempt_id != attempt.binding.id
            || !valid_command_id(&self.effect_key)
            || !(1..=MAX_QUOTE_UNIX_SECONDS).contains(&self.revision)
            || !(1..=MAX_SEARCH_PAGES).contains(&self.page_number)
            || (self.page_number == 1) != self.intent.previous_page_id.is_none()
            || self.recorded_at < started_at
            || self.recorded_at > MAX_QUOTE_UNIX_SECONDS
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
    /// Exact continuation only; terminal pages cannot be resumed or declared absent.
    pub fn follows(&self, previous: &Self) -> bool {
        self.attempt_id == previous.attempt_id
            && self.intent.previous_page_id.as_ref() == Some(&previous.id)
            && previous.intent.next.is_some()
            && self.intent.from == previous.intent.next
            && previous.page_number.checked_add(1) == Some(self.page_number)
            && self.revision > previous.revision
            && self.recorded_at >= previous.recorded_at
    }
}
