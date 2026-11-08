//! Private immutable preparation evidence; never a provider execution permit.
use crate::{
    payment_attempts::identifier,
    refund_reservations::RefundAllocation,
    staff_access::{SessionToken, StaffError, lock_staff},
};
use commerce_application::{
    request_context::{Clock, Identifiers, ObservedClock},
    staff_access::Capability,
};
use sqlx::PgPool;
mod store;

pub struct ProposalRequest<'a> {
    pub order_id: &'a str,
    pub reservation_id: &'a str,
    pub expected_recognition_id: &'a str,
    pub expected_through_observation: i64,
    pub effect_key: &'a str,
    pub reason: &'a str,
    pub confirmed: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProposalReceipt {
    pub id: String,
    pub order_id: String,
    pub reservation_id: String,
    pub recognition_id: String,
    pub through_observation: i64,
    pub allocation: RefundAllocation,
    pub proposed_at: i64,
    pub replayed: bool,
}
impl std::fmt::Debug for ProposalReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProposalReceipt")
            .field("id", &self.id)
            .field("order_id", &self.order_id)
            .field("reservation_id", &self.reservation_id)
            .field("recognition_id", &self.recognition_id)
            .field("through_observation", &self.through_observation)
            .field("allocation", &"[redacted]")
            .field("proposed_at", &self.proposed_at)
            .field("replayed", &self.replayed)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalError {
    InvalidRequest,
    Denied,
    Conflict,
    Unavailable,
    InvalidStoredData,
    ClockUnavailable,
    EntropyUnavailable,
    Database,
}
impl From<sqlx::Error> for ProposalError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
fn staff(error: StaffError) -> ProposalError {
    match error {
        StaffError::Denied | StaffError::InvalidRequest => ProposalError::Denied,
        StaffError::ClockUnavailable => ProposalError::ClockUnavailable,
        _ => ProposalError::Database,
    }
}
fn fresh(ids: &dyn Identifiers) -> Result<String, ProposalError> {
    ids.next()
        .map(|id| format!("{:032x}", u128::from_be_bytes(id)))
        .map_err(|_| ProposalError::EntropyUnavailable)
}
fn valid(request: &ProposalRequest<'_>) -> bool {
    identifier(request.order_id)
        && identifier(request.reservation_id)
        && identifier(request.expected_recognition_id)
        && (0..=1000).contains(&request.expected_through_observation)
        && commerce_domain::media::valid_command_id(request.effect_key)
        && request.confirmed
        && !request.reason.is_empty()
        && request.reason.len() <= 240
        && request.reason.trim() == request.reason
        && !request.reason.chars().any(char::is_control)
}

pub async fn propose(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    request: ProposalRequest<'_>,
) -> Result<ProposalReceipt, ProposalError> {
    if !valid(&request) {
        return Err(ProposalError::InvalidRequest);
    }
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::RefundExecutionPrepare)
        .await
        .map_err(staff)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("refund-execution-proposal:{}", request.effect_key))
        .execute(&mut *access.tx)
        .await?;
    let prior = store::lookup(&mut access.tx, request.effect_key, None).await?;
    let attempt = match &prior {
        Some(row) => row.attempt_id.clone(),
        None => crate::order_stock::source::attempt(&mut access.tx, request.order_id)
            .await
            .map_err(|e| match e {
                crate::order_stock::SettlementError::NotFound => ProposalError::Unavailable,
                crate::order_stock::SettlementError::Database => ProposalError::Database,
                _ => ProposalError::InvalidStoredData,
            })?,
    };
    crate::order_stock::source::claim_attempt(&mut access.tx, &attempt)
        .await
        .map_err(|_| ProposalError::Database)?;
    access.check_time(&observed).map_err(staff)?;
    if let Some(row) = prior {
        store::validate(&mut access.tx, &row.id).await?;
        if row.order_id != request.order_id
            || row.reservation_id != request.reservation_id
            || row.recognition_id != request.expected_recognition_id
            || row.through_observation != request.expected_through_observation
            || row.reason != request.reason
            || row.actor_id != access.actor
        {
            return Err(ProposalError::Conflict);
        }
        let mut receipt = row.receipt()?;
        receipt.replayed = true;
        access.check_time(&observed).map_err(staff)?;
        access.tx.commit().await?;
        return Ok(receipt);
    }
    if store::reservation_used(&mut access.tx, request.reservation_id, &attempt).await? {
        return Err(ProposalError::Conflict);
    }
    let candidate = store::candidate(&mut access.tx, &attempt, &request).await?;
    let at = access.authorized_at(&observed).map_err(staff)?;
    if at < candidate.minimum_time {
        return Err(ProposalError::ClockUnavailable);
    }
    let id = fresh(ids)?;
    store::insert(
        &mut access.tx,
        &request,
        &candidate,
        &id,
        &fresh(ids)?,
        &access.actor,
        at,
    )
    .await?;
    sqlx::query("SET CONSTRAINTS require_complete_refund_execution_proposal IMMEDIATE")
        .execute(&mut *access.tx)
        .await?;
    let row = store::lookup(&mut access.tx, request.effect_key, Some(&id))
        .await?
        .ok_or(ProposalError::InvalidStoredData)?;
    // The deferred constraint has just checked the complete current source and event.
    let receipt = row.receipt()?;
    access.check_time(&observed).map_err(staff)?;
    access.tx.commit().await?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "0123456789abcdef0123456789abcdef";
    fn request() -> ProposalRequest<'static> {
        ProposalRequest {
            order_id: ID,
            reservation_id: ID,
            expected_recognition_id: ID,
            expected_through_observation: 1,
            effect_key: "effect-1",
            reason: "Customer request",
            confirmed: true,
        }
    }
    #[test]
    fn explicit_intent_requires_valid_ids_confirmation_and_bounded_reason() {
        assert!(valid(&request()));
        assert!(!valid(&ProposalRequest {
            order_id: "invalid",
            ..request()
        }));
        assert!(!valid(&ProposalRequest {
            reservation_id: "invalid",
            ..request()
        }));
        assert!(!valid(&ProposalRequest {
            expected_recognition_id: "invalid",
            ..request()
        }));
        assert!(!valid(&ProposalRequest {
            expected_through_observation: -1,
            ..request()
        }));
        assert!(!valid(&ProposalRequest {
            expected_through_observation: 1001,
            ..request()
        }));
        assert!(!valid(&ProposalRequest {
            effect_key: "unsafe key",
            ..request()
        }));
        assert!(!valid(&ProposalRequest {
            confirmed: false,
            ..request()
        }));
        for reason in ["", " padded", "padded ", "bad\nreason"] {
            assert!(!valid(&ProposalRequest {
                reason,
                ..request()
            }));
        }
        assert!(!valid(&ProposalRequest {
            reason: &"x".repeat(241),
            ..request()
        }));
    }
    #[test]
    fn identifiers_fail_closed_when_entropy_is_unavailable() {
        struct Ids(bool);
        impl Identifiers for Ids {
            fn next(
                &self,
            ) -> Result<[u8; 16], commerce_application::request_context::ContextError> {
                if self.0 {
                    Ok([7; 16])
                } else {
                    Err(commerce_application::request_context::ContextError::EntropyUnavailable)
                }
            }
        }
        assert_eq!(
            fresh(&Ids(true)).expect("identifier"),
            "07070707070707070707070707070707"
        );
        assert_eq!(fresh(&Ids(false)), Err(ProposalError::EntropyUnavailable));
    }
    #[test]
    fn errors_and_debug_do_not_expose_private_intent() {
        assert_eq!(staff(StaffError::Denied), ProposalError::Denied);
        assert_eq!(
            staff(StaffError::Database(sqlx::Error::RowNotFound)),
            ProposalError::Database
        );
        assert_eq!(staff(StaffError::InvalidRequest), ProposalError::Denied);
        assert_eq!(
            staff(StaffError::ClockUnavailable),
            ProposalError::ClockUnavailable
        );
        assert_eq!(
            ProposalError::from(sqlx::Error::RowNotFound),
            ProposalError::Database
        );
        let receipt = ProposalReceipt {
            id: ID.into(),
            order_id: ID.into(),
            reservation_id: ID.into(),
            recognition_id: ID.into(),
            through_observation: 1,
            allocation: RefundAllocation {
                schema_version: 1,
                lines: vec![],
                shipping: crate::refund_reservations::RefundAllocationAmounts {
                    gross_minor: 0,
                    net_minor: 0,
                    vat_minor: 0,
                },
                total: crate::refund_reservations::RefundAllocationAmounts {
                    gross_minor: 0,
                    net_minor: 0,
                    vat_minor: 0,
                },
            },
            proposed_at: 1,
            replayed: false,
        };
        let debug = format!("{receipt:?}");
        assert!(!debug.contains("profile"));
        assert!(!debug.contains("reason"));
    }
}
