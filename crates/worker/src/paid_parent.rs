//! Local/test evidence inspection without provider access or sensitive projections.
use commerce_infrastructure::payment_attempts::paid_parent_evidence::{self, Status};
use sqlx::PgPool;
use std::io::{Error, Result};

#[derive(PartialEq, Eq)]
pub(super) struct Inspection {
    attempt: String,
    effect: String,
}
impl Inspection {
    pub(super) fn parse(attempt: &str, effect: &str) -> Result<Self> {
        if !super::reversal_pages::identifier(attempt)
            || effect.is_empty()
            || effect.len() > 128
            || !effect
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::other("unsupported worker command"));
        }
        Ok(Self {
            attempt: attempt.into(),
            effect: effect.into(),
        })
    }
}

pub(super) async fn inspect(pool: &PgPool, input: Inspection) -> Result<()> {
    let status = paid_parent_evidence::lookup(pool, &input.attempt, &input.effect)
        .await
        .map_err(|_| Error::other("paid-parent evidence unavailable"))?;
    match status {
        None => tracing::info!(code = "paid_parent_evidence_state", state = "absent"),
        Some(Status::Pending(_)) => {
            tracing::info!(code = "paid_parent_evidence_state", state = "pending")
        }
        Some(Status::Complete(receipt)) => tracing::info!(
            code = "paid_parent_evidence_state",
            state = "complete",
            refunded_present = receipt.totals.refunded_minor.is_some(),
            remaining_present = receipt.totals.remaining_minor.is_some(),
            captured_present = receipt.totals.captured_minor.is_some(),
            charged_back_present = receipt.totals.charged_back_minor.is_some(),
        ),
    }
    Ok(())
}

pub(super) async fn inspect_set(pool: &PgPool, attempt: &str) -> Result<()> {
    let set = paid_parent_evidence::inspect_set(pool, attempt)
        .await
        .map_err(|_| Error::other("paid-parent evidence set unavailable"))?;
    tracing::info!(
        code = "paid_parent_evidence_set",
        operations = set.operations,
        pending = set.pending,
        complete = set.complete,
        digest = %set.digest,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_identity_and_effect_boundaries() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(Inspection::parse(id, "a-Z9").is_ok());
        assert!(Inspection::parse(id, &"x".repeat(128)).is_ok());
        for invalid in ["", "bad_id", "é", &"x".repeat(129)] {
            assert!(Inspection::parse(id, invalid).is_err());
        }
        for invalid in [
            "",
            "ABCDEF0123456789abcdef0123456789",
            "0123456789abcdef0123456789abcde",
        ] {
            assert!(Inspection::parse(invalid, "effect").is_err());
        }
    }
}
