//! Shared disposable payment custody and worker process fixtures for review commands.
use commerce_application::request_context::{Clock, Identifiers};
use commerce_domain::{
    Currency,
    payment_attempts::{
        AttemptSnapshot, PaymentStatus, ProviderPayment,
        observations::{ObservationIntent, ObservationSnapshot},
    },
};
use commerce_infrastructure::{
    order_stock::settle,
    payment_attempts::{identities::resolve, recognition::recognize},
};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, PgPool};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

fn run(pool: &PgPool, args: &[&str], token: &[u8; 32]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-worker"));
    command
        .args(args)
        .env_clear()
        .env("APP_ENV", "test")
        .env(
            "DATABASE_URL",
            pool.connect_options().to_url_lossy().as_str(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let mut child = command.spawn().expect("start private review worker");
    child
        .stdin
        .take()
        .expect("staff input")
        .write_all(token)
        .expect("send synthetic staff input");
    child.wait_with_output().expect("worker result")
}

pub fn report(pool: &PgPool, args: &[&str], token: &[u8; 32]) -> Value {
    let output = run(pool, args, token);
    assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
        1
    );
    assert!(output.stdout.ends_with(b"\n"));
    serde_json::from_slice(&output.stdout).expect("one private report")
}

pub fn denied(pool: &PgPool, args: &[&str], token: &[u8; 32], expected: &str) {
    let output = run(pool, args, token);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("fixed error"),
        format!("Error: Custom {{ kind: Other, error: \"{expected}\" }}\n")
    );
}

pub async fn settle_paid_attempt(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    attempt: &AttemptSnapshot,
) {
    seed_paid_observation(pool, attempt).await;
    let binding = &attempt.binding;
    Box::pin(resolve(
        pool,
        clock,
        ids,
        &binding.id,
        "staff-review-resolution",
    ))
    .await
    .expect("resolved synthetic identity");
    Box::pin(recognize(pool, clock, ids, &binding.id))
        .await
        .expect("received recognition");
    Box::pin(settle(pool, clock, ids, &binding.order_id))
        .await
        .expect("settled synthetic allocation");
}

async fn seed_paid_observation(pool: &PgPool, attempt: &AttemptSnapshot) {
    // Supply only the synthetic provider fact and reciprocal event.
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
                id: "tr_SyntheticReviewPayment".into(),
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
        .bind(serde_json::to_value(&observation).expect("synthetic observation"))
        .execute(&mut *tx).await.expect("record fixture observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',$1,'payment.observation_recorded.v1',$2,1500)")
        .bind(&binding.order_id)
        .bind(json!({"schema_version": 1, "order_id": binding.order_id, "attempt_id": binding.id, "observation_id": observation.id}))
        .execute(&mut *tx).await.expect("record reciprocal fixture event");
    tx.commit().await.expect("complete observation fixture");
}
