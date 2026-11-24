//! Explicit local/test invocation of existing, staff-authorized recovery decisions.
use crate::staff_files::{self, Visibility};
use commerce_infrastructure::{
    late_stock::disposition::{self, DecisionOutcome, RetryRequest},
    payment_attempts::reconciliation::operator::{self, ResumeRequest},
    runtime::{SecureIdentifiers, SystemClock},
    staff_access::SessionToken,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use sqlx::PgPool;
use std::io::{self, Write};

const MAX_BYTES: u64 = 8 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    RetryLateStock,
    ResumePayment,
}
impl Action {
    pub(super) fn from_flag(flag: &str) -> Option<Self> {
        match flag {
            "--retry-late-stock" => Some(Self::RetryLateStock),
            "--resume-payment-recovery" => Some(Self::ResumePayment),
            _ => None,
        }
    }

    pub(super) fn read(self, path: &str) -> io::Result<Intent> {
        let bytes =
            staff_files::read(path, MAX_BYTES, Visibility::OwnerOnly).map_err(|_| input_error())?;
        match self {
            Self::RetryLateStock => decode(&bytes).map(Intent::RetryLateStock),
            Self::ResumePayment => decode(&bytes).map(Intent::ResumePayment),
        }
    }
}

fn input_error() -> io::Error {
    io::Error::other("operator recovery input unavailable")
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(input_error());
    }
    serde_json::from_slice(bytes).map_err(|_| input_error())
}

pub(super) enum Intent {
    RetryLateStock(LateStockIntent),
    ResumePayment(PaymentIntent),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LateStockIntent {
    case_digest: String,
    expected_generation: i32,
    effect_key: String,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct PaymentIntent {
    profile_id: String,
    expected_generation: i64,
    expected_resume_revision: i32,
    effect_key: String,
    reason: String,
}

pub(super) async fn run(
    pool: &PgPool,
    token: &SessionToken,
    target: &str,
    intent: Intent,
) -> io::Result<()> {
    let report = match intent {
        Intent::RetryLateStock(input) => {
            let receipt = Box::pin(disposition::retry(
                pool,
                token,
                &SystemClock,
                &SecureIdentifiers,
                RetryRequest {
                    case_id: target,
                    case_digest: &input.case_digest,
                    effect_key: &input.effect_key,
                    expected_generation: input.expected_generation,
                    reason: &input.reason,
                    confirmed: true,
                },
            ))
            .await
            .map_err(|_| io::Error::other("late stock retry unavailable"))?;
            let outcome = match receipt.outcome {
                DecisionOutcome::Allocated { allocation_id } => {
                    json!({"state": "allocated", "allocationId": allocation_id})
                }
                DecisionOutcome::StillScarce => json!({"state": "still_scarce"}),
            };
            json!({
                "kind": "late_stock_retry",
                "decisionId": receipt.id,
                "caseId": receipt.case_id,
                "generation": receipt.generation,
                "recordedAt": receipt.recorded_at,
                "outcome": outcome,
                "replayed": receipt.replayed,
            })
        }
        Intent::ResumePayment(input) => {
            let receipt = operator::resume_transient(
                pool,
                token,
                &SystemClock,
                ResumeRequest {
                    attempt_id: target,
                    profile_id: &input.profile_id,
                    expected_generation: input.expected_generation,
                    expected_resume_revision: input.expected_resume_revision,
                    effect_key: &input.effect_key,
                    reason: &input.reason,
                    confirmed: true,
                },
            )
            .await
            .map_err(|_| io::Error::other("payment recovery resume unavailable"))?;
            json!({
                "kind": "payment_recovery_resume",
                "generation": receipt.after.generation,
                "resumeRevision": receipt.revision,
                "phase": receipt.after.phase,
                "replayed": receipt.replayed,
            })
        }
    };
    let unavailable = || io::Error::other("operator recovery output unavailable");
    let mut bytes = serde_json::to_vec(&report).map_err(|_| unavailable())?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| unavailable())
}
