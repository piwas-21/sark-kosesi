//! Gross historical payment recognition; no stock or fulfilment authority.
use super::{identities::IdentitySummary, *};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Unconfirmed,
    Received,
    NeedsReview,
}
impl State {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unconfirmed => "unconfirmed",
            Self::Received => "received",
            Self::NeedsReview => "needs_review",
        }
    }
}

/// Derive state only from a closed summary of a complete authenticated prefix.
pub fn derive(summary: &IdentitySummary) -> Result<State, PaymentError> {
    if summary.provider_ids.len() > identities::MAX_IDENTITY_OBSERVATIONS
        || summary.paid_provider_ids.len() > summary.provider_ids.len()
        || !summary.provider_ids.iter().all(|id| opaque(id, "tr_"))
        || !summary.provider_ids.windows(2).all(|ids| ids[0] < ids[1])
        || !summary
            .paid_provider_ids
            .windows(2)
            .all(|ids| ids[0] < ids[1])
        || !summary
            .paid_provider_ids
            .iter()
            .all(|id| summary.provider_ids.binary_search(id).is_ok())
    {
        return Err(PaymentError::InvalidSnapshot);
    }
    Ok(if summary.provider_ids.len() > 1 {
        State::NeedsReview
    } else if !summary.paid_provider_ids.is_empty() {
        State::Received
    } else {
        State::Unconfirmed
    })
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recognition {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub resolution_id: String,
    pub through_observation: i64,
    pub recorded_at: i64,
    pub state: State,
    pub currency: Currency,
    pub amount_minor: i64,
}
impl Recognition {
    pub fn validate(
        &self,
        attempt: &AttemptSnapshot,
        summary: &IdentitySummary,
        through_observation: i64,
        resolution_recorded_at: i64,
    ) -> Result<(), PaymentError> {
        attempt.validate()?;
        if !hex(&self.id, 32)
            || !hex(&self.resolution_id, 32)
            || self.order_id != attempt.binding.order_id
            || self.attempt_id != attempt.binding.id
            || !(0..=identities::MAX_IDENTITY_OBSERVATIONS as i64)
                .contains(&self.through_observation)
            || self.through_observation != through_observation
            || self.recorded_at < resolution_recorded_at
            || resolution_recorded_at < attempt.binding.created_at
            || self.recorded_at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS
            || self.state != derive(summary)?
            || self.currency != attempt.binding.currency
            || self.amount_minor != attempt.binding.amount_minor
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}
