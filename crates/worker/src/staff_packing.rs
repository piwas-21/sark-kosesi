//! Local/test-only output for a validated view of recorded packing assertions.
use commerce_infrastructure::{
    packing_custody::{self, Inspection},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::io::{self, Write};

pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, order: &str) -> io::Result<()> {
    let inspection = packing_custody::inspect(pool, token, &SystemClock, order)
        .await
        .map_err(|_| io::Error::other("packing inspection unavailable"))?;
    let mut bytes = serde_json::to_vec(&report(inspection))
        .map_err(|_| io::Error::other("packing output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("packing output unavailable"))
}

fn report(inspection: Inspection) -> Value {
    let lines: Vec<_> = inspection
        .lines
        .into_iter()
        .map(|line| {
            json!({
                "sku": line.sku,
                "ordered": line.ordered,
                "activeAsserted": line.active_asserted,
                "notAsserted": line.not_asserted,
            })
        })
        .collect();
    let history: Vec<_> = inspection
        .history
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "action": entry.action,
                "targetId": entry.target_id,
                "recordedAt": entry.recorded_at,
            })
        })
        .collect();
    json!({"kind": "packing_assertions", "lines": lines, "history": history})
}

#[cfg(test)]
mod tests {
    use super::*;
    use commerce_infrastructure::packing_custody::{InspectionEntry, InspectionLine};

    #[test]
    fn output_contains_only_packing_assertions() {
        let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let release = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let result = report(Inspection {
            lines: vec![InspectionLine {
                sku: "SYNTHETIC-FILLED".into(),
                ordered: 2,
                active_asserted: 0,
                not_asserted: 2,
            }],
            history: vec![
                InspectionEntry {
                    id: id.into(),
                    action: "packed".into(),
                    target_id: None,
                    recorded_at: 1500,
                },
                InspectionEntry {
                    id: release.into(),
                    action: "released".into(),
                    target_id: Some(id.into()),
                    recorded_at: 1501,
                },
            ],
        });
        assert_eq!(
            result,
            json!({
                "kind": "packing_assertions",
                "lines": [{"sku": "SYNTHETIC-FILLED", "ordered": 2, "activeAsserted": 0, "notAsserted": 2}],
                "history": [
                    {"id": id, "action": "packed", "targetId": null, "recordedAt": 1500},
                    {"id": release, "action": "released", "targetId": id, "recordedAt": 1501},
                ],
            }),
        );
    }
}
