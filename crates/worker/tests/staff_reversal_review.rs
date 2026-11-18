//! Private command boundary; synthetic input and isolated PostgreSQL only.
#![cfg(unix)]
use commerce_infrastructure::{MIGRATOR, staff_access::SessionToken};
use sqlx::{ConnectOptions, PgPool};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const JOB: &str = "0123456789abcdef0123456789abcdef";
const BEARER: &[u8; 32] = b"synthetic-staff-token-0123456789";

fn base_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    cmd.env("APP_ENV", "test")
        .env_remove("DATABASE_URL")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}
fn command() -> Command {
    let mut cmd = base_command();
    cmd.args(["--review-reversal-item", JOB]);
    cmd
}
fn inbox_command() -> Command {
    let mut cmd = base_command();
    cmd.arg("--review-reversal-inbox");
    cmd
}
fn output(mut cmd: Command, input: &[u8]) -> Output {
    let mut child = cmd.spawn().expect("spawn worker");
    child
        .stdin
        .take()
        .expect("pipe")
        .write_all(input)
        .expect("synthetic input");
    child.wait_with_output().expect("worker output")
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn input_is_bounded_and_fails_before_database_and_provider() {
    let invalid = output(
        {
            let mut c = command();
            c.arg("synthetic-extra-secret");
            c
        },
        b"",
    );
    assert!(!invalid.status.success());
    assert!(text(&invalid).contains("unsupported worker command"));
    assert!(!text(&invalid).contains("synthetic-extra-secret"));
    for input in [b"".as_slice(), &BEARER[..31], &[b'x'; 33]] {
        let result = output(command(), input);
        assert!(!result.status.success());
        assert!(text(&result).contains("staff review input unavailable"));
        assert!(!text(&result).contains("DATABASE_URL required"));
    }
    let result = output(command(), BEARER);
    assert!(!result.status.success());
    assert!(text(&result).contains("DATABASE_URL required"));
    assert!(!text(&result).contains("synthetic-staff-token"));
    assert!(!text(&result).contains("test payment configuration"));
    let result = output(
        {
            let mut c = command();
            c.env("APP_ENV", "production");
            c
        },
        b"",
    );
    assert!(!result.status.success());
    assert!(text(&result).contains("production is disabled"));
}

#[test]
fn inbox_arguments_and_authority_fail_closed_before_database() {
    for args in [
        vec!["--review-reversal-inbox", "1"],
        vec!["--review-reversal-inbox", "-1", JOB],
        vec!["--review-reversal-inbox", "01", JOB],
        vec!["--review-reversal-inbox", "9007199254740992", JOB],
        vec!["--review-reversal-inbox", "1", "secret-invalid-id"],
        vec!["--review-reversal-inbox", "1", JOB, "secret-extra"],
    ] {
        let mut cmd = base_command();
        cmd.args(args);
        let result = output(cmd, b"");
        assert!(!result.status.success());
        assert!(text(&result).contains("unsupported worker command"));
        assert!(!text(&result).contains("secret-"));
    }
    for input in [b"".as_slice(), &BEARER[..31], &[b'x'; 33]] {
        let result = output(inbox_command(), input);
        assert!(!result.status.success());
        assert!(text(&result).contains("staff review input unavailable"));
        assert!(!text(&result).contains("DATABASE_URL required"));
    }
    for environment in ["production", "staging"] {
        let mut cmd = inbox_command();
        cmd.env("APP_ENV", environment);
        let result = output(cmd, b"");
        assert!(!result.status.success());
        assert!(text(&result).contains("production is disabled"));
    }
    let result = output(inbox_command(), BEARER);
    assert!(!result.status.success());
    assert!(text(&result).contains("DATABASE_URL required"));
    assert!(!text(&result).contains("synthetic-staff-token"));
    assert!(!text(&result).contains("test payment configuration"));
}

#[test]
fn open_pipe_cannot_hang_process_after_input_deadline() {
    for mut cmd in [command(), inbox_command()] {
        let mut child = cmd.spawn().expect("spawn worker");
        child
            .stdin
            .as_mut()
            .expect("pipe")
            .write_all(BEARER)
            .expect("synthetic input");
        let start = Instant::now();
        while child.try_wait().expect("poll").is_none() && start.elapsed() < Duration::from_secs(9)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        if child.try_wait().expect("poll").is_none() {
            child.kill().expect("stop stuck synthetic worker");
            let _ = child.wait();
            panic!("worker did not stop after input deadline");
        }
        let result = child.wait_with_output().expect("worker output");
        assert!(!result.status.success());
        assert!(text(&result).contains("staff review input unavailable"));
        assert!(!text(&result).contains("synthetic-staff-token"));
    }
}

#[cfg(unix)]
#[test]
fn input_rejects_excess_without_stdio_read_ahead() {
    use std::{
        io::Read,
        net::Shutdown,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    let (mut retained, mut sender) = UnixStream::pair().expect("socket pair");
    retained
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let child_fd: OwnedFd = retained.try_clone().expect("duplicate reader").into();
    sender.write_all(&[b'x'; 33]).expect("excess input");
    sender.write_all(b"tail").expect("trailing marker");
    sender.shutdown(Shutdown::Write).expect("input EOF");
    let result = command()
        .stdin(Stdio::from(child_fd))
        .output()
        .expect("worker");
    assert!(!result.status.success());
    assert!(text(&result).contains("staff review input unavailable"));
    let mut trailing = [0; 4];
    retained
        .read_exact(&mut trailing)
        .expect("unread trailing bytes");
    assert_eq!(&trailing, b"tail");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_authority_is_required_and_absent_review_is_read_only(pool: PgPool) {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs(),
    )
    .expect("time");
    sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES('synthetic-reviewer',true),('synthetic-wrong',true),('synthetic-revoked',true),('synthetic-expired',true),('synthetic-stale',true)")
        .execute(&pool).await.expect("staff");
    sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES('synthetic-reviewer','reversal.review'),('synthetic-wrong','stock.review'),('synthetic-revoked','reversal.review'),('synthetic-expired','reversal.review'),('synthetic-stale','reversal.review')")
        .execute(&pool).await.expect("capabilities");
    for (id, token, created, expires, mfa, revoked) in [
        (
            "synthetic-reviewer",
            *BEARER,
            now - 5,
            now + 300,
            now - 1,
            None,
        ),
        (
            "synthetic-wrong",
            [7_u8; 32],
            now - 5,
            now + 300,
            now - 1,
            None,
        ),
        (
            "synthetic-revoked",
            [8_u8; 32],
            now - 5,
            now + 300,
            now - 1,
            Some(now - 1),
        ),
        (
            "synthetic-expired",
            [9_u8; 32],
            now - 30,
            now - 1,
            now - 20,
            None,
        ),
        (
            "synthetic-stale",
            [10_u8; 32],
            now - 2000,
            now + 300,
            now - 1000,
            None,
        ),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at,revoked_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(SessionToken::from_bytes(token).digest().as_slice()).bind(id)
            .bind(created).bind(expires).bind(mfa).bind(revoked).execute(&pool).await.expect("session");
    }
    let before: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)")
        .fetch_one(&pool).await.expect("before");
    let database = pool.connect_options().to_url_lossy().to_string();
    let result = output(
        {
            let mut c = command();
            c.env("DATABASE_URL", &database);
            c
        },
        BEARER,
    );
    assert!(result.status.success());
    assert!(text(&result).contains("reversal_item_review_absent"));
    assert!(!text(&result).contains("synthetic-staff-token"));
    let mut inbox = inbox_command();
    inbox.env("DATABASE_URL", &database);
    let empty = output(inbox, BEARER);
    assert!(empty.status.success());
    assert!(text(&empty).contains("reversal_item_review_inbox_empty"));
    assert!(!text(&empty).contains("synthetic-staff-token"));
    let mut after_cursor = inbox_command();
    after_cursor
        .args(["1500", JOB])
        .env("DATABASE_URL", &database);
    let continued = output(after_cursor, BEARER);
    assert!(continued.status.success());
    assert!(text(&continued).contains("reversal_item_review_inbox_empty"));
    for byte in [7_u8, 8, 9, 10, 11] {
        let denied = output(
            {
                let mut c = command();
                c.env("DATABASE_URL", &database);
                c
            },
            &[byte; 32],
        );
        assert!(!denied.status.success());
        assert!(text(&denied).contains("staff review unavailable"));
        let mut inbox = inbox_command();
        inbox.env("DATABASE_URL", &database);
        let denied = output(inbox, &[byte; 32]);
        assert!(!denied.status.success());
        assert!(text(&denied).contains("staff review unavailable"));
        assert!(!text(&denied).contains("reversal_item_review_inbox_empty"));
    }
    let after: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)")
        .fetch_one(&pool).await.expect("after");
    assert_eq!(before, after);
    assert!(!text(&result).contains(&database));
    assert!(!text(&empty).contains(&database));
}

#[test]
fn packing_uses_the_bounded_staff_input_and_rejects_extra_arguments() {
    let mut invalid = base_command();
    invalid.args(["--inspect-packing", JOB, "synthetic-extra-secret"]);
    let rejected = output(invalid, b"");
    assert!(!rejected.status.success());
    assert!(text(&rejected).contains("unsupported worker command"));
    assert!(!text(&rejected).contains("synthetic-extra-secret"));

    for (environment, input, expected) in [
        ("staging", b"".as_slice(), "production is disabled"),
        ("production", b"".as_slice(), "production is disabled"),
        ("test", &BEARER[..31], "staff review input unavailable"),
        ("test", &BEARER[..], "DATABASE_URL required"),
    ] {
        let mut cmd = base_command();
        cmd.args(["--inspect-packing", JOB])
            .env("APP_ENV", environment);
        let rejected = output(cmd, input);
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert!(text(&rejected).contains(expected));
        assert!(!text(&rejected).contains("synthetic-staff-token"));
    }
}
