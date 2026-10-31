use super::*;
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentStatus {
    Open,
    Pending,
    Authorized,
    Paid,
    Canceled,
    Expired,
    Failed,
}
/// Projection from a server-authenticated provider fetch, never callback/redirect claims.
/// Matching this value alone performs no order, stock, payment or fulfilment transition.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPayment {
    pub resource: String,
    pub id: String,
    pub profile_id: String,
    pub mode: PaymentMode,
    pub currency: Currency,
    pub amount_minor: i64,
    pub metadata_schema_version: u16,
    pub order_id: String,
    pub attempt_id: String,
    pub status: PaymentStatus,
}
/// No current hold deadline: genuine late paid evidence must not be discarded.
pub fn match_provider(
    attempt: &AttemptSnapshot,
    payment: &ProviderPayment,
) -> Result<(), PaymentError> {
    attempt.validate()?;
    // Version 1 uses closed Test/EUR types; other modes/currencies cannot decode.
    if payment.resource != "payment"
        || !opaque(&payment.id, "tr_")
        || payment.profile_id != attempt.profile_id
        || payment.amount_minor != attempt.amount_minor
        || payment.metadata_schema_version != ATTEMPT_SCHEMA_VERSION
        || payment.order_id != attempt.order_id
        || payment.attempt_id != attempt.id
    {
        return Err(PaymentError::ProviderMismatch);
    }
    Ok(())
}
