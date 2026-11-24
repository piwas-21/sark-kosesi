//! Closed local/test reports for recorded post-dispatch cancellation requests.
use super::staff_packing::PackingSummary;
use commerce_infrastructure::{
    checkout_cancellations::{self, CancellationInspection, PackingEvidence},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde::Serialize;
use sqlx::PgPool;
use std::io::{self, Write};

#[derive(Serialize)]
enum Kind {
    #[serde(rename = "post_dispatch_cancellation_request")]
    Request,
    #[serde(rename = "post_dispatch_cancellation_inbox")]
    Inbox,
    #[serde(rename = "post_dispatch_cancellation_packing")]
    Packing,
}

#[derive(Serialize)]
struct RequestReport {
    kind: Kind,
    request: Option<Request>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    order_id: String,
    requested_at: i64,
}

impl From<CancellationInspection> for Request {
    fn from(value: CancellationInspection) -> Self {
        Self {
            order_id: value.order_id,
            requested_at: value.requested_at,
        }
    }
}

#[derive(Serialize)]
struct EvidenceReport {
    kind: Kind,
    evidence: Option<Evidence>,
}

#[derive(Serialize)]
struct Evidence {
    request: Request,
    packing: Packing,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Packing {
    Unavailable,
    Validated(PackingSummary),
}

pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, order: &str) -> io::Result<()> {
    let request =
        checkout_cancellations::inspect_post_dispatch_request(pool, token, &SystemClock, order)
            .await
            .map_err(|_| io::Error::other("cancellation review unavailable"))?;
    write_report(&RequestReport {
        kind: Kind::Request,
        request: request.map(Request::from),
    })
}

pub(super) async fn next(
    pool: &PgPool,
    token: &SessionToken,
    after: Option<&str>,
) -> io::Result<()> {
    let request =
        checkout_cancellations::next_post_dispatch_request(pool, token, &SystemClock, after)
            .await
            .map_err(|_| io::Error::other("cancellation review unavailable"))?;
    write_report(&RequestReport {
        kind: Kind::Inbox,
        request: request.map(Request::from),
    })
}

pub(super) async fn inspect_packing(
    pool: &PgPool,
    token: &SessionToken,
    order: &str,
) -> io::Result<()> {
    let evidence = Box::pin(checkout_cancellations::inspect_cancellation_packing(
        pool,
        token,
        &SystemClock,
        order,
    ))
    .await
    .map_err(|_| io::Error::other("cancellation packing inspection unavailable"))?;
    write_report(&EvidenceReport {
        kind: Kind::Packing,
        evidence: evidence.map(|value| Evidence {
            request: Request::from(value.request),
            packing: match value.packing {
                PackingEvidence::Unavailable => Packing::Unavailable,
                PackingEvidence::Validated(inspection) => Packing::Validated(inspection.into()),
            },
        }),
    })
}

fn write_report(report: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(report)
        .map_err(|_| io::Error::other("cancellation output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("cancellation output unavailable"))
}
