//! Private callback evidence custody, not ingress, financial or fulfilment authority.
use super::{
    evidence::{context, limits},
    *,
};
use crate::mollie::CallbackPayment;
use commerce_application::request_context::{Clock, Identifiers};
use sqlx::PgPool;

/// The provider GET must already have completed outside all database transactions.
/// Only the opaque authenticated result enters. Metadata locates the original;
/// its entire immutable order/attempt/dispatch is checked before custody, and the
/// existing recorder repeats original matching under the append claim.
/// A changed fetched status needs a new durable operation; exact retries retain
/// the old receipt. A future inbox must retain that operation before doing I/O.
pub async fn record_fetched(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    effect: &str,
    fetched: &CallbackPayment,
) -> Result<observations::ObservationReceipt, PaymentStoreError> {
    let attempt_id = fetched.attempt_id_hint();
    if !valid_command_id(effect) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (original, _) = context(&mut tx, attempt_id, false).await?;
    let verified = fetched
        .bind(original.snapshot())
        .map_err(|_| PaymentStoreError::Conflict)?;
    tx.rollback().await?;
    observations::record_verified(pool, clock, ids, attempt_id, effect, &verified).await
}
