//! Identity evidence only: no current status, payment, stock or fulfilment authority.
use super::{observations::ObservationSnapshot, *};
use std::collections::BTreeSet;
pub const MAX_IDENTITY_OBSERVATIONS: usize = 1000;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySummary {
    /// Sorted distinct identities; more than one is an explicit finance exception.
    pub provider_ids: Vec<String>,
    /// Sticky historical paid evidence, including for ambiguous identities.
    pub paid_provider_ids: Vec<String>,
}
/// Validate the complete contiguous prefix. A bounded stop never proves absence.
pub fn summarize(
    attempt: &AttemptSnapshot,
    started_at: i64,
    observations: &[ObservationSnapshot],
) -> Result<IdentitySummary, PaymentError> {
    DispatchState::Started { started_at }.validate(attempt)?;
    if observations.len() > MAX_IDENTITY_OBSERVATIONS {
        return Err(PaymentError::InvalidSnapshot);
    }
    let mut identities = BTreeSet::new();
    let mut paid = BTreeSet::new();
    let mut at = started_at;
    for (revision, observation) in (1..).zip(observations) {
        observation.validate(attempt, started_at)?;
        if observation.revision != revision || observation.recorded_at < at {
            return Err(PaymentError::InvalidSnapshot);
        }
        at = observation.recorded_at;
        identities.insert(observation.intent.payment.id.clone());
        if observation.intent.payment.status == PaymentStatus::Paid {
            paid.insert(observation.intent.payment.id.clone());
        }
    }
    Ok(IdentitySummary {
        provider_ids: identities.into_iter().collect(),
        paid_provider_ids: paid.into_iter().collect(),
    })
}
