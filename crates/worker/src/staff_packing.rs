//! Local/test-only output for a validated view of recorded packing assertions.
use commerce_infrastructure::{
    packing_custody::{self, Action, Inspection, Request},
    runtime::{SecureIdentifiers, SystemClock},
    staff_access::SessionToken,
};
use serde::Serialize;
use serde_json::json;
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

/// Record a bounded assertion through the same warehouse custody operation.
pub(super) async fn record(
    pool: &PgPool,
    token: &SessionToken,
    order: &str,
    effect: &str,
    input: &super::staff_packing_input::PackingInput,
) -> io::Result<()> {
    let receipt = packing_custody::record(
        pool,
        token,
        &SystemClock,
        &SecureIdentifiers,
        Request {
            order_id: order,
            effect_key: effect,
            confirmed: true,
            action: Action::Packed {
                units: &input.units,
                packages: &input.packages,
            },
        },
    )
    .await
    .map_err(|_| io::Error::other("packing record unavailable"))?;
    let mut bytes = serde_json::to_vec(&json!({
        "kind": "packing_assertion_record",
        "id": receipt.record.id,
        "recordedAt": receipt.record.recorded_at,
        "replayed": receipt.replayed,
    }))
    .map_err(|_| io::Error::other("packing output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("packing output unavailable"))
}

/// Release only an existing assertion; this does not establish physical unpacking.
pub(super) async fn release(
    pool: &PgPool,
    token: &SessionToken,
    order: &str,
    target: &str,
    effect: &str,
) -> io::Result<()> {
    let receipt = packing_custody::record(
        pool,
        token,
        &SystemClock,
        &SecureIdentifiers,
        Request {
            order_id: order,
            effect_key: effect,
            confirmed: true,
            action: Action::Released { target_id: target },
        },
    )
    .await
    .map_err(|_| io::Error::other("packing release unavailable"))?;
    let mut bytes = serde_json::to_vec(&json!({
        "kind": "packing_assertion_release",
        "id": receipt.record.id,
        "targetId": receipt.record.target_id,
        "recordedAt": receipt.record.recorded_at,
        "replayed": receipt.replayed,
    }))
    .map_err(|_| io::Error::other("packing output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("packing output unavailable"))
}

#[derive(Serialize)]
pub(super) struct PackingSummary {
    lines: Vec<Line>,
    history: Vec<Entry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Line {
    sku: String,
    ordered: u32,
    active_asserted: u32,
    not_asserted: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    action: String,
    target_id: Option<String>,
    recorded_at: i64,
}

impl From<Inspection> for PackingSummary {
    fn from(value: Inspection) -> Self {
        Self {
            lines: value
                .lines
                .into_iter()
                .map(|line| Line {
                    sku: line.sku,
                    ordered: line.ordered,
                    active_asserted: line.active_asserted,
                    not_asserted: line.not_asserted,
                })
                .collect(),
            history: value
                .history
                .into_iter()
                .map(|entry| Entry {
                    id: entry.id,
                    action: entry.action,
                    target_id: entry.target_id,
                    recorded_at: entry.recorded_at,
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct PackingReport {
    kind: &'static str,
    #[serde(flatten)]
    summary: PackingSummary,
}

fn report(inspection: Inspection) -> PackingReport {
    PackingReport {
        kind: "packing_assertions",
        summary: inspection.into(),
    }
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
            serde_json::to_value(result).expect("serialize packing report"),
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
