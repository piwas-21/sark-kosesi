//! Local/test view of one validated historical refund execution proposal.
use commerce_infrastructure::{
    refund_execution_proposals::{self, ProposalInspection},
    refund_reservations::{RefundAllocation, RefundAllocationAmounts},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::io::{self, Write};

pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, id: &str) -> io::Result<()> {
    let proposal = refund_execution_proposals::inspect(pool, token, &SystemClock, id)
        .await
        .map_err(|_| io::Error::other("refund proposal inspection unavailable"))?;
    let report = json!({"kind": "refund_execution_proposal", "proposal": proposal.map(project)});
    let mut bytes = serde_json::to_vec(&report)
        .map_err(|_| io::Error::other("refund proposal output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("refund proposal output unavailable"))
}

fn project(proposal: ProposalInspection) -> Value {
    json!({
        "id": proposal.id,
        "orderId": proposal.order_id,
        "reservationId": proposal.reservation_id,
        "proposedAt": proposal.proposed_at,
        "allocation": allocation(proposal.allocation),
    })
}

pub(super) fn allocation(allocation: RefundAllocation) -> Value {
    let lines: Vec<_> = allocation
        .lines
        .into_iter()
        .map(|line| {
            json!({
                "sku": line.sku,
                "grossMinor": line.gross_minor,
                "netMinor": line.net_minor,
                "vatMinor": line.vat_minor,
            })
        })
        .collect();
    json!({
        "currency": "EUR",
        "lines": lines,
        "shipping": amounts(allocation.shipping),
        "total": amounts(allocation.total),
    })
}

fn amounts(amounts: RefundAllocationAmounts) -> Value {
    json!({
        "grossMinor": amounts.gross_minor,
        "netMinor": amounts.net_minor,
        "vatMinor": amounts.vat_minor,
    })
}
