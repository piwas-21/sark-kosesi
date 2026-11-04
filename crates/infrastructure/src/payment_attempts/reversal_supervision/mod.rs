//! Private bounded reversal-page supervision; no financial or stock authority.
use super::evidence::{context, fresh_id, input, limits, now};
use super::*;
use crate::mollie::{ReversalKind, VerifiedReconciliationClient};
use commerce_application::request_context::{Clock, Identifiers};
use commerce_domain::payment_attempts::PaymentStatus;
use sqlx::PgPool;
mod jobs;
mod rows;
pub mod runner;
mod transitions;
mod validate;
pub use runner::run_batch;

#[derive(Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub attempt_id: String,
    pub observation_id: String,
    pub enrollment_key: String,
    pub profile_id: String,
    pub payment_id: String,
    pub kind: ReversalKind,
    pub credential_fingerprint: String,
    pub state: String,
    pub review_reason: Option<String>,
    pub tip_page_id: Option<String>,
    pub generation: i32,
    pub failures: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub due_at: i64,
    pub lease_until: Option<i64>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryError {
    InvalidRequest,
    ClockUnavailable,
    Storage,
    InvalidStoredData,
    LeaseLost,
    Conflict,
}
impl From<PaymentStoreError> for RecoveryError {
    fn from(v: PaymentStoreError) -> Self {
        match v {
            PaymentStoreError::InvalidRequest => Self::InvalidRequest,
            PaymentStoreError::ClockUnavailable => Self::ClockUnavailable,
            PaymentStoreError::InvalidStoredData
            | PaymentStoreError::NotFound
            | PaymentStoreError::Unavailable => Self::InvalidStoredData,
            PaymentStoreError::Conflict => Self::Conflict,
            _ => Self::Storage,
        }
    }
}
impl From<sqlx::Error> for RecoveryError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
}
fn kind_name(kind: ReversalKind) -> &'static str {
    match kind {
        ReversalKind::Refund => "refund",
        ReversalKind::Chargeback => "chargeback",
    }
}
fn fingerprint(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub struct EnrollIntent<'a> {
    pub attempt_id: &'a str,
    pub observation_id: &'a str,
    pub kind: ReversalKind,
    pub enrollment_key: &'a str,
}
/// Enroll a verified paid source under an explicit trusted idempotency key.
pub async fn enroll(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    intent: EnrollIntent<'_>,
) -> Result<Job, RecoveryError> {
    let EnrollIntent {
        attempt_id,
        observation_id,
        kind,
        enrollment_key,
    } = intent;
    input(attempt_id, enrollment_key)?;
    if !identifier(observation_id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = context(&mut tx, attempt_id, true).await?;
    let observation =
        super::observations::rows::by_id(&mut tx, &frozen, started, observation_id).await?;
    if observation.intent.payment.status != PaymentStatus::Paid
        || frozen.snapshot().binding.profile.id != client.profile_id()
    {
        return Err(RecoveryError::InvalidRequest);
    }
    let fp = client.credential_fingerprint();
    if let Some(existing) = rows::by_key(&mut tx, attempt_id, enrollment_key).await? {
        if existing.observation_id != observation_id
            || existing.kind != kind
            || existing.credential_fingerprint != fp
        {
            return Err(RecoveryError::Conflict);
        }
        validate::source(&mut tx, &existing, &frozen, started).await?;
        tx.rollback().await?;
        return Ok(existing);
    }
    let at = now(clock)?;
    if at < observation.recorded_at {
        return Err(RecoveryError::ClockUnavailable);
    }
    let id = fresh_id(ids)?;
    let expected = Job {
        id: id.clone(),
        attempt_id: attempt_id.into(),
        observation_id: observation_id.into(),
        enrollment_key: enrollment_key.into(),
        profile_id: client.profile_id().into(),
        payment_id: observation.intent.payment.id,
        kind,
        credential_fingerprint: fp.into(),
        state: "pending".into(),
        review_reason: None,
        tip_page_id: None,
        generation: 0,
        failures: 0,
        created_at: at,
        updated_at: at,
        due_at: at,
        lease_until: None,
    };
    let changed=sqlx::query("INSERT INTO reversal_scan_jobs(id,attempt_id,observation_id,enrollment_key,profile_id,payment_id,reversal_kind,credential_fingerprint,state,generation,failures,created_at,updated_at,due_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pending',0,0,$9,$9,$9)")
        .bind(&expected.id).bind(&expected.attempt_id).bind(&expected.observation_id).bind(&expected.enrollment_key).bind(&expected.profile_id).bind(&expected.payment_id).bind(kind_name(expected.kind)).bind(&expected.credential_fingerprint).bind(at).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(RecoveryError::InvalidStoredData);
    }
    let job = rows::load(&mut tx, &id)
        .await?
        .ok_or(RecoveryError::InvalidStoredData)?;
    if job != expected {
        return Err(RecoveryError::InvalidStoredData);
    }
    if now(clock)? < at {
        return Err(RecoveryError::ClockUnavailable);
    }
    tx.commit().await?;
    Ok(job)
}
/// Read and revalidate a bounded private supervision record.
pub async fn read_job(pool: &PgPool, id: &str) -> Result<Option<Job>, RecoveryError> {
    if !identifier(id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let initial = rows::load(&mut tx, id).await?;
    let job = if let Some(j) = initial {
        let (frozen, started) = context(&mut tx, &j.attempt_id, true).await?;
        let current = rows::load(&mut tx, id)
            .await?
            .ok_or(RecoveryError::InvalidStoredData)?;
        if current.attempt_id != j.attempt_id {
            return Err(RecoveryError::InvalidStoredData);
        }
        validate::source(&mut tx, &current, &frozen, started).await?;
        validate::tip(&mut tx, &current, &frozen, started).await?;
        Some(current)
    } else {
        None
    };
    tx.rollback().await?;
    Ok(job)
}
