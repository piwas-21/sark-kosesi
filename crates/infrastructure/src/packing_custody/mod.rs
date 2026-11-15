//! Private staff packing assertions. No stock, physical shipment or provider effect.
mod model;
mod store;
use crate::{
    shipment_planning::{self, PlanningError},
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{
    request_context::{Clock, Identifiers, ObservedClock},
    staff_access::Capability,
};
use commerce_domain::{order_shipment_components::expected_components, order_shipments::SkuUnits};
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
    store::lock_key(&mut access.tx, request.effect_key).await?;
    let existing = store::lookup(&mut access.tx, request.effect_key).await?;
    if existing
        .as_ref()
        .is_some_and(|(order, _)| order != request.order_id)
    {
        return Err(Error::Conflict);
    }
    let attempt = match existing {
        Some((_, attempt)) => attempt,
        None => crate::order_stock::source::attempt(&mut access.tx, request.order_id)
            .await
            .map_err(|_| Error::Unavailable)?,
    };
    crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(|_| Error::Database)?;
    access.check_time(&observed).map_err(staff)?;
    let history = store::history(&mut access.tx, request.order_id).await?;
    let old = history.iter().find(|r| r.effect_key == request.effect_key);
    let historical = old
        .map(|r| r.recognition_id.as_str())
        .or_else(|| match &request.action {
            Action::Released { target_id } => history
                .iter()
                .find(|r| r.id == *target_id && r.action == "packed")
                .map(|r| r.recognition_id.as_str()),
            _ => None,
        });
    if matches!(request.action, Action::Released { .. }) && historical.is_none() {
        return Err(Error::Conflict);
    }
    let source = shipment_planning::source(&mut access.tx, request.order_id, &attempt, historical)
        .await
        .map_err(source)?;
    let (prior, active_count) = model::validate(&source, &history)?;
    if let Some(old) = old {
        let matches_action = match &request.action {
            Action::Packed { units, packages } => {
                old.action == "packed" && old.units.0 == *units && old.packages.0 == *packages
            }
            Action::Released { target_id } => {
                old.action == "released" && old.target_id.as_deref() == Some(*target_id)
            }
        };
        if old.actor_id != access.actor || !matches_action {
            return Err(Error::Conflict);
        }
        let record = old.clone();
        access.check_time(&observed).map_err(staff)?;
        access.tx.commit().await?;
        return Ok(Receipt {
            record,
            replayed: true,
        });
    }
    if history.len() >= 64
        || (matches!(request.action, Action::Packed { .. })
            && history.len() + active_count + 2 > 64)
    {
        return Err(Error::Conflict);
    }
    let (action, target, units, packages) = match &request.action {
        Action::Packed { units, packages } => {
            let proposed: Vec<_> = units
                .iter()
                .map(|u| SkuUnits {
                    sku: u.sku.clone(),
                    units: u.units,
                })
                .collect();
            let expected = expected_components(
                &source.order,
                &prior,
                &proposed,
                &source.members,
                &source.boms,
            )
            .map_err(|_| Error::InvalidRequest)?;
            let expected: std::collections::BTreeMap<_, _> =
                expected.into_iter().map(|e| (e.code, e.quantity)).collect();
            if model::check_packages(packages)? != expected {
                return Err(Error::InvalidRequest);
            }
            ("packed", None, units.to_vec(), packages.to_vec())
        }
        Action::Released { target_id } => {
            if history
                .iter()
                .any(|r| r.target_id.as_deref() == Some(*target_id))
            {
                return Err(Error::Conflict);
            }
            (
                "released",
                Some((*target_id).to_owned()),
                Vec::new(),
                Vec::new(),
            )
        }
    };
    let at = access.authorized_at(&observed).map_err(staff)?;
    if at < source.order.identity.created_at || history.last().is_some_and(|r| at < r.recorded_at) {
        return Err(Error::ClockUnavailable);
    }
    let record = Record {
        id: fresh(ids)?,
        order_id: request.order_id.into(),
        attempt_id: attempt,
        recognition_id: source.recognition_id.clone(),
        allocation_kind: source.allocation_kind.into(),
        allocation_id: source.allocation_id.clone(),
        actor_id: access.actor.clone(),
        effect_key: request.effect_key.into(),
        generation: i16::try_from(history.len() + 1).map_err(|_| Error::InvalidStoredData)?,
        action: action.into(),
        target_id: target,
        units: sqlx::types::Json(units),
        packages: sqlx::types::Json(packages),
        recorded_at: at,
        event_id: fresh(ids)?,
    };
    store::insert(&mut access.tx, &record).await?;
    let written = store::history(&mut access.tx, request.order_id).await?;
    model::validate(&source, &written)?;
    if written.last() != Some(&record) {
        return Err(Error::InvalidStoredData);
    }
    access.check_time(&observed).map_err(staff)?;
    access.tx.commit().await?;
    Ok(Receipt {
        record,
        replayed: false,
    })
}

#[cfg(test)]
#[path = "../../tests/support/packing_custody.rs"]
mod tests;
