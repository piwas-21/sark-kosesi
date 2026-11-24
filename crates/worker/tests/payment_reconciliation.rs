//! Worker command rejects production and uninstalled/live keys before provider I/O.
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment_fixture;
use commerce_infrastructure::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    payment_attempts::{DispatchOutcome, prepare_attempt, start_dispatch},
};
use sqlx::{ConnectOptions, Executor, PgPool};
use std::process::{Command, Output};
use std::sync::atomic::AtomicU64;
fn command() -> Command {
    let mut command = worker();
    command.arg("--reconcile-payments");
    command
}
fn worker() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    c.env("APP_ENV", "test")
        .env_remove("MOLLIE_TEST_API_KEY")
        .env_remove("MOLLIE_TEST_PROFILE_ID")
        .env_remove("PAYMENT_RECONCILIATION_BATCH");
    c
}

#[test]
fn checkpoint_inspection_rejects_wrong_environment_and_malformed_arguments() {
    const ATTEMPT: &str = "0123456789abcdef0123456789abcdef";
    for environment in ["staging", "production"] {
        let output = worker()
            .args(["--inspect-payment-recovery", ATTEMPT])
            .env("APP_ENV", environment)
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!output.status.success());
        assert!(log(&output).contains("production is disabled"));
        assert!(!log(&output).contains(ATTEMPT));
    }
    for args in [
        vec!["--inspect-payment-recovery"],
        vec!["--inspect-payment-recovery", "invalid-attempt"],
        vec!["--inspect-payment-recovery", ATTEMPT, "unexpected-argument"],
    ] {
        let output = worker()
            .args(&args)
            .env_remove("DATABASE_URL")
            .output()
            .expect("worker");
        assert!(!output.status.success());
        let text = log(&output);
        assert!(text.contains("unsupported worker command"));
        for argument in args.iter().skip(1) {
            assert!(!text.contains(argument));
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_checkpoint_is_reported_without_provider_configuration_or_effects(pool: PgPool) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let attempt = "0123456789abcdef0123456789abcdef";
    let output = worker()
        .args(["--inspect-payment-recovery", attempt])
        .env("DATABASE_URL", &database)
        .output()
        .expect("worker");
    assert!(output.status.success());
    let text = log(&output);
    assert!(text.contains(r#""code":"payment_recovery_checkpoint""#));
    assert!(text.contains(r#""state":"absent""#));
    assert!(!text.contains(attempt));
    assert!(!text.contains(&database));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("no recovery jobs written");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stored_checkpoint_is_redacted_and_bound_to_dispatch_evidence(pool: PgPool) {
    let (store, clock, session, csrf, order) = payment_fixture::accepted(&pool).await;
    let guest = GuestCredential::from_session(&session).expect("fixture owner");
    let config = payment_fixture::config();
    let prepared = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "inspection-fixture",
        &payment_fixture::intent(&order),
        &config,
    )
    .await
    .expect("prepared payment");
    let binding = &prepared.frozen.snapshot().binding;
    let ids = payment_fixture::Ids(AtomicU64::new(20000));
    assert!(matches!(
        start_dispatch(&pool, clock.as_ref(), &ids, &binding.id, &config).await,
        Ok(DispatchOutcome::Start(_))
    ));
    let source: String = sqlx::query_scalar(
        "SELECT id FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("committed dispatch event");
    sqlx::query(
        "INSERT INTO payment_reconciliation_jobs(attempt_id,source_event_id,created_at,updated_at,due_at)
         VALUES ($1,$2,1500,1500,1500)",
    )
    .bind(&binding.id)
    .bind(&source)
    .execute(&pool)
    .await
    .expect("guarded initial checkpoint");
    pool.execute(
        "UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason='unresolved',
         updated_at=1501,due_at=1501,generation=1,operation=1",
    )
    .await
    .expect("guarded review checkpoint");

    let snapshot = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT to_jsonb(j)::text FROM payment_reconciliation_jobs j",
        )
        .fetch_one(&pool)
        .await
        .expect("read checkpoint snapshot")
    };
    let before = snapshot().await;
    let database = pool.connect_options().to_url_lossy().to_string();
    let inspect = || {
        worker()
            .args(["--inspect-payment-recovery", &binding.id])
            .env("DATABASE_URL", &database)
            .output()
            .expect("worker inspection")
    };
    let output = inspect();
    assert!(output.status.success());
    let message = log(&output);
    for field in [
        r#""code":"payment_recovery_checkpoint""#,
        r#""state":"present""#,
        r#""phase":"NeedsReview""#,
        r#""review_reason":"Some(Unresolved)""#,
        r#""generation":1"#,
        r#""resume_revision":0"#,
        r#""due_at":1501"#,
        r#""lease_until":"None""#,
        r#""polls":0"#,
        r#""failures":0"#,
    ] {
        assert!(message.contains(field), "missing checkpoint field {field}");
    }
    assert!(
        snapshot().await == before,
        "inspection must preserve the checkpoint"
    );

    // Simulate damaged custody in this disposable database after the positive control.
    pool.execute(
        "ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox;
         UPDATE commerce_outbox SET payload='{}' WHERE kind='payment.reconciliation_required.v1';
         ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox",
    )
    .await
    .expect("damage fixture event");
    let damaged = inspect();
    assert!(!damaged.status.success());
    let diagnostic = log(&damaged);
    assert!(diagnostic.contains("payment recovery checkpoint unavailable"));
    assert!(!diagnostic.contains("payment_recovery_checkpoint"));
    assert!(
        snapshot().await == before,
        "inspection must preserve the damaged checkpoint"
    );
    for private in [database.as_str(), &binding.id, &binding.profile.id, &source] {
        assert!(
            !message.contains(private),
            "checkpoint exposed a private value"
        );
        assert!(
            !diagnostic.contains(private),
            "diagnostic exposed a private value"
        );
    }
}
fn log(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
#[test]
fn production_never_enters_payment_recovery() {
    let output = command()
        .env("APP_ENV", "production")
        .env_remove("DATABASE_URL")
        .output()
        .expect("worker");
    assert!(!output.status.success());
    assert!(log(&output).contains("production is disabled"));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_live_invalid_and_nonunicode_configuration_is_redacted(pool: PgPool) {
    let database = pool.connect_options().to_url_lossy().to_string();
    let missing = command()
        .env("DATABASE_URL", &database)
        .output()
        .expect("worker");
    assert!(!missing.status.success());
    assert!(log(&missing).contains("test payment configuration required"));
    assert!(!log(&missing).contains(&database));
    let synthetic = format!("test_{}", "synthetic".repeat(4));
    for (key, profile, batch) in [
        ("live_DO_NOT_USE", "pfl_SYNTHETIC", "10"),
        ("test_short", "pfl_SYNTHETIC", "10"),
        (synthetic.as_str(), "wrong", "10"),
        (synthetic.as_str(), "pfl_SYNTHETIC", "26"),
    ] {
        let output = command()
            .env("DATABASE_URL", &database)
            .env("MOLLIE_TEST_API_KEY", key)
            .env("MOLLIE_TEST_PROFILE_ID", profile)
            .env("PAYMENT_RECONCILIATION_BATCH", batch)
            .output()
            .expect("worker");
        assert!(!output.status.success());
        let l = log(&output);
        assert!(!l.contains(key));
        assert!(!l.contains(profile));
        assert!(!l.contains(&database));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(vec![0xff]);
        let output = command()
            .env("DATABASE_URL", &database)
            .env("MOLLIE_TEST_API_KEY", &synthetic)
            .env("MOLLIE_TEST_PROFILE_ID", "pfl_SYNTHETIC")
            .env("PAYMENT_RECONCILIATION_BATCH", invalid)
            .output()
            .expect("invalid batch");
        assert!(!output.status.success());
        assert!(log(&output).contains("invalid payment reconciliation batch"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("no effects");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn custody_preflight_is_single_attempt_redacted_and_never_mutates(pool: PgPool) {
    let (store, clock, session, csrf, order) = payment_fixture::accepted(&pool).await;
    let guest = GuestCredential::from_session(&session).expect("owner");
    let config = payment_fixture::config();
    let receipt = prepare_attempt(
        &store,
        &guest,
        &csrf,
        "custody-test",
        &payment_fixture::intent(&order),
        &config,
    )
    .await
    .expect("prepare");
    let binding = &receipt.frozen.snapshot().binding;
    let ids = payment_fixture::Ids(AtomicU64::new(20000));
    assert!(matches!(
        start_dispatch(&pool, clock.as_ref(), &ids, &binding.id, &config).await,
        Ok(DispatchOutcome::Start(_))
    ));
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM payment_reconciliation_jobs),(SELECT count(*) FROM payment_observations)").fetch_one(&pool).await.expect("before");
    let database = pool.connect_options().to_url_lossy().to_string();
    let inspect = |id: &str| {
        worker()
            .args(["--inspect-payment-custody", id])
            .env("DATABASE_URL", &database)
            .output()
            .expect("worker")
    };
    let output = inspect(&binding.id);
    assert!(output.status.success());
    let message = log(&output);
    for value in [
        r#""code":"payment_custody_inspection""#,
        r#""observations":0"#,
        r#""identities":0"#,
        r#""paid_observed":false"#,
    ] {
        assert!(message.contains(value), "missing field {value}");
    }
    for private in [
        database.as_str(),
        binding.id.as_str(),
        binding.profile.id.as_str(),
    ] {
        assert!(!message.contains(private));
    }
    assert!(!inspect(&"f".repeat(32)).status.success());
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM payment_reconciliation_jobs),(SELECT count(*) FROM payment_observations)").fetch_one(&pool).await.expect("after");
    assert_eq!(before, after);
}
#[test]
fn custody_preflight_rejects_wrong_mode_and_invalid_identifier_before_database() {
    let invalid = worker()
        .args(["--inspect-payment-custody", "bad"])
        .env_remove("DATABASE_URL")
        .output()
        .expect("worker");
    assert!(!invalid.status.success());
    assert!(log(&invalid).contains("unsupported worker command"));
    let production = worker()
        .args([
            "--inspect-payment-custody",
            "0123456789abcdef0123456789abcdef",
        ])
        .env("APP_ENV", "production")
        .env_remove("DATABASE_URL")
        .output()
        .expect("worker");
    assert!(!production.status.success());
    assert!(log(&production).contains("production is disabled"));
}

#[cfg(unix)]
#[path = "support/recovery_file.rs"]
mod recovery_file;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "support/staff_review.rs"]
mod staff_review;

#[cfg(unix)]
#[sqlx::test(migrator = "MIGRATOR")]
async fn finance_cli_resumes_exact_transient_checkpoint_without_provider_configuration(
    pool: PgPool,
) {
    use commerce_application::request_context::Clock;
    use commerce_infrastructure::{
        payment_attempts::reconciliation::{Phase, read_job},
        runtime::SystemClock,
        staff_access::SessionToken,
    };
    use serde_json::json;
    use staff_review::{denied, report};
    use std::{fs, os::unix::fs::PermissionsExt};

    let (store, clock, session, csrf, order) = Box::pin(payment_fixture::accepted(&pool)).await;
    let guest = GuestCredential::from_session(&session).expect("fixture owner");
    let config = payment_fixture::config();
    let prepared = Box::pin(prepare_attempt(
        &store,
        &guest,
        &csrf,
        "operator-resume-fixture",
        &payment_fixture::intent(&order),
        &config,
    ))
    .await
    .expect("prepared payment");
    let binding = &prepared.frozen.snapshot().binding;
    let ids = payment_fixture::Ids(AtomicU64::new(92_000));
    Box::pin(start_dispatch(
        &pool,
        clock.as_ref(),
        &ids,
        &binding.id,
        &config,
    ))
    .await
    .expect("committed dispatch custody");
    sqlx::query(
        "INSERT INTO payment_reconciliation_jobs(attempt_id,source_event_id,created_at,updated_at,due_at)
         SELECT $1,id,created_at,created_at,created_at FROM commerce_outbox
         WHERE aggregate_id=$2 AND kind='payment.reconciliation_required.v1'",
    )
    .bind(&binding.id)
    .bind(&order.identity.id)
    .execute(&pool)
    .await
    .expect("guarded initial checkpoint");
    for _ in 0..4 {
        pool.execute("UPDATE payment_reconciliation_jobs SET failures=failures+1,generation=generation+1,operation=operation+1")
            .await.expect("guarded earlier transient failure checkpoint");
    }
    pool.execute("UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason='retry_exhausted',failures=5,updated_at=1501,due_at=1501,generation=5,operation=5")
        .await.expect("synthetic exhausted transient checkpoint");

    let finance = [61; 32];
    let reviewer = [62; 32];
    pool.execute("INSERT INTO staff_accounts VALUES ('fixture-finance',true),('fixture-reviewer',true); INSERT INTO staff_capabilities VALUES ('fixture-finance','payment.reconcile'),('fixture-reviewer','stock.review')").await.expect("separate synthetic capabilities");
    let now = i64::try_from(SystemClock.unix_seconds().expect("clock")).expect("SQL time");
    for (id, token) in [("fixture-finance", finance), ("fixture-reviewer", reviewer)] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,$3,$4,$3)")
            .bind(SessionToken::from_bytes(token).digest().as_slice())
            .bind(id).bind(now - 1).bind(now + 300).execute(&pool).await.expect("synthetic staff session");
    }

    let file = recovery_file::IntentFile::new();
    let args = [
        "--resume-payment-recovery",
        &binding.id,
        file.path(),
        "--confirm",
    ];
    let mut payload = json!({
        "profileId":binding.profile.id,
        "expectedGeneration":5,
        "expectedResumeRevision":0,
        "effectKey":"synthetic-finance-resume",
        "reason":"Synthetic checked transport incident",
    });
    let before = payment_fixture::counts(&pool).await;
    let receipt_count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_recovery_resumes")
            .fetch_one(&pool)
            .await
            .expect("immutable recovery audit count")
    };
    for body in [
        b"[\"profile\",5,0,\"effect\",\"reason\"]".as_slice(),
        br#"{"profileId":"profile","expectedGeneration":5,"expectedGeneration":5,"expectedResumeRevision":0,"effectKey":"effect","reason":"reason"}"#,
        &vec![b'x'; 8193],
    ] {
        fs::write(file.path(), body).expect("invalid synthetic intent");
        denied(&pool, &args, &finance, "operator recovery input unavailable");
    }
    file.write(&payload);
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o640))
        .expect("exposed fixture mode");
    denied(
        &pool,
        &args,
        &finance,
        "operator recovery input unavailable",
    );
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600))
        .expect("restore private mode");
    denied(
        &pool,
        &args,
        &reviewer,
        "payment recovery resume unavailable",
    );
    payload["profileId"] = "different-test-profile".into();
    file.write(&payload);
    denied(
        &pool,
        &args,
        &finance,
        "payment recovery resume unavailable",
    );
    payload["profileId"] = binding.profile.id.clone().into();
    payload["expectedResumeRevision"] = 1.into();
    file.write(&payload);
    denied(
        &pool,
        &args,
        &finance,
        "payment recovery resume unavailable",
    );
    assert_eq!(receipt_count().await, 0);
    assert_eq!(payment_fixture::counts(&pool).await, before);

    payload["expectedResumeRevision"] = 0.into();
    file.write(&payload);
    assert_eq!(
        report(&pool, &args, &finance),
        json!({"kind":"payment_recovery_resume","generation":5,"resumeRevision":1,"phase":"search","replayed":false})
    );
    let resumed = read_job(&pool, &binding.id)
        .await
        .expect("valid checkpoint")
        .expect("present");
    assert_eq!(resumed.phase, Phase::Search);
    assert_eq!(
        (
            resumed.generation,
            resumed.operation,
            resumed.failures,
            resumed.resume_revision
        ),
        (5, 5, 0, 1)
    );
    pool.execute("UPDATE payment_reconciliation_jobs SET generation=6,operation=6,failures=1")
        .await
        .expect("later synthetic progress");
    assert_eq!(
        report(&pool, &args, &finance),
        json!({"kind":"payment_recovery_resume","generation":5,"resumeRevision":1,"phase":"search","replayed":true})
    );
    let after = read_job(&pool, &binding.id)
        .await
        .expect("valid checkpoint")
        .expect("present");
    assert_eq!(
        (
            after.generation,
            after.operation,
            after.failures,
            after.resume_revision
        ),
        (6, 6, 1, 1)
    );
    assert_eq!(receipt_count().await, 1);
    assert_eq!(payment_fixture::counts(&pool).await, before);
}
