//! One bounded pass. PostgreSQL row locks are the claim; no external effects.
use super::{HoldCommand, HoldWriteError, StoredHold, limits, transition::transition_locked};
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExpiryBatch {
    pub expired: u16,
    pub failed: u16,
}

fn failure_code(error: &HoldWriteError) -> &'static str {
    match error {
        HoldWriteError::Stock(_) => "invalid_stock",
        HoldWriteError::Database(_) => "database_error",
        _ => "invalid_hold",
    }
}

pub async fn expire_due_holds(
    pool: &PgPool,
    now: i64,
    limit: u16,
) -> Result<ExpiryBatch, HoldWriteError> {
    if now < 0 || !(1..=100).contains(&limit) {
        return Err(HoldWriteError::InvalidRequest);
    }
    let next_attempt = now.checked_add(60).ok_or(HoldWriteError::InvalidRequest)?;
    let mut batch = ExpiryBatch::default();
    for _ in 0..limit {
        let mut tx = pool.begin().await?;
        limits(&mut tx).await?;
        let candidate=sqlx::query_as::<_,StoredHold>(
            "SELECT h.* FROM stock_holds h LEFT JOIN stock_hold_expiry_failures f ON f.hold_id=h.id
             WHERE h.state='active' AND h.expires_at<=$1 AND (f.hold_id IS NULL OR f.next_attempt_at<=$1)
             ORDER BY h.expires_at,h.id LIMIT 1 FOR UPDATE OF h SKIP LOCKED")
            .bind(now).fetch_optional(&mut *tx).await?;
        let Some(stored) = candidate else {
            tx.rollback().await?;
            break;
        };
        // Keep the row claim across rollback of a failed component transition.
        sqlx::query("SAVEPOINT expiry_effect")
            .execute(&mut *tx)
            .await?;
        match transition_locked(
            &mut tx,
            &stored,
            &HoldCommand::Expire {
                actor_id: "system:hold-expiry",
            },
            now,
        )
        .await
        {
            Ok(_) => batch.expired += 1,
            Err(error) => {
                sqlx::query("ROLLBACK TO SAVEPOINT expiry_effect")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("INSERT INTO stock_hold_expiry_failures (hold_id,attempts,last_attempt_at,next_attempt_at,code) VALUES ($1,1,$2,$3,$4)
                    ON CONFLICT (hold_id) DO UPDATE SET attempts=LEAST(stock_hold_expiry_failures.attempts+1,1000000),last_attempt_at=$2,next_attempt_at=$3,code=$4,resolved_at=NULL")
                    .bind(&stored.id).bind(now).bind(next_attempt).bind(failure_code(&error)).execute(&mut *tx).await?;
                batch.failed += 1;
            }
        }
        tx.commit().await?;
    }
    Ok(batch)
}
