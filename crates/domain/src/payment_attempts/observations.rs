//! Immutable minimal fetched facts, never a payment/order/fulfilment transition.
use super::*;
use crate::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, media::valid_command_id};
pub const MAX_OBSERVATION_REVISION: i64 = 9_007_199_254_740_991;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationIntent {
    pub payment: ProviderPayment,
    pub credential_fingerprint: String,
}
impl ObservationIntent {
    pub fn validate(&self, attempt: &AttemptSnapshot) -> Result<(), PaymentError> {
        if !hex(&self.credential_fingerprint, 64) {
            return Err(PaymentError::InvalidSnapshot);
        }
        match_provider(attempt, &self.payment)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationSnapshot {
    pub schema_version: u16,
    pub id: String,
    pub attempt_id: String,
    pub effect_key: String,
    pub revision: i64,
    /// Server-observed operation time; neither provider paidAt nor commit time.
    pub recorded_at: i64,
    pub intent: ObservationIntent,
}
impl ObservationSnapshot {
    pub fn validate(&self, attempt: &AttemptSnapshot, started_at: i64) -> Result<(), PaymentError> {
        DispatchState::Started { started_at }.validate(attempt)?;
        if self.schema_version != 1
            || !hex(&self.id, 32)
            || self.attempt_id != attempt.binding.id
            || !valid_command_id(&self.effect_key)
            || !(1..=MAX_OBSERVATION_REVISION).contains(&self.revision)
            || self.recorded_at < started_at
            || self.recorded_at > MAX_QUOTE_UNIX_SECONDS
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        self.intent.validate(attempt)
    }
}
