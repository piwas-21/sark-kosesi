//! Exact child observations only. They confer no financial or inventory authority.
use super::*;
use commerce_domain::payment_attempts::{PaymentMode, PaymentStatus};
use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// The observed phase is a provider fact, not a local financial disposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReversalPhase {
    RefundQueued,
    RefundPending,
    RefundProcessing,
    RefundRefunded,
    RefundFailed,
    RefundCanceled,
    ChargebackReceived,
    ChargebackReversed,
}

/// Authenticated exact-item evidence. No public constructor, serialization or Debug.
pub struct VerifiedReversal {
    attempt: AttemptSnapshot,
    payment_id: String,
    child_id: String,
    phase: ReversalPhase,
    amount_minor: i64,
    credential_fingerprint: String,
    created_at: OffsetDateTime,
    reversed_at: Option<OffsetDateTime>,
}
impl VerifiedReversal {
    pub fn attempt(&self) -> &AttemptSnapshot {
        &self.attempt
    }
    pub fn payment_id(&self) -> &str {
        &self.payment_id
    }
    pub fn child_id(&self) -> &str {
        &self.child_id
    }
    pub fn phase(&self) -> ReversalPhase {
        self.phase
    }
    pub fn amount_minor(&self) -> i64 {
        self.amount_minor
    }
    pub fn credential_fingerprint(&self) -> &str {
        &self.credential_fingerprint
    }
    pub fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }
    pub fn reversed_at(&self) -> Option<OffsetDateTime> {
        self.reversed_at
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Refund {
    resource: String,
    id: String,
    payment_id: String,
    mode: PaymentMode,
    amount: wire::Amount,
    status: RefundStatus,
    created_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RefundStatus {
    Queued,
    Pending,
    Processing,
    Refunded,
    Failed,
    Canceled,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chargeback {
    resource: String,
    id: String,
    payment_id: String,
    // Current chargeback schema omits mode; reject any contradictory value if supplied.
    #[serde(default, deserialize_with = "optional_mode")]
    mode: Option<PaymentMode>,
    amount: wire::Amount,
    created_at: String,
    // `deserialize_with` prevents Serde treating a missing field as null.
    #[serde(deserialize_with = "required_nullable")]
    reversed_at: Option<String>,
}
fn required_nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
fn optional_mode<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<PaymentMode>, D::Error> {
    PaymentMode::deserialize(d).map(Some)
}
fn timestamp(value: &str) -> Result<OffsetDateTime, MollieError> {
    if value.len() > 64 {
        return Err(MollieError::InvalidResponse);
    }
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| MollieError::InvalidResponse)
}
fn child_id(id: &str, prefix: &str) -> bool {
    id.starts_with(prefix)
        && id.len() > prefix.len()
        && id.len() <= 256
        && id.trim() == id
        && !id.chars().any(char::is_control)
}
fn amount(value: wire::Amount, maximum: i64) -> Result<i64, MollieError> {
    let commerce_domain::Currency::EUR = value.currency;
    let minor = wire::minor(&value.value)?;
    if minor <= 0 || minor > maximum {
        return Err(MollieError::PaymentMismatch);
    }
    Ok(minor)
}
impl VerifiedReconciliationClient {
    /// Verify a paid original and then fetch one exact refund. 404 is unavailable,
    /// never proof that the payment has no refund.
    pub async fn fetch_refund(
        &self,
        attempt: &AttemptSnapshot,
        payment_id: &str,
        id: &str,
    ) -> Result<VerifiedReversal, MollieError> {
        self.fetch_reversal(attempt, payment_id, id, false).await
    }
    /// Verify a paid original and then fetch one exact chargeback.
    pub async fn fetch_chargeback(
        &self,
        attempt: &AttemptSnapshot,
        payment_id: &str,
        id: &str,
    ) -> Result<VerifiedReversal, MollieError> {
        self.fetch_reversal(attempt, payment_id, id, true).await
    }
    async fn fetch_reversal(
        &self,
        attempt: &AttemptSnapshot,
        payment_id: &str,
        id: &str,
        chargeback: bool,
    ) -> Result<VerifiedReversal, MollieError> {
        let (prefix, collection) = if chargeback {
            ("chb_", "chargebacks")
        } else {
            ("re_", "refunds")
        };
        if !provider_id(payment_id) || !child_id(id, prefix) {
            return Err(MollieError::PaymentMismatch);
        }
        let payment = self.fetch(attempt, payment_id).await?;
        if !matches!(payment.payment.status, PaymentStatus::Paid) {
            return Err(MollieError::PaymentMismatch);
        }
        let body = self
            .inner
            .http
            .get(&["payments", payment_id, collection, id])
            .await?;
        let object = wire::json(&body)?;
        if !object
            .get("amount")
            .is_some_and(serde_json::Value::is_object)
        {
            return Err(MollieError::InvalidResponse);
        }
        let (phase, amount_minor, created_at, reversed_at) = if chargeback {
            let item: Chargeback =
                serde_json::from_slice(&body).map_err(|_| MollieError::InvalidResponse)?;
            if item.resource != "chargeback" || item.id != id || item.payment_id != payment_id {
                return Err(MollieError::PaymentMismatch);
            }
            if item
                .mode
                .is_some_and(|mode| !matches!(mode, PaymentMode::Test))
            {
                return Err(MollieError::PaymentMismatch);
            }
            let created = timestamp(&item.created_at)?;
            let reversed = item.reversed_at.as_deref().map(timestamp).transpose()?;
            if reversed.is_some_and(|at| at < created) {
                return Err(MollieError::InvalidResponse);
            }
            (
                if reversed.is_some() {
                    ReversalPhase::ChargebackReversed
                } else {
                    ReversalPhase::ChargebackReceived
                },
                amount(item.amount, payment.payment.amount_minor)?,
                created,
                reversed,
            )
        } else {
            let item: Refund =
                serde_json::from_slice(&body).map_err(|_| MollieError::InvalidResponse)?;
            if item.resource != "refund"
                || item.id != id
                || item.payment_id != payment_id
                || !matches!(item.mode, PaymentMode::Test)
            {
                return Err(MollieError::PaymentMismatch);
            }
            let phase = match item.status {
                RefundStatus::Queued => ReversalPhase::RefundQueued,
                RefundStatus::Pending => ReversalPhase::RefundPending,
                RefundStatus::Processing => ReversalPhase::RefundProcessing,
                RefundStatus::Refunded => ReversalPhase::RefundRefunded,
                RefundStatus::Failed => ReversalPhase::RefundFailed,
                RefundStatus::Canceled => ReversalPhase::RefundCanceled,
            };
            (
                phase,
                amount(item.amount, payment.payment.amount_minor)?,
                timestamp(&item.created_at)?,
                None,
            )
        };
        Ok(VerifiedReversal {
            attempt: attempt.clone(),
            payment_id: payment_id.into(),
            child_id: id.into(),
            phase,
            amount_minor,
            credential_fingerprint: payment.credential_fingerprint,
            created_at,
            reversed_at,
        })
    }
}
