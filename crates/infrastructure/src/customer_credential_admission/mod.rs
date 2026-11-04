//! Private, durable fixed-window admission for credential operations.
//! Callers must obtain admission before undertaking the protected operation.
mod custody;
mod state;

use commerce_application::request_context::Clock;
use commerce_domain::{checkout_quotes::MAX_QUOTE_UNIX_SECONDS, email_identity::normalize_email};
use ring::hmac;
use sqlx::PgPool;
use zeroize::Zeroizing;

use self::state::Scope;

const KEY_CHECK_DOMAIN: &[u8] = b"piwas:credential-admission:key-check:v1\0";
const TARGET_DOMAIN: &[u8] = b"piwas:credential-admission:target:v1\0";
const WINDOW: i64 = 60;

#[derive(Clone, Copy)]
#[repr(i16)]
pub enum CredentialPurpose {
    Login = 1,
    EmailVerificationIssue = 2,
    EmailVerificationConsume = 3,
    PasswordProofIssue = 4,
    PasswordReplace = 5,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AdmissionLimits {
    global: i32,
    target: i32,
}
impl AdmissionLimits {
    pub fn new(global: i32, target: i32) -> Option<Self> {
        ((1..=1000).contains(&global) && (1..=100).contains(&target) && target <= global)
            .then_some(Self { global, target })
    }
}

/// Secret admission key. Its bytes are zeroized when dropped.
pub struct AdmissionKey {
    version: i32,
    bytes: Zeroizing<[u8; 32]>,
}
impl AdmissionKey {
    pub fn new(version: i32, key: [u8; 32]) -> Option<Self> {
        let bytes = Zeroizing::new(key);
        if version < 1 || bytes.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(Self { version, bytes })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Limited,
    Unavailable,
}

fn time(clock: &dyn Clock) -> Result<i64, AdmissionError> {
    clock
        .unix_seconds()
        .ok()
        .and_then(|v| i64::try_from(v).ok())
        .filter(|v| *v <= MAX_QUOTE_UNIX_SECONDS)
        .ok_or(AdmissionError::Unavailable)
}

fn tag(key: &hmac::Key, domain: &[u8], purpose: CredentialPurpose, input: &[u8]) -> hmac::Tag {
    let mut context = hmac::Context::with_key(key);
    context.update(domain);
    context.update(&(purpose as i16).to_be_bytes());
    context.update(input);
    context.sign()
}

/// Commit one quota claim independently of the subsequent protected operation.
pub async fn admit(
    pool: &PgPool,
    purpose: CredentialPurpose,
    email: &str,
    key: &AdmissionKey,
    limits: AdmissionLimits,
    clock: &dyn Clock,
) -> Result<(), AdmissionError> {
    let started = time(clock)?;
    let mac_key = hmac::Key::new(hmac::HMAC_SHA256, key.bytes.as_ref());
    let check = tag(&mac_key, KEY_CHECK_DOMAIN, purpose, b"");
    let normalized = normalize_email(email);
    let mut tx = super::customer_sessions::begin(pool)
        .await
        .map_err(|_| AdmissionError::Unavailable)?;
    let scope = sqlx::query_as::<_, Scope>(
        "SELECT window_start,last_seen_at,key_version,key_check,global_limit,target_limit,admitted \
         FROM customer_credential_admission_scopes WHERE purpose=$1 FOR UPDATE",
    )
    .bind(purpose as i16)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| AdmissionError::Unavailable)?
    .ok_or(AdmissionError::Unavailable)?;
    let at = time(clock)?;
    if at < started {
        return Err(AdmissionError::Unavailable);
    }
    let window = at - at % WINDOW;
    let mut target_mac = hmac::Context::with_key(&mac_key);
    target_mac.update(TARGET_DOMAIN);
    target_mac.update(&(purpose as i16).to_be_bytes());
    target_mac.update(&window.to_be_bytes());
    match normalized.as_deref() {
        Some(mailbox) => {
            target_mac.update(&[1]);
            target_mac.update(mailbox.as_bytes());
        }
        None => target_mac.update(&[0]),
    }
    let digest = target_mac.sign();
    let existing = scope.validate()?;
    let same_window = if let Some(ref previous) = existing {
        if at < previous.last_seen_at
            || window < previous.window_start
            || key.version < previous.key_version
        {
            return Err(AdmissionError::Unavailable);
        }
        if key.version == previous.key_version {
            let mut message = Vec::from(KEY_CHECK_DOMAIN);
            message.extend_from_slice(&(purpose as i16).to_be_bytes());
            if hmac::verify(&mac_key, &message, &previous.key_check).is_err() {
                return Err(AdmissionError::Unavailable);
            }
        }
        if window == previous.window_start {
            if key.version != previous.key_version || limits != previous.limits {
                return Err(AdmissionError::Unavailable);
            }
            true
        } else {
            false
        }
    } else {
        false
    };
    if !same_window {
        sqlx::query("DELETE FROM customer_credential_admission_targets WHERE purpose=$1")
            .bind(purpose as i16)
            .execute(&mut *tx)
            .await
            .map_err(|_| AdmissionError::Unavailable)?;
        custody::require_empty_targets(&mut tx, purpose).await?;
    }
    let total = if same_window { scope.admitted } else { 0 };
    let target = sqlx::query_scalar::<_, i32>(
        "SELECT admitted FROM customer_credential_admission_targets WHERE purpose=$1 AND target_digest=$2 FOR UPDATE",
    ).bind(purpose as i16).bind(digest.as_ref()).fetch_optional(&mut *tx).await
        .map_err(|_| AdmissionError::Unavailable)?;
    let target_count = target.unwrap_or(0);
    if target.is_some()
        && (target_count < 1 || target_count > limits.target || target_count > total)
    {
        return Err(AdmissionError::Unavailable);
    }
    if total >= limits.global || target_count >= limits.target {
        return Err(AdmissionError::Limited);
    }
    let next_total = total.checked_add(1).ok_or(AdmissionError::Unavailable)?;
    let next_target = target_count
        .checked_add(1)
        .ok_or(AdmissionError::Unavailable)?;
    sqlx::query("INSERT INTO customer_credential_admission_targets(purpose,target_digest,admitted) VALUES($1,$2,$3) \
        ON CONFLICT (purpose,target_digest) DO UPDATE SET admitted=EXCLUDED.admitted")
        .bind(purpose as i16).bind(digest.as_ref()).bind(next_target).execute(&mut *tx).await
        .map_err(|_| AdmissionError::Unavailable)?;
    sqlx::query("UPDATE customer_credential_admission_scopes SET window_start=$2,last_seen_at=$3,key_version=$4, \
        key_check=$5,global_limit=$6,target_limit=$7,admitted=$8 WHERE purpose=$1")
        .bind(purpose as i16).bind(window).bind(at).bind(key.version).bind(check.as_ref())
        .bind(limits.global).bind(limits.target).bind(next_total).execute(&mut *tx).await
        .map_err(|_| AdmissionError::Unavailable)?;
    let mut written = custody::Expected {
        purpose,
        window,
        last_seen: at,
        key_version: key.version,
        key_check: check.as_ref(),
        limits,
        total: next_total,
        digest: digest.as_ref(),
        target: next_target,
    };
    written.require_stored(&mut tx).await?;
    let finished = time(clock)?;
    if finished < at || finished - finished % WINDOW != window {
        return Err(AdmissionError::Unavailable);
    }
    // Retain the post-write observation as the durable high-water mark. A later
    // check can fail closed, but cannot persist every future clock observation.
    let before_commit = if finished > at {
        sqlx::query(
            "UPDATE customer_credential_admission_scopes SET last_seen_at=$2 WHERE purpose=$1",
        )
        .bind(purpose as i16)
        .bind(finished)
        .execute(&mut *tx)
        .await
        .map_err(|_| AdmissionError::Unavailable)?;
        written.last_seen = finished;
        written.require_stored(&mut tx).await?;
        let checked = time(clock)?;
        if checked < finished || checked - checked % WINDOW != window {
            return Err(AdmissionError::Unavailable);
        }
        checked
    } else {
        finished
    };
    tx.commit().await.map_err(|_| AdmissionError::Unavailable)?;
    // A commit can wait across a boundary. Deny a stale grant, retaining its
    // committed debit rather than refunding or replaying it.
    let committed = time(clock)?;
    if committed < before_commit || committed - committed % WINDOW != window {
        return Err(AdmissionError::Unavailable);
    }
    Ok(())
}
