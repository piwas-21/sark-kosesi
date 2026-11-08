//! Test-only, task-scoped classification; never retain SQL, parameters or error text.
use super::RefundError;
use std::{cell::Cell, future::Future, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Code {
    SqlState([u8; 5]),
    Unavailable,
    NonDatabase,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub phase: &'static str,
    pub code: Code,
}
tokio::task_local! { static FAILURE: Cell<Option<Failure>>; }

pub(super) fn record(phase: &'static str, error: &sqlx::Error) {
    let _ = FAILURE.try_with(|slot| {
        let code = match error.as_database_error() {
            None => Code::NonDatabase,
            Some(database) => match database.code() {
                Some(value)
                    if value.len() == 5 && value.bytes().all(|b| b.is_ascii_alphanumeric()) =>
                {
                    let mut code = [0; 5];
                    code.copy_from_slice(value.as_bytes());
                    Code::SqlState(code)
                }
                _ => Code::Unavailable,
            },
        };
        slot.set(Some(Failure { phase, code }));
    });
}

async fn capture<T>(
    future: impl Future<Output = Result<T, RefundError>>,
) -> (Result<T, RefundError>, Option<Failure>) {
    FAILURE
        .scope(Cell::new(None), async {
            let result = future.await;
            let failure = FAILURE.with(Cell::get);
            (result, failure)
        })
        .await
}

pub(crate) async fn scope<T>(
    prefix: i64,
    future: impl Future<Output = Result<T, RefundError>>,
) -> Result<T, RefundError> {
    let start = Instant::now();
    let (result, failure) = capture(future).await;
    if let Err(error) = &result {
        let failure = failure.unwrap_or(Failure {
            phase: "unmapped",
            code: if *error == RefundError::Database {
                Code::Unavailable
            } else {
                Code::NonDatabase
            },
        });
        let code = match &failure.code {
            Code::SqlState(value) => std::str::from_utf8(value).unwrap_or("unavailable"),
            Code::Unavailable => "unavailable",
            Code::NonDatabase => "non_database",
        };
        eprintln!(
            "refund-capacity prefix={prefix} elapsed_ms={} phase={} sqlstate={code}",
            start.elapsed().as_millis(),
            failure.phase
        );
    }
    result
}

#[path = "refund_reservation_diagnostic_tests.rs"]
mod tests;
