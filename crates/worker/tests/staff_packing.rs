//! Exercise the worker process against a synthetic, disposable order.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment_fixture;

use commerce_application::request_context::Clock;
use commerce_domain::{
    Currency,
    payment_attempts::{
        AttemptSnapshot, PaymentStatus, ProviderPayment,
        observations::{ObservationIntent, ObservationSnapshot},
    },
};
use commerce_infrastructure::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    order_stock::settle,
    packing_custody::{self, Action, Component, Request, Units},
    payment_attempts::{
        identities::resolve, prepare_attempt, recognition::recognize, start_dispatch,
    },
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, Executor, PgPool};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
    sync::atomic::AtomicU64,
};

async fn seed_paid_observation(pool: &PgPool, attempt: &AttemptSnapshot) {
    // Seed the sole provider fact in the isolated fixture. The DB constraints
    // and real recognition, settlement and packing operations remain active.
    let binding = &attempt.binding;
    let observation = ObservationSnapshot {
        schema_version: 1,
        id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        attempt_id: binding.id.clone(),
        effect_key: "fixture-paid-observation".into(),
        revision: 1,
        recorded_at: 1500,
        intent: ObservationIntent {
            payment: ProviderPayment {
                resource: "payment".into(),
                id: "tr_SyntheticPackingPayment".into(),
                profile_id: binding.profile.id.clone(),
                mode: binding.profile.mode,
                currency: Currency::EUR,
                amount_minor: binding.amount_minor,
                metadata_schema_version: 1,
                order_id: binding.order_id.clone(),
                attempt_id: binding.id.clone(),
                status: PaymentStatus::Paid,
            },
            credential_fingerprint: binding.profile.credential_fingerprint.clone(),
        },
    };
    let mut tx = pool.begin().await.expect("fixture transaction");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,1,1500,$4,$5,'paid',$6)")
        .bind(&observation.id).bind(&binding.id).bind(&observation.effect_key)
        .bind(&observation.intent.payment.id).bind(&observation.intent.credential_fingerprint)
        .bind(serde_json::to_value(&observation).expect("typed synthetic observation"))
        .execute(&mut *tx).await.expect("synthetic paid observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',$1,'payment.observation_recorded.v1',$2,1500)")
        .bind(&binding.order_id)
        .bind(json!({"schema_version": 1, "order_id": binding.order_id, "attempt_id": binding.id, "observation_id": observation.id}))
        .execute(&mut *tx).await.expect("synthetic observation event");
    tx.commit().await.expect("reciprocal observation and event");
}

fn run(database: &str, args: &[&str], bearer: &[u8; 32]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    command
        .args(args)
        .env_clear()
        .env("APP_ENV", "test")
        .env("DATABASE_URL", database)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Preserve only the coverage output path when this real CLI test is
    // instrumented; credentials and all other parent settings stay isolated.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let mut child = command.spawn().expect("packing worker");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(bearer)
        .expect("staff test token");
    child.wait_with_output().expect("packing worker result")
}

async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM packing_custody),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM payment_recognitions)")
        .fetch_one(pool).await.expect("packing and related effect counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn packing_cli_returns_exact_assertions_only_to_warehouse_staff(pool: PgPool) {
    let (store, clock, session, csrf, order) = payment_fixture::accepted(&pool).await;
    let config = payment_fixture::config();
    let ids = payment_fixture::Ids(AtomicU64::new(80_000));
    let owner = GuestCredential::from_session(&session).expect("fixture owner");
    let prepared = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "packing-worker-attempt",
        &payment_fixture::intent(&order),
        &config,
    )
    .await
    .expect("original attempt");
    let attempt = prepared.frozen.snapshot();
    let attempt_id = &attempt.binding.id;
    start_dispatch(&pool, clock.as_ref(), &ids, attempt_id, &config)
        .await
        .expect("original dispatch start");
    seed_paid_observation(&pool, attempt).await;
    resolve(
        &pool,
        clock.as_ref(),
        &ids,
        attempt_id,
        "packing-worker-resolution",
    )
    .await
    .expect("resolved synthetic identity");
    recognize(&pool, clock.as_ref(), &ids, attempt_id)
        .await
        .expect("received recognition");
    let order_id = &order.identity.id;
    settle(&pool, clock.as_ref(), &ids, order_id)
        .await
        .expect("settled synthetic allocation");

    let packer = *b"synthetic-packer-000000000000000";
    let observer = *b"synthetic-reader-000000000000000";
    let other_packer = *b"synthetic-other--000000000000000";
    let now =
        i64::try_from(SystemClock.unix_seconds().expect("system clock")).expect("clock fits SQL");
    pool.execute("INSERT INTO staff_accounts VALUES ('packing-worker',true),('stock-review-only',true),('other-packer',true); INSERT INTO staff_capabilities VALUES ('packing-worker','warehouse.manage'),('stock-review-only','stock.review'),('other-packer','warehouse.manage')")
        .await.expect("test staff grants");
    for (id, bearer) in [
        ("packing-worker", packer),
        ("stock-review-only", observer),
        ("other-packer", other_packer),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,$3,$4,$3)")
            .bind(SessionToken::from_bytes(bearer).digest().as_slice())
            .bind(id).bind(now - 1).bind(now + 300).execute(&pool).await.expect("staff session");
    }
    let receipt = packing_custody::record(
        &pool,
        &SessionToken::from_bytes(packer),
        &SystemClock,
        &ids,
        Request {
            order_id,
            effect_key: "packing-worker-pack",
            confirmed: true,
            action: Action::Packed {
                units: &[Units {
                    sku: "SET-FILLED".into(),
                    units: 1,
                }],
                packages: &[vec![
                    Component {
                        code: "SEAT-COVER".into(),
                        quantity: 2,
                    },
                    Component {
                        code: "SEAT-FOAM".into(),
                        quantity: 2,
                    },
                ]],
            },
        },
    )
    .await
    .expect("record synthetic package assertion");
    let before = footprint(&pool).await;
    let database = pool.connect_options().to_url_lossy().to_string();
    let result = run(&database, &["--inspect-packing", order_id], &packer);
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).expect("single JSON report"),
        json!({
            "kind": "packing_assertions",
            "lines": [{"sku": "SET-FILLED", "ordered": 3, "activeAsserted": 1, "notAsserted": 2}],
            "history": [{"id": receipt.record.id, "action": "packed", "targetId": null, "recordedAt": receipt.record.recorded_at}],
        })
    );
    for (id, token) in [
        (order_id.as_str(), &observer),
        ("cccccccccccccccccccccccccccccccc", &packer),
    ] {
        let denied = run(&database, &["--inspect-packing", id], token);
        assert!(!denied.status.success());
        assert!(denied.stdout.is_empty());
        let error = String::from_utf8(denied.stderr).expect("process diagnostic");
        assert!(error.contains("packing inspection unavailable"));
        for sensitive in [
            id,
            database.as_str(),
            "synthetic-reader",
            "synthetic-packer",
            "tr_SyntheticPackingPayment",
        ] {
            assert!(!error.contains(sensitive));
        }
    }
    assert_eq!(before, footprint(&pool).await);

    let target = &receipt.record.id;
    let release_args = [
        "--release-packing",
        order_id,
        target,
        "packing-release",
        "--confirm",
    ];
    let result = run(&database, &release_args, &packer);
    assert!(result.status.success(), "release failed");
    assert!(result.stderr.is_empty());
    let released: Value = serde_json::from_slice(&result.stdout).expect("release receipt");
    assert_eq!(released["kind"], "packing_assertion_release");
    assert_eq!(released["targetId"], *target);
    assert_eq!(released["replayed"], false);
    assert_eq!(released.as_object().expect("closed receipt").len(), 5);
    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_outbox e JOIN packing_custody p ON p.event_id=e.id WHERE e.aggregate_id=$1 AND e.kind='order.packing_custody_recorded.v1' AND p.action='released' AND e.payload=packing_custody_event(p)",
    )
    .bind(order_id).fetch_one(&pool).await.expect("release event");
    assert_eq!(event_count, 1);
    let after_release = footprint(&pool).await;
    assert_eq!(
        after_release,
        (before.0 + 1, before.1 + 1, before.2, before.3, before.4)
    );
    let replay = run(&database, &release_args, &packer);
    assert!(replay.status.success());
    let replayed: Value = serde_json::from_slice(&replay.stdout).expect("replay receipt");
    assert_eq!(replayed["id"], released["id"]);
    assert_eq!(replayed["targetId"], released["targetId"]);
    assert_eq!(replayed["recordedAt"], released["recordedAt"]);
    assert_eq!(replayed["replayed"], true);
    for (args, bearer) in [
        (
            vec![
                "--release-packing",
                order_id.as_str(),
                target.as_str(),
                "another-effect",
                "--confirm",
            ],
            &packer,
        ),
        (
            vec![
                "--release-packing",
                order_id.as_str(),
                "cccccccccccccccccccccccccccccccc",
                "packing-release",
                "--confirm",
            ],
            &packer,
        ),
        (release_args.to_vec(), &observer),
        (release_args.to_vec(), &other_packer),
    ] {
        let denied = run(&database, &args, bearer);
        assert!(!denied.status.success());
        assert!(denied.stdout.is_empty());
        let diagnostic = String::from_utf8(denied.stderr).expect("redacted diagnostic");
        assert!(diagnostic.contains("packing release unavailable"));
        for secret in [
            database.as_str(),
            "synthetic-packer",
            "synthetic-reader",
            "tr_SyntheticPackingPayment",
        ] {
            assert!(!diagnostic.contains(secret));
        }
    }
    sqlx::query("UPDATE staff_sessions SET revoked_at=$1 WHERE staff_id='packing-worker'")
        .bind(now)
        .execute(&pool)
        .await
        .expect("revoke session");
    assert!(!run(&database, &release_args, &packer).status.success());
    let stale = *b"synthetic-stale--000000000000000";
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'packing-worker',$2,$3,$2)")
        .bind(SessionToken::from_bytes(stale).digest().as_slice())
        .bind(now - 902).bind(now + 300).execute(&pool).await.expect("stale MFA session");
    assert!(!run(&database, &release_args, &stale).status.success());
    assert_eq!(after_release, footprint(&pool).await);
}
