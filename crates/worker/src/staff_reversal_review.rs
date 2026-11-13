//! Local/test-only staff review. The bearer is never an argument or log field.
use commerce_infrastructure::{
    payment_attempts::reversal_item_supervision::review::{self, Reason},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use sqlx::PgPool;
use std::{
    io::{self, IsTerminal, Read},
    sync::mpsc,
    time::Duration,
};
use zeroize::Zeroizing;

const INPUT_DEADLINE: Duration = Duration::from_secs(5);

/// Exactly 32 raw bytes followed by EOF on a non-terminal stdin.
/// A detached standard thread permits a blocked pipe to time out without
/// making Tokio wait for a blocking task during runtime shutdown.
pub(super) fn read_token() -> io::Result<SessionToken> {
    if io::stdin().is_terminal() {
        return Err(io::Error::other("staff review input unavailable"));
    }
    let (send, recv) = mpsc::sync_channel(1);
    let _reader = std::thread::Builder::new()
        .spawn(move || {
            let mut bytes = Zeroizing::new(Vec::with_capacity(33));
            let read = io::stdin().lock().take(33).read_to_end(&mut bytes);
            let _ = send.send(read.map(|_| bytes));
        })
        .map_err(|_| io::Error::other("staff review input unavailable"))?;
    let bytes = recv
        .recv_timeout(INPUT_DEADLINE)
        .map_err(|_| io::Error::other("staff review input unavailable"))?
        .map_err(|_| io::Error::other("staff review input unavailable"))?;
    if bytes.len() != 32 {
        return Err(io::Error::other("staff review input unavailable"));
    }
    let mut token = Zeroizing::new([0_u8; 32]);
    token.copy_from_slice(&bytes);
    Ok(SessionToken::from_bytes(*token))
}

pub(super) async fn inspect(pool: &PgPool, token: &SessionToken, id: &str) -> io::Result<()> {
    let summary = review::inspect(pool, token, &SystemClock, id)
        .await
        .map_err(|_| io::Error::other("staff review unavailable"))?;
    match summary {
        None => tracing::info!(code = "reversal_item_review_absent"),
        Some(summary) => {
            let reason = reason(summary.reason);
            tracing::info!(
                code = "reversal_item_review",
                reason,
                generation = summary.generation,
                late_receipt_present = summary.late_receipt_present
            );
        }
    }
    Ok(())
}

fn reason(value: Reason) -> &'static str {
    match value {
        Reason::Uncertain => "uncertain",
        Reason::Invalid => "invalid",
        Reason::RetryExhausted => "retry_exhausted",
        Reason::ObservationLimit => "observation_limit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_closed_reason_labels_are_emitted() {
        assert_eq!(reason(Reason::Uncertain), "uncertain");
        assert_eq!(reason(Reason::Invalid), "invalid");
        assert_eq!(reason(Reason::RetryExhausted), "retry_exhausted");
        assert_eq!(reason(Reason::ObservationLimit), "observation_limit");
    }
}
