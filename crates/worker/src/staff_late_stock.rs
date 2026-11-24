//! Local/test projection of one validated late-stock review case.
use commerce_infrastructure::{
    late_stock::{
        ReviewReason,
        inbox::{self, CaseStatus, Summary},
    },
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde::Serialize;
use sqlx::PgPool;
use std::io::{self, Write};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    case_id: String,
    case_digest: String,
    order_id: String,
    reason: ReviewReason,
    status: Status,
    next_generation: Option<i32>,
    assessed_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Open,
    Allocated,
}

impl From<Summary> for Case {
    fn from(summary: Summary) -> Self {
        Self {
            case_id: summary.case_id,
            case_digest: summary.case_digest,
            order_id: summary.order_id,
            reason: summary.reason,
            status: match summary.status {
                CaseStatus::Open => Status::Open,
                CaseStatus::Allocated => Status::Allocated,
            },
            next_generation: summary.next_generation,
            assessed_at: summary.assessed_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    kind: &'static str,
    case: Option<Case>,
    next_cursor: Option<String>,
}

pub(super) async fn next(
    pool: &PgPool,
    token: &SessionToken,
    after: Option<&str>,
) -> io::Result<()> {
    let page = Box::pin(inbox::list(pool, token, &SystemClock, after, None, 1))
        .await
        .map_err(|_| io::Error::other("late stock review unavailable"))?;
    let report = Report {
        kind: "late_stock_review_inbox",
        case: page.cases.into_iter().next().map(Case::from),
        next_cursor: page.next_cursor,
    };
    let mut bytes = serde_json::to_vec(&report)
        .map_err(|_| io::Error::other("late stock review output unavailable"))?;
    bytes.push(b'\n');
    io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| io::Error::other("late stock review output unavailable"))
}
