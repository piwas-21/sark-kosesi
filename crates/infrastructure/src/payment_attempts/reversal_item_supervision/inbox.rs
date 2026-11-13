//! Private one-candidate review scan; not recovery or financial authority.
use super::{
    identifier,
    review::{self, ReviewError, Summary},
};
use crate::staff_access::{SessionToken, lock_staff};
use commerce_application::{
    request_context::{Clock, ObservedClock},
    staff_access::Capability,
};
use sqlx::{PgPool, Postgres, Transaction};

const MAX_TIME: i64 = 9_007_199_254_740_991;

/// Position in a live scan, not proof that earlier jobs remain unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    updated_at: i64,
    job_id: String,
}
impl Cursor {
    pub fn new(updated_at: i64, job_id: &str) -> Result<Self, ReviewError> {
        if !(0..=MAX_TIME).contains(&updated_at) || !identifier(job_id) {
            return Err(ReviewError::InvalidRequest);
        }
        Ok(Self {
            updated_at,
            job_id: job_id.into(),
        })
    }
    pub fn updated_at(&self) -> i64 {
        self.updated_at
    }
    pub fn job_id(&self) -> &str {
        &self.job_id
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// Pass this position back to scan the next candidate.
    pub position: Cursor,
    pub summary: Summary,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: String,
    attempt_id: String,
    updated_at: i64,
}

/// Returns one validated current review stop or an empty scan result.
/// This is a live keyset scan: refresh from the beginning for earlier changes.
/// Even a returned last entry can be followed by an empty scan.
pub async fn next(
    pool: &PgPool,
    token: &SessionToken,
    clock: &dyn Clock,
    after: Option<&Cursor>,
) -> Result<Option<Entry>, ReviewError> {
    let observed = ObservedClock::new(clock, 0);
    let mut access = lock_staff(pool, token, &observed, Capability::ReversalReview)
        .await
        .map_err(review::staff_error)?;
    let result = examine(&mut access.tx, after).await;
    // An empty result or damaged custody is not disclosed after authority expires.
    let authority = access.check_time(&observed).map_err(review::staff_error);
    let rollback = access
        .tx
        .rollback()
        .await
        .map_err(|_| ReviewError::Unavailable);
    authority?;
    rollback?;
    result
}

async fn examine(
    tx: &mut Transaction<'_, Postgres>,
    after: Option<&Cursor>,
) -> Result<Option<Entry>, ReviewError> {
    let candidate: Option<Candidate> = if let Some(cursor) = after {
        sqlx::query_as("SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,updated_at FROM reversal_item_jobs WHERE state='review' AND (updated_at,id COLLATE \"C\")>($1,$2 COLLATE \"C\") ORDER BY updated_at,id COLLATE \"C\" LIMIT 1")
            .bind(cursor.updated_at).bind(&cursor.job_id).fetch_optional(&mut **tx).await
    } else {
        sqlx::query_as("SELECT left(id,33) AS id,left(attempt_id,33) AS attempt_id,updated_at FROM reversal_item_jobs WHERE state='review' ORDER BY updated_at,id COLLATE \"C\" LIMIT 1")
            .fetch_optional(&mut **tx).await
    }.map_err(|_| ReviewError::Unavailable)?;
    let Some(candidate) = candidate else {
        return Ok(None);
    };
    let position =
        Cursor::new(candidate.updated_at, &candidate.id).map_err(|_| ReviewError::Unavailable)?;
    if !identifier(&candidate.attempt_id) {
        return Err(ReviewError::Unavailable);
    }
    let (job, summary) = review::validated(tx, &candidate.id)
        .await
        .map_err(|_| ReviewError::Unavailable)?
        .ok_or(ReviewError::Unavailable)?;
    if job.id != candidate.id
        || job.attempt_id != candidate.attempt_id
        || job.updated_at != candidate.updated_at
    {
        return Err(ReviewError::Unavailable);
    }
    Ok(Some(Entry { position, summary }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_is_exact_and_bounded() {
        let id = "a".repeat(32);
        let cursor = Cursor::new(MAX_TIME, &id).expect("bounded");
        assert_eq!(
            (cursor.updated_at(), cursor.job_id()),
            (MAX_TIME, id.as_str())
        );
        for (time, id) in [
            (-1, "a".repeat(32)),
            (MAX_TIME + 1, "a".repeat(32)),
            (1, "A".repeat(32)),
            (1, "a".repeat(31)),
            (1, "a".repeat(33)),
        ] {
            assert_eq!(Cursor::new(time, &id), Err(ReviewError::InvalidRequest));
        }
    }
}
