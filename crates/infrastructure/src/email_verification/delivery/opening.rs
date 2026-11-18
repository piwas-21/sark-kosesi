//! Trusted in-process opening; it never performs network I/O or verifies an account.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    Opened,
    AlreadyStarted,
}

/// Commit one start, then locally authenticate its live envelope and call `accept` once.
///
/// The callback receives borrowed secret data after all database locks have been released.
/// It is a trusted synchronous consumer: this API does not send mail or prove delivery.
/// Challenge state can change immediately after the final local liveness fence.
pub async fn claim_and_open(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &VerificationDeliveryKey,
    accept: impl FnOnce(&str, &[u8; 32]),
) -> Result<OpenOutcome, VerificationError> {
    let observed = ObservedClock::new(clock, 0);
    match open(pool, verifier, &observed, key).await? {
        Some(opened) => {
            accept(&opened.identity, &opened.bearer);
            Ok(OpenOutcome::Opened)
        }
        None => Ok(OpenOutcome::AlreadyStarted),
    }
}

/// A closed synthetic transport result may be recorded only by this fresh opening.
/// No transaction is held while the trusted callback runs. This does not send mail.
pub async fn claim_open_and_record(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &VerificationDeliveryKey,
    accept: impl FnOnce(&str, &[u8; 32]) -> super::outcome::TransportOutcome,
) -> Result<OpenOutcome, VerificationError> {
    let observed = ObservedClock::new(clock, 0);
    match open(pool, verifier, &observed, key).await? {
        Some(opened) => {
            let outcome = accept(&opened.identity, &opened.bearer);
            let FreshOpening {
                identity,
                bearer,
                permit,
            } = opened;
            drop((identity, bearer));
            super::outcome::record(pool, &permit, key, &observed, outcome).await?;
            Ok(OpenOutcome::Opened)
        }
        None => Ok(OpenOutcome::AlreadyStarted),
    }
}

pub(super) struct FreshOpening {
    pub(super) identity: Zeroizing<String>,
    pub(super) bearer: Zeroizing<[u8; 32]>,
    pub(super) permit: start::Permit,
}

pub(super) async fn open(
    pool: &PgPool,
    verifier: &[u8; 32],
    observed: &dyn Clock,
    key: &VerificationDeliveryKey,
) -> Result<Option<FreshOpening>, VerificationError> {
    if let start::InternalClaim::New(permit) =
        start::claim_with_permit(pool, verifier, observed, key).await?
    {
        let mut tx = begin(pool).await?;
        let source = start::source_for_open(&mut tx, &permit, key, observed).await?;
        let identity = source
            .identity
            .as_deref()
            .ok_or(VerificationError::Unavailable)?;
        let ciphertext = source
            .ciphertext
            .as_deref()
            .ok_or(VerificationError::Unavailable)?;
        let counter = source.nonce_counter.ok_or(VerificationError::Unavailable)?;
        let context = crypto::Context {
            purpose: PURPOSE,
            identity,
            verifier: &permit.verifier,
            created_at: source.created_at,
            expires_at: source.expires_at,
            key_id: &key.key_id,
            counter,
            version: crypto::VERSION,
        };
        let bearer = crypto::open(key, &context, ciphertext)?;
        let identity = Zeroizing::new(identity.to_owned());
        start::check_live(&source, bounded_now(observed)?)?;
        tx.commit()
            .await
            .map_err(|_| VerificationError::Unavailable)?;
        start::check_live(&source, bounded_now(observed)?)?;
        Ok(Some(FreshOpening {
            identity,
            bearer,
            permit: *permit,
        }))
    } else {
        Ok(None)
    }
}
