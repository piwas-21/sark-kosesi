//! Explicit local/test refund reservation and test-mode proposal preparation.
use crate::{staff_files, staff_refund_proposal::allocation};
use commerce_infrastructure::{
    refund_execution_proposals::{ProposalRequest, propose},
    refund_reservations::{
        PreparationPhase, RefundLine, RefundRequest, inspect_preparation_source, reserve,
    },
    runtime::{SecureIdentifiers, SystemClock},
    staff_access::SessionToken,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::io::{self, Write};

const MAX_BYTES: u64 = 8 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Reserve,
    Propose,
}

impl Action {
    pub(super) fn from_flag(flag: &str) -> Option<Self> {
        match flag {
            "--reserve-refund" => Some(Self::Reserve),
            "--propose-refund" => Some(Self::Propose),
            _ => None,
        }
    }

    pub(super) fn read(self, path: &str) -> io::Result<Intent> {
        match self {
            Self::Reserve => staff_files::read_private_json(path, MAX_BYTES).map(Intent::Reserve),
            Self::Propose => staff_files::read_private_json(path, MAX_BYTES).map(Intent::Propose),
        }
        .map_err(|_| io::Error::other("refund preparation input unavailable"))
    }
}

pub(super) enum Intent {
    Reserve(ReservationIntent),
    Propose(ProposalIntent),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ReservationIntent {
    expected_recognition_id: String,
    effect_key: String,
    lines: Vec<Line>,
    shipping_gross_minor: i64,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Line {
    sku: String,
    gross_minor: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ProposalIntent {
    reservation_id: String,
    expected_recognition_id: String,
    expected_through_observation: i64,
    effect_key: String,
    reason: String,
}

pub(super) async fn inspect(
    pool: &PgPool,
    token: &SessionToken,
    order: &str,
    phase: PreparationPhase,
) -> io::Result<()> {
    let source = Box::pin(inspect_preparation_source(
        pool,
        token,
        &SystemClock,
        order,
        phase,
    ))
    .await
    .map_err(|_| io::Error::other("refund preparation source unavailable"))?;
    let label = match phase {
        PreparationPhase::Reservation => "reservation",
        PreparationPhase::Proposal => "proposal",
    };
    let source = source.map(|binding| match binding.through_observation {
        Some(through) => json!({
            "orderId": binding.order_id,
            "recognitionId": binding.recognition_id,
            "throughObservation": through,
        }),
        None => json!({"orderId": binding.order_id, "recognitionId": binding.recognition_id}),
    });
    write_report(json!({"kind": "refund_preparation_source", "phase": label, "source": source}))
}

pub(super) async fn run(
    pool: &PgPool,
    token: &SessionToken,
    order: &str,
    intent: Intent,
) -> io::Result<()> {
    let report = match intent {
        Intent::Reserve(input) => {
            let lines: Vec<_> = input
                .lines
                .iter()
                .map(|line| RefundLine {
                    sku: &line.sku,
                    gross_minor: line.gross_minor,
                })
                .collect();
            let receipt = Box::pin(reserve(
                pool,
                token,
                &SystemClock,
                &SecureIdentifiers,
                RefundRequest {
                    order_id: order,
                    expected_recognition_id: &input.expected_recognition_id,
                    effect_key: &input.effect_key,
                    lines: &lines,
                    shipping_gross_minor: input.shipping_gross_minor,
                    reason: &input.reason,
                    confirmed: true,
                },
            ))
            .await
            .map_err(|_| io::Error::other("refund reservation unavailable"))?;
            json!({
                "kind": "refund_reservation",
                "id": receipt.id,
                "orderId": receipt.order_id,
                "generation": receipt.generation,
                "reservedAt": receipt.reserved_at,
                "replayed": receipt.replayed,
                "allocation": allocation(receipt.allocation),
            })
        }
        Intent::Propose(input) => {
            let receipt = Box::pin(propose(
                pool,
                token,
                &SystemClock,
                &SecureIdentifiers,
                ProposalRequest {
                    order_id: order,
                    reservation_id: &input.reservation_id,
                    expected_recognition_id: &input.expected_recognition_id,
                    expected_through_observation: input.expected_through_observation,
                    effect_key: &input.effect_key,
                    reason: &input.reason,
                    confirmed: true,
                },
            ))
            .await
            .map_err(|_| io::Error::other("refund proposal preparation unavailable"))?;
            json!({
                "kind": "refund_proposal_prepared",
                "id": receipt.id,
                "orderId": receipt.order_id,
                "reservationId": receipt.reservation_id,
                "proposedAt": receipt.proposed_at,
                "replayed": receipt.replayed,
                "allocation": allocation(receipt.allocation),
            })
        }
    };
    write_report(report)
}

fn write_report(report: Value) -> io::Result<()> {
    let unavailable = || io::Error::other("refund preparation output unavailable");
    let mut bytes = serde_json::to_vec(&report).map_err(|_| unavailable())?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| unavailable())
}
