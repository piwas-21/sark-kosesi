//! Test-only longer wait budget for explicitly scoped, real lock-queue races.
use sqlx::{Postgres, Transaction};
use std::cell::RefCell;
use std::future::Future;

tokio::task_local! { static ORDERED_RACE: (); }
tokio::task_local! { static EFFECT_KEY_ERROR: RefCell<Option<String>>; }

pub(crate) async fn scope<F: Future>(future: F) -> F::Output {
    ORDERED_RACE.scope((), future).await
}

/// Capture only SQLSTATE from the disposition effect-key query, never SQL or data.
pub(crate) async fn diagnose_effect_key<F: Future>(future: F) -> (F::Output, Option<String>) {
    EFFECT_KEY_ERROR
        .scope(RefCell::new(None), async {
            let result = future.await;
            let code = EFFECT_KEY_ERROR.with(|value| value.borrow_mut().take());
            (result, code)
        })
        .await
}

pub(crate) fn record_effect_key_error(error: &sqlx::Error) {
    let code = error.as_database_error().and_then(|error| error.code());
    if let Some(code) = code.filter(|code| {
        code.len() == 5
            && code
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }) {
        let _ = EFFECT_KEY_ERROR.try_with(|value| *value.borrow_mut() = Some(code.into_owned()));
    }
}

pub(crate) async fn apply(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    if ORDERED_RACE.try_with(|()| ()).is_ok() {
        // Two independent three-second wait observations precede gate release.
        sqlx::query("SET LOCAL lock_timeout='10s'")
            .execute(&mut **tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='20s'")
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub(crate) async fn settings(tx: &mut Transaction<'_, Postgres>) -> (String, String) {
    sqlx::query_as("SELECT current_setting('lock_timeout'),current_setting('statement_timeout')")
        .fetch_one(&mut **tx)
        .await
        .expect("test timeout settings")
}

#[tokio::test]
async fn scope_is_not_inherited_by_another_task() {
    assert!(ORDERED_RACE.try_with(|()| ()).is_err());
    scope(async {
        assert!(ORDERED_RACE.try_with(|()| ()).is_ok());
        assert!(
            tokio::spawn(async { ORDERED_RACE.try_with(|()| ()).is_err() })
                .await
                .expect("child task")
        );
    })
    .await;
    assert!(ORDERED_RACE.try_with(|()| ()).is_err());
}
