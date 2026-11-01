//! Minimal callback lookup custody, independent of any possibly unknown order.
use super::*;
use crate::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, quotes::MAX_MINOR_UNITS};
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupOperationSnapshot {
    pub schema_version: u16,
    pub id: String,
    pub profile_id: String,
    pub provider_id: String,
    pub created_at: i64,
}
impl LookupOperationSnapshot {
    pub fn validate(&self) -> Result<(), PaymentError> {
        if self.schema_version != 1
            || !hex(&self.id, 32)
            || !opaque(&self.profile_id, "pfl_")
            || !opaque(&self.provider_id, "tr_")
            || !(0..=MAX_QUOTE_UNIX_SECONDS).contains(&self.created_at)
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupSnapshot {
    pub schema_version: u16,
    pub id: String,
    pub operation: LookupOperationSnapshot,
    pub started_at: i64,
    pub recorded_at: i64,
    pub payment: ProviderPayment,
    pub credential_fingerprint: String,
}
impl LookupSnapshot {
    pub fn validate(&self) -> Result<(), PaymentError> {
        self.operation.validate()?;
        let p = &self.payment;
        if self.schema_version != 1
            || !hex(&self.id, 32)
            || !(self.operation.created_at..=MAX_QUOTE_UNIX_SECONDS).contains(&self.started_at)
            || !(self.started_at..=MAX_QUOTE_UNIX_SECONDS).contains(&self.recorded_at)
            || !hex(&self.credential_fingerprint, 64)
            || p.resource != "payment"
            || p.id != self.operation.provider_id
            || p.profile_id != self.operation.profile_id
            || p.metadata_schema_version != ATTEMPT_SCHEMA_VERSION
            || !hex(&p.order_id, 32)
            || !hex(&p.attempt_id, 32)
            || !(1..=MAX_MINOR_UNITS).contains(&p.amount_minor)
        {
            return Err(PaymentError::InvalidSnapshot);
        }
        Ok(())
    }
}
