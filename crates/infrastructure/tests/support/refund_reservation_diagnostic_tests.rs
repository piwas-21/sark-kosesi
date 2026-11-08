use super::*;
use crate::MIGRATOR;
use sqlx::{Executor, PgPool};

#[sqlx::test(migrator = "MIGRATOR")]
async fn reports_only_static_phase_and_real_sqlstate_with_isolated_scopes(pool: PgPool) {
    let mut timed = pool.begin().await.expect("timeout transaction");
    timed
        .execute("SET LOCAL statement_timeout='20ms'")
        .await
        .expect("fixed timeout");
    let setting: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&mut *timed)
        .await
        .expect("actual timeout");
    assert_eq!(setting, "20ms");
    let mut constraint = pool.begin().await.expect("constraint transaction");
    constraint
        .execute(
            "CREATE TEMP TABLE diagnostic_check (value integer CHECK (value > 0)) ON COMMIT DROP",
        )
        .await
        .expect("isolated check");
    let timeout = capture(async {
        sql_phase!(
            "native_history",
            sqlx::query("SELECT pg_sleep(0.1)")
                .execute(&mut *timed)
                .await
        )
        .map(|_| ())
    });
    let violation = capture(async {
        sql_phase!(
            "reservation_insert",
            sqlx::query("INSERT INTO diagnostic_check VALUES (0)")
                .execute(&mut *constraint)
                .await
        )
        .map(|_| ())
    });
    let ((timed_result, timed_failure), (constraint_result, constraint_failure)) =
        tokio::join!(timeout, violation);
    assert_eq!(timed_result, Err(RefundError::Database));
    assert_eq!(
        timed_failure,
        Some(Failure {
            phase: "native_history",
            code: Code::SqlState(*b"57014")
        })
    );
    assert_eq!(constraint_result, Err(RefundError::Database));
    assert_eq!(
        constraint_failure,
        Some(Failure {
            phase: "reservation_insert",
            code: Code::SqlState(*b"23514")
        })
    );
    timed.rollback().await.expect("discard timeout");
    constraint.rollback().await.expect("discard violation");
    record("outside_scope", &sqlx::Error::PoolTimedOut);
    let (_, non_database) = capture(async {
        record("readback", &sqlx::Error::PoolTimedOut);
        Err::<(), _>(RefundError::Database)
    })
    .await;
    assert_eq!(
        non_database,
        Some(Failure {
            phase: "readback",
            code: Code::NonDatabase
        })
    );
    let (success, absent) = capture(async { Ok::<_, RefundError>(()) }).await;
    assert_eq!(success, Ok(()));
    assert_eq!(absent, None);
    let (unmapped, absent) = capture(async { Err::<(), _>(RefundError::Database) }).await;
    assert_eq!(unmapped, Err(RefundError::Database));
    assert_eq!(absent, None);
    let check: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&pool)
        .await
        .expect("pool still usable");
    assert_eq!(check, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_claim_keeps_the_existing_database_timeouts(pool: PgPool) {
    use crate::staff_access::{SessionToken, lock_staff};
    use commerce_application::{
        request_context::{Clock, ContextError, ObservedClock},
        staff_access::Capability,
    };
    struct Time;
    impl Clock for Time {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(1500)
        }
    }
    let token = SessionToken::from_bytes([94; 32]);
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('diagnostic-finance',true); INSERT INTO staff_capabilities VALUES('diagnostic-finance','refund.prepare')").await.expect("isolated staff grant");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,'diagnostic-finance',1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .execute(&pool)
        .await
        .expect("synthetic session");
    let time = Time;
    let observed = ObservedClock::new(&time, 0);
    let mut access = lock_staff(&pool, &token, &observed, Capability::RefundPrepare)
        .await
        .expect("valid staff claim");
    let statement: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&mut *access.tx)
        .await
        .expect("statement bound");
    let lock: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&mut *access.tx)
        .await
        .expect("lock bound");
    assert_eq!((statement.as_str(), lock.as_str()), ("2s", "1s"));
    access.tx.rollback().await.expect("discard staff claim");
}
