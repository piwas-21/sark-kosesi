//! Private staff packing assertions. No stock, physical shipment or provider effect.
mod command;
mod inspection;
pub(crate) mod model;
pub(crate) mod store;
use crate::{
    shipment_planning::{self, PlanningError},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{
    request_context::{Clock, Identifiers, ObservedClock},
    staff_access::Capability,
};
pub use inspection::{Inspection, InspectionEntry, InspectionLine, inspect};
pub use model::{Component, Packages, Record, Units};
use sqlx::PgPool;

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

fn staff(e: StaffError) -> Error {
    match e {
        StaffError::Denied | StaffError::InvalidRequest => Error::Denied,
        StaffError::ClockUnavailable => Error::ClockUnavailable,
        _ => Error::Database,
    }
}
fn source(e: PlanningError) -> Error {
    match e {
        PlanningError::Unavailable => Error::Unavailable,
        PlanningError::Database(_) => Error::Database,
        _ => Error::InvalidStoredData,
    }
}
fn fresh(ids: &dyn Identifiers) -> Result<String, Error> {
    ids.next()
        .map(|id| format!("{:032x}", u128::from_be_bytes(id)))
        .map_err(|_| Error::EntropyUnavailable)
}

pub enum Action<'a> {
    Packed {
        units: &'a [Units],
        packages: &'a [Vec<Component>],
    },
    Released {
        target_id: &'a str,
    },
}
pub struct Request<'a> {
    pub order_id: &'a str,
    pub effect_key: &'a str,
    pub confirmed: bool,
    pub action: Action<'a>,
}
pub struct Receipt {
    pub record: Record,
    pub replayed: bool,
}

/// Journal a staff assertion, with complete prior custody and the attempt claim held.
pub async fn record(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    request: Request<'_>,
) -> Result<Receipt, Error> {
    if !commerce_domain::order_stock::identifier(request.order_id)
        || !commerce_domain::media::valid_command_id(request.effect_key)
        || !request.confirmed
    {
        return Err(Error::InvalidRequest);
    }
    match &request.action {
        Action::Packed { units, packages } => {
            if !(1..=20).contains(&units.len()) {
                return Err(Error::InvalidRequest);
            }
            model::check_packages(packages)?;
        }
        Action::Released { target_id } => {
            if !commerce_domain::order_stock::identifier(target_id) {
                return Err(Error::InvalidRequest);
            }
        }
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::WarehouseManage)
        .await
        .map_err(staff)?;
    // Every result after the staff claim, including errors, crosses this fence.
    let result = command::execute(&mut access, &observed, ids, &request).await;
    access.check_time(&observed).map_err(staff)?;
    let receipt = result?;
    access.commit(&observed).await.map_err(staff)?;
    Ok(receipt)
}

#[cfg(test)]
#[path = "../../tests/support/packing_custody.rs"]
mod tests;
