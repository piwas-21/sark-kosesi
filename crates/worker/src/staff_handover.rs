//! Local/test view of recorded handover evidence and its packing parent.
use commerce_infrastructure::{
    handover_evidence, runtime::SystemClock, staff_access::SessionToken,
};
use serde_json::json;
use sqlx::PgPool;
use std::io::{self, Write};

pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, order: &str) -> io::Result<()> {
    let submissions = handover_evidence::inspect(pool, token, &SystemClock, order)
        .await
        .map_err(|_| io::Error::other("handover evidence unavailable"))?;
    let submissions: Vec<_> = submissions
        .into_iter()
        .map(|(submission, released)| {
            json!({
                "id": submission.id,
                "packingId": submission.packing_id,
                "generation": submission.generation,
                "recordedAt": submission.recorded_at,
                "packingReleased": released,
            })
        })
        .collect();
    let report = json!({"kind": "handover_evidence", "submissions": submissions});
    let mut bytes = serde_json::to_vec(&report)
        .map_err(|_| io::Error::other("handover evidence output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("handover evidence output unavailable"))
}
