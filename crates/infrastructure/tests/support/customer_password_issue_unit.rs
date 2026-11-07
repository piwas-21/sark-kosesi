use super::*;
use std::{collections::VecDeque, sync::Mutex};

struct IssueClock(Mutex<VecDeque<Result<u64, ContextError>>>);
impl Clock for IssueClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
            .lock()
            .expect("clock lock")
            .pop_front()
            .unwrap_or(Err(ContextError::ClockUnavailable))
    }
}

fn issue_clock(samples: &[u64]) -> IssueClock {
    IssueClock(Mutex::new(samples.iter().copied().map(Ok).collect()))
}

async fn issued_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM customer_password_challenges")
        .fetch_one(pool)
        .await
        .expect("challenge count")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_rejects_corrupt_returned_rows_and_rolls_back(pool: PgPool) {
    let time = clock();
    let first = account(&pool, "issue-first@example.com", &time).await;
    let second = account(&pool, "issue-second@example.com", &time).await;

    // The fault runs after the ordinary custody guard. This simulates a database
    // returning a row different from the values Rust submitted, while observing
    // whether the entire insert is rolled back. The expiry constraint alone is
    // removed in this isolated test database so that its mismatch can reach Rust.
    sqlx::query(
        "DO $$ DECLARE constraint_name text; BEGIN \
         SELECT conname INTO STRICT constraint_name FROM pg_constraint \
         WHERE conrelid='customer_password_challenges'::regclass \
           AND contype='c' AND pg_get_constraintdef(oid) LIKE '%900%'; \
         EXECUTE format('ALTER TABLE customer_password_challenges DROP CONSTRAINT %I', constraint_name); \
         END $$",
    )
    .execute(&pool)
    .await
    .expect("remove expiry constraint for fault injection");
    sqlx::query(
        "CREATE TABLE issue_fault_control (field text NOT NULL, target_account bigint NOT NULL)",
    )
    .execute(&pool)
    .await
    .expect("fault control table");
    sqlx::query("INSERT INTO issue_fault_control VALUES ('none', $1)")
        .bind(second)
        .execute(&pool)
        .await
        .expect("fault control row");
    sqlx::query(
        "CREATE FUNCTION issue_row_fault() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           CASE (SELECT field FROM issue_fault_control)
             WHEN 'verifier' THEN NEW.verifier := decode(repeat('ab', 32), 'hex');
             WHEN 'account' THEN NEW.account_id := (SELECT target_account FROM issue_fault_control);
             WHEN 'revision' THEN NEW.expected_revision := 1;
             WHEN 'created' THEN NEW.created_at := NEW.created_at + 1;
             WHEN 'expires' THEN NEW.expires_at := NEW.expires_at + 1;
             WHEN 'consumed' THEN NEW.consumed_at := NEW.created_at;
             WHEN 'xid' THEN NEW.consumed_xid := pg_current_xact_id();
             ELSE NULL;
           END CASE;
           RETURN NEW;
         END $$",
    )
    .execute(&pool)
    .await
    .expect("fault function");
    sqlx::query("CREATE TRIGGER zzz_issue_row_fault BEFORE INSERT ON customer_password_challenges FOR EACH ROW EXECUTE FUNCTION issue_row_fault()")
        .execute(&pool).await.expect("fault trigger");

    for fault in [
        "verifier", "account", "revision", "created", "expires", "consumed", "xid",
    ] {
        sqlx::query("UPDATE issue_fault_control SET field=$1")
            .bind(fault)
            .execute(&pool)
            .await
            .expect("set fault");
        assert!(matches!(
            customer_passwords::issue(&pool, "issue-first@example.com", &time).await,
            Err(PasswordError::Unavailable)
        ));
        assert_eq!(issued_count(&pool).await, 0, "failed issuance rolls back");
    }
    sqlx::query("DROP TRIGGER zzz_issue_row_fault ON customer_password_challenges")
        .execute(&pool)
        .await
        .expect("remove fault");
    let token = customer_passwords::issue(&pool, "issue-first@EXAMPLE.COM", &time)
        .await
        .expect("healthy issue");
    let stored: (Vec<u8>, i64, i32, i64, i64, Option<i64>, bool) = sqlx::query_as(
        "SELECT verifier, account_id, expected_revision, created_at, expires_at, consumed_at, consumed_xid IS NULL FROM customer_password_challenges",
    )
    .fetch_one(&pool)
    .await
    .expect("persisted issue");
    assert_eq!(stored, (verifier(&token), first, 0, 1000, 1900, None, true));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_checks_fresh_time_identity_and_rolls_back_expired_insert(pool: PgPool) {
    let time = clock();
    account(&pool, "issue-clock@example.com", &time).await;
    for email in ["invalid", "missing@example.com", "Issue-clock@example.com"] {
        assert!(matches!(
            customer_passwords::issue(&pool, email, &time).await,
            Err(PasswordError::Denied)
        ));
    }
    assert_eq!(issued_count(&pool).await, 0);

    // Three samples correspond to initial validation, creation and final check.
    for samples in [
        [1000, 1000, 1900],
        [1000, 1000, 999],
        [i64::MAX as u64, i64::MAX as u64, i64::MAX as u64],
    ] {
        assert!(matches!(
            customer_passwords::issue(&pool, "issue-clock@example.com", &issue_clock(&samples))
                .await,
            Err(PasswordError::Unavailable)
        ));
        assert_eq!(issued_count(&pool).await, 0);
    }
    assert!(matches!(
        customer_passwords::issue(&pool, "issue-clock@example.com", &issue_clock(&[])).await,
        Err(PasswordError::Unavailable)
    ));
    assert_eq!(issued_count(&pool).await, 0);
    let valid = issue_clock(&[1000, 1000, 1899]);
    let token = customer_passwords::issue(&pool, "issue-clock@EXAMPLE.COM", &valid)
        .await
        .expect("unexpired issue");
    let stored: (Vec<u8>, i64, i64) =
        sqlx::query_as("SELECT verifier, created_at, expires_at FROM customer_password_challenges")
            .fetch_one(&pool)
            .await
            .expect("persisted challenge");
    assert_eq!(stored, (verifier(&token), 1000, 1900));
}
