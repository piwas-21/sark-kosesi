//! Private fingerprints of purported evidence. No physical or financial authority.
mod command;
mod store;

use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{
    request_context::{Clock, Identifiers, ObservedClock},
    staff_access::Capability,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    Denied,
    Conflict,
    Unavailable,
    InvalidStoredData,
    ClockUnavailable,
    EntropyUnavailable,
    Database,
}
impl From<sqlx::Error> for Error {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
fn staff(error: StaffError) -> Error {
    match error {
        StaffError::Denied | StaffError::InvalidRequest => Error::Denied,
        StaffError::ClockUnavailable => Error::ClockUnavailable,
        _ => Error::Database,
    }
}
fn packing(error: crate::packing_custody::Error) -> Error {
    match error {
        crate::packing_custody::Error::Database => Error::Database,
        _ => Error::InvalidStoredData,
    }
}
fn source(error: crate::shipment_planning::PlanningError) -> Error {
    match error {
        crate::shipment_planning::PlanningError::Database(_) => Error::Database,
        crate::shipment_planning::PlanningError::Unavailable => Error::Unavailable,
        _ => Error::InvalidStoredData,
    }
}
fn fresh(ids: &dyn Identifiers) -> Result<String, Error> {
    ids.next()
        .map(|id| format!("{:032x}", u128::from_be_bytes(id)))
        .map_err(|_| Error::EntropyUnavailable)
}

/// Opaque input is borrowed and never persisted or formatted by this module.
pub struct Request<'a> {
    pub order_id: &'a str,
    pub packing_id: &'a str,
    pub effect_key: &'a str,
    pub bytes: &'a [u8],
    pub confirmed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct Submission {
    pub id: String,
    pub order_id: String,
    pub attempt_id: String,
    pub packing_id: String,
    pub actor_id: String,
    pub effect_key: String,
    pub generation: i16,
    pub evidence_sha256: String,
    pub recorded_at: i64,
    pub event_id: String,
}
pub struct Receipt {
    pub submission: Submission,
    pub replayed: bool,
}

/// Records the fingerprint of bytes described by staff as possible handover evidence.
pub async fn submit(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    request: Request<'_>,
) -> Result<Receipt, Error> {
    if !commerce_domain::order_stock::identifier(request.order_id)
        || !commerce_domain::order_stock::identifier(request.packing_id)
        || !commerce_domain::media::valid_command_id(request.effect_key)
        || !(1..=MAX_BYTES).contains(&request.bytes.len())
        || !request.confirmed
    {
        return Err(Error::InvalidRequest);
    }
    let digest = format!("{:x}", Sha256::digest(request.bytes));
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::HandoverRecord)
        .await
        .map_err(staff)?;
    let mut watermark = 0;
    let result = async {
        sqlx::query("SET LOCAL search_path=pg_catalog,public,pg_temp")
            .execute(&mut *access.tx)
            .await?;
        command::execute(
            &mut access,
            &observed,
            ids,
            &request,
            &digest,
            &mut watermark,
        )
        .await
    }
    .await;
    let now = access.authorized_at(&observed).map_err(staff)?;
    if now < watermark {
        return Err(Error::ClockUnavailable);
    }
    let receipt = result?;
    access.commit(&observed).await.map_err(staff)?;
    Ok(receipt)
}

/// Exact order history, including whether each parent was later released.
pub async fn inspect(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    order_id: &str,
) -> Result<Vec<(Submission, bool)>, Error> {
    if !commerce_domain::order_stock::identifier(order_id) {
        return Err(Error::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::HandoverRecord)
        .await
        .map_err(staff)?;
    let mut watermark = 0;
    let result = async {
        sqlx::query("SET LOCAL search_path=pg_catalog,public,pg_temp")
            .execute(&mut *access.tx)
            .await?;
        command::inspect(&mut access, order_id, &mut watermark).await
    }
    .await;
    let now = access.authorized_at(&observed).map_err(staff)?;
    if now < watermark {
        return Err(Error::ClockUnavailable);
    }
    drop(access);
    result
}

#[cfg(test)]
#[path = "../../tests/support/handover_evidence.rs"]
mod tests;
