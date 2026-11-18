//! Local/test-only staff review. The bearer is never an argument or log field.
use commerce_infrastructure::{
    payment_attempts::reversal_item_supervision::{
        inbox::{self, Cursor},
        review::{self, Reason, Summary},
    },
    runtime::SystemClock,
    staff_access::SessionToken,
};
use sqlx::PgPool;
use std::io;
pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, id: &str) -> io::Result<()> {
    let summary = review::inspect(pool, token, &SystemClock, id)
        .await
        .map_err(|_| io::Error::other("staff review unavailable"))?;
    match summary {
        None => tracing::info!(code = "reversal_item_review_absent"),
        Some(summary) => {
            let (reason, generation, late_receipt_present) = project(&summary);
            tracing::info!(
                code = "reversal_item_review",
                reason,
                generation,
                late_receipt_present
            );
        }
    }
    Ok(())
}

pub(super) async fn next(
    pool: &PgPool,
    token: &SessionToken,
    after: Option<&Cursor>,
) -> io::Result<()> {
    let entry = inbox::next(pool, token, &SystemClock, after)
        .await
        .map_err(|_| io::Error::other("staff review unavailable"))?;
    match entry {
        None => tracing::info!(code = "reversal_item_review_inbox_empty"),
        Some(entry) => {
            let (updated_at, job_id, reason, generation, late_receipt_present) =
                project_entry(&entry);
            tracing::info!(
                code = "reversal_item_review_inbox_entry",
                updated_at,
                job_id,
                reason,
                generation,
                late_receipt_present
            );
        }
    }
    Ok(())
}

fn project_entry(entry: &inbox::Entry) -> (i64, &str, &'static str, i32, bool) {
    let (reason, generation, late_receipt_present) = project(&entry.summary);
    (
        entry.position.updated_at(),
        entry.position.job_id(),
        reason,
        generation,
        late_receipt_present,
    )
}

fn project(summary: &Summary) -> (&'static str, i32, bool) {
    let reason = match summary.reason {
        Reason::Uncertain => "uncertain",
        Reason::Invalid => "invalid",
        Reason::RetryExhausted => "retry_exhausted",
        Reason::ObservationLimit => "observation_limit",
    };
    (reason, summary.generation, summary.late_receipt_present)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_closed_reason_labels_are_emitted() {
        for (reason, label) in [
            (Reason::Uncertain, "uncertain"),
            (Reason::Invalid, "invalid"),
            (Reason::RetryExhausted, "retry_exhausted"),
            (Reason::ObservationLimit, "observation_limit"),
        ] {
            assert_eq!(
                project(&Summary {
                    reason,
                    generation: 2,
                    late_receipt_present: true
                }),
                (label, 2, true)
            );
            assert_eq!(
                project(&Summary {
                    reason,
                    generation: 1,
                    late_receipt_present: false
                }),
                (label, 1, false)
            );
        }
    }
    #[test]
    fn inbox_entry_projects_only_the_internal_position_and_closed_summary() {
        let id = "0123456789abcdef0123456789abcdef";
        let entry = inbox::Entry {
            position: Cursor::new(1500, id).expect("valid synthetic position"),
            summary: Summary {
                reason: Reason::Invalid,
                generation: 1,
                late_receipt_present: false,
            },
        };
        assert_eq!(project_entry(&entry), (1500, id, "invalid", 1, false));
    }
}
