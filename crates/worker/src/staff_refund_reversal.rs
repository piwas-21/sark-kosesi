//! Local/test snapshot of bounded refund and reversal evidence for one paid order.
use commerce_infrastructure::{
    refund_execution_proposals::{ReversalEvidence, inspect_reversal_evidence},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde::Serialize;
use sqlx::PgPool;
use std::io::{self, Write};

pub(super) const INSPECTION_UNAVAILABLE: &str = "refund reversal evidence inspection unavailable";
const OUTPUT_UNAVAILABLE: &str = "refund reversal evidence output unavailable";

#[derive(Serialize)]
struct Report {
    kind: &'static str,
    evidence: Option<Evidence>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    order_id: String,
    recognition_id: String,
    through_observation: i64,
    parent_operations: usize,
    parent_pending: usize,
    parent_complete: usize,
    child_observations: usize,
    digest: String,
}

impl From<ReversalEvidence> for Evidence {
    fn from(value: ReversalEvidence) -> Self {
        Self {
            order_id: value.order_id,
            recognition_id: value.recognition_id,
            through_observation: value.through_observation,
            parent_operations: value.parent_operations,
            parent_pending: value.parent_pending,
            parent_complete: value.parent_complete,
            child_observations: value.child_observations,
            digest: value.digest,
        }
    }
}

pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, order: &str) -> io::Result<()> {
    let evidence = inspect_reversal_evidence(pool, token, &SystemClock, order)
        .await
        .map_err(|_| io::Error::other(INSPECTION_UNAVAILABLE))?;
    let report = Report {
        kind: "refund_reversal_evidence",
        evidence: evidence.map(Evidence::from),
    };
    let mut bytes =
        serde_json::to_vec(&report).map_err(|_| io::Error::other(OUTPUT_UNAVAILABLE))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other(OUTPUT_UNAVAILABLE))
}
