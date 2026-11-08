//! Optional aggregate fields from one authenticated paid-parent GET.
use super::*;
use commerce_domain::{Currency, payment_attempts::PaymentStatus};
use serde::{Deserialize, Deserializer};

/// Raw provider facts in EUR minor units. Missing fields remain unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReversalTotals {
    pub refunded_minor: Option<i64>,
    pub remaining_minor: Option<i64>,
    pub captured_minor: Option<i64>,
    pub charged_back_minor: Option<i64>,
}

/// Authenticated projection; no public constructor, serialization or Debug.
pub struct VerifiedReversalTotals {
    attempt: AttemptSnapshot,
    payment_id: String,
    credential_fingerprint: String,
    totals: ReversalTotals,
}
impl VerifiedReversalTotals {
    pub fn attempt(&self) -> &AttemptSnapshot {
        &self.attempt
    }
    pub fn payment_id(&self) -> &str {
        &self.payment_id
    }
    pub fn credential_fingerprint(&self) -> &str {
        &self.credential_fingerprint
    }
    pub fn totals(&self) -> ReversalTotals {
        self.totals
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Amounts {
    #[serde(default, deserialize_with = "present_amount")]
    amount_refunded: Option<i64>,
    #[serde(default, deserialize_with = "present_amount")]
    amount_remaining: Option<i64>,
    #[serde(default, deserialize_with = "present_amount")]
    amount_captured: Option<i64>,
    #[serde(default, deserialize_with = "present_amount")]
    amount_charged_back: Option<i64>,
}
fn present_amount<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    // Present null, non-objects and extra or duplicate money keys are malformed.
    let amount = wire::Amount::deserialize(d)?;
    let Currency::EUR = amount.currency;
    wire::minor(&amount.value)
        .map(Some)
        .map_err(|_| serde::de::Error::custom("invalid EUR aggregate"))
}

impl VerifiedReconciliationClient {
    /// Fetch only the exact paid original. These observations neither establish a
    /// net balance nor authorize a refund, even when every field is present.
    pub async fn fetch_reversal_totals(
        &self,
        attempt: &AttemptSnapshot,
        payment_id: &str,
    ) -> Result<VerifiedReversalTotals, MollieError> {
        reconciliation::check(&self.inner.profile, attempt)?;
        if !provider_id(payment_id) {
            return Err(MollieError::PaymentMismatch);
        }
        let body = self.inner.http.get(&["payments", payment_id]).await?;
        let (payment, _) = wire::payment(&body, attempt, Some(payment_id))?;
        if payment.status != PaymentStatus::Paid {
            return Err(MollieError::PaymentMismatch);
        }
        let amounts: Amounts =
            serde_json::from_slice(&body).map_err(|_| MollieError::InvalidResponse)?;
        Ok(VerifiedReversalTotals {
            attempt: attempt.clone(),
            payment_id: payment.id,
            credential_fingerprint: self.inner.profile.credential_fingerprint.clone(),
            totals: ReversalTotals {
                refunded_minor: amounts.amount_refunded,
                remaining_minor: amounts.amount_remaining,
                captured_minor: amounts.amount_captured,
                charged_back_minor: amounts.amount_charged_back,
            },
        })
    }
}
