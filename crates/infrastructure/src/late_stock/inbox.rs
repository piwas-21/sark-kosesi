//! Private, bounded staff inspection of historical late-stock review stops.
use super::{LateStockError, Outcome, ReviewReason, identifier, rows};
use crate::staff_access::{SessionToken, StaffError, lock_staff};
use commerce_application::{request_context::Clock, staff_access::Capability};
use sqlx::PgPool;

/// Limits full historical proofs, each of which may include a 2 MiB footprint.
pub const MAX_CANDIDATES: u8 = 4;

#[derive(Debug)]
pub enum InboxError {
    InvalidRequest,
    Staff(StaffError),
    Custody(LateStockError),
}

/// Deliberately excludes customer details, footprints, provider and payment data.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub case_id: String,
    pub order_id: String,
    pub assessed_at: i64,
    pub reason: ReviewReason,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub cases: Vec<Summary>,
    /// Last scanned candidate, even when filtering produces no matches. A full
    /// page may be followed by an empty page; None means this scan reached its end.
    pub next_cursor: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: String,
    order_id: String,
}

/// A live keyset scan, not a snapshot: refresh from the beginning to include
/// cases inserted at or before a previously returned cursor. Filtering happens
/// after validation, and the page fails closed if any scanned custody is invalid.
pub async fn list(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    after: Option<&str>,
    reason: Option<ReviewReason>,
    limit: u8,
) -> Result<Page, InboxError> {
    if limit == 0 || limit > MAX_CANDIDATES || after.is_some_and(|value| !identifier(value)) {
        return Err(InboxError::InvalidRequest);
    }
    let mut access = lock_staff(pool, token, clock, Capability::StockReview)
        .await
        .map_err(InboxError::Staff)?;
    // Separate queries preserve an index range seek even under generic plans.
    let candidates: Vec<Candidate> = if let Some(after) = after {
        sqlx::query_as("SELECT left(id,33) AS id,left(order_id,33) AS order_id FROM late_stock_review_cases WHERE id COLLATE \"C\" > $1 COLLATE \"C\" ORDER BY id COLLATE \"C\" LIMIT $2")
            .bind(after).bind(i64::from(limit)).fetch_all(&mut *access.tx).await
    } else {
        sqlx::query_as("SELECT left(id,33) AS id,left(order_id,33) AS order_id FROM late_stock_review_cases ORDER BY id COLLATE \"C\" LIMIT $1")
            .bind(i64::from(limit)).fetch_all(&mut *access.tx).await
    }.map_err(|error| InboxError::Custody(error.into()))?;
    let mut cases = Vec::new();
    for candidate in &candidates {
        if !identifier(&candidate.id) || !identifier(&candidate.order_id) {
            return Err(InboxError::Custody(LateStockError::InvalidStoredData));
        }
        let outcome = rows::read(&mut access.tx, &candidate.order_id)
            .await
            .map_err(InboxError::Custody)?;
        let Some(Outcome::NeedsReview(case)) = outcome else {
            return Err(InboxError::Custody(LateStockError::InvalidStoredData));
        };
        if case.id != candidate.id {
            return Err(InboxError::Custody(LateStockError::InvalidStoredData));
        }
        if reason.is_none_or(|wanted| wanted == case.reason) {
            cases.push(Summary {
                case_id: case.id,
                order_id: case.order_id,
                assessed_at: case.assessed_at,
                reason: case.reason,
            });
        }
    }
    let next_cursor = if candidates.len() == usize::from(limit) {
        candidates.last().map(|candidate| candidate.id.clone())
    } else {
        None
    };
    // Recheck after potentially expensive proofs, before releasing locked authority.
    access.check_time(clock).map_err(InboxError::Staff)?;
    access
        .tx
        .commit()
        .await
        .map_err(|error| InboxError::Custody(error.into()))?;
    Ok(Page { cases, next_cursor })
}
