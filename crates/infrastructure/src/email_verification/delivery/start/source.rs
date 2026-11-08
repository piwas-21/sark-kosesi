//! Exact bounded challenge and encrypted-envelope source custody.
use super::*;

#[derive(sqlx::FromRow, PartialEq, Eq)]
pub(in crate::email_verification::delivery) struct Source {
    pub(in crate::email_verification::delivery) created_at: i64,
    pub(in crate::email_verification::delivery) expires_at: i64,
    pub(in crate::email_verification::delivery) identity: Option<String>,
    pub(super) mode: String,
    pub(super) consumed_at: Option<i64>,
    pub(super) consumed_xid_empty: bool,
    pub(super) envelope_key: Option<String>,
    pub(in crate::email_verification::delivery) nonce_counter: Option<i64>,
    pub(super) fingerprint: Option<Vec<u8>>,
    pub(in crate::email_verification::delivery) ciphertext: Option<Vec<u8>>,
    pub(super) envelope_version: Option<i16>,
    pub(super) challenge_xid: Option<String>,
    pub(super) envelope_xid: Option<String>,
    pub(super) nonce_xid: Option<String>,
    pub(super) nonce_verifier: Option<Vec<u8>>,
    pub(super) reserved_at: Option<i64>,
    pub(super) registered_at: Option<i64>,
}
impl Drop for Source {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.identity.zeroize();
    }
}
impl Source {
    pub(super) fn is_consumed(&self) -> bool {
        self.consumed_at.is_some() || !self.consumed_xid_empty
    }

    /// Validate an exact stored header before looking up or writing a start.
    pub(super) fn validate(&self, key: &VerificationDeliveryKey) -> Result<i64, VerificationError> {
        if self.mode != "envelope" || self.is_consumed() {
            return Err(VerificationError::Denied);
        }
        if self.envelope_key.as_deref() != Some(key.key_id.as_str()) {
            return Err(VerificationError::Denied);
        }
        let fingerprint = self
            .fingerprint
            .as_deref()
            .ok_or(VerificationError::Unavailable)?;
        if fingerprint != key.fingerprint.as_slice() {
            return Err(VerificationError::Denied);
        }
        let counter = self.nonce_counter.ok_or(VerificationError::Unavailable)?;
        let identity = self
            .identity
            .as_deref()
            .ok_or(VerificationError::Unavailable)?;
        let normalized = zeroize::Zeroizing::new(
            normalize_email(identity).ok_or(VerificationError::Unavailable)?,
        );
        if normalized.as_str() != identity
            || self.created_at < 0
            || self.created_at.checked_add(LIFETIME) != Some(self.expires_at)
            || self.expires_at > MAX_QUOTE_UNIX_SECONDS
            || !(1..=1_000_000).contains(&counter)
        {
            return Err(VerificationError::Unavailable);
        }
        Ok(counter)
    }
}

pub(super) async fn lock_source(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
) -> Result<Source, VerificationError> {
    let claimed: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE",
    )
    .bind(verifier.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    if claimed.is_none() {
        return Err(VerificationError::Denied);
    }
    // This statement receives a fresh READ COMMITTED snapshot after any lock wait.
    sqlx::query_as("SELECT c.created_at,c.expires_at,CASE WHEN octet_length(c.email_identity)<=254 THEN c.email_identity END AS identity,c.delivery_mode AS mode,c.consumed_at,c.consumed_xid IS NULL AS consumed_xid_empty,left(e.key_id,65) AS envelope_key,e.nonce_counter,CASE WHEN octet_length(k.key_fingerprint)=32 THEN k.key_fingerprint END AS fingerprint,CASE WHEN octet_length(e.ciphertext)=48 THEN e.ciphertext END AS ciphertext,e.envelope_version,left(c.delivery_transaction_id::text,21) AS challenge_xid,left(e.transaction_id::text,21) AS envelope_xid,left(n.transaction_id::text,21) AS nonce_xid,CASE WHEN octet_length(n.verifier)=32 THEN n.verifier END AS nonce_verifier,n.reserved_at,k.registered_at FROM email_verification_challenges c LEFT JOIN email_verification_delivery_envelopes e ON e.verifier=c.verifier LEFT JOIN email_verification_delivery_keys k ON k.key_id=e.key_id LEFT JOIN email_verification_delivery_nonces n ON n.key_id=e.key_id AND n.nonce_counter=e.nonce_counter WHERE c.verifier=$1")
        .bind(verifier.as_slice()).fetch_one(&mut **tx).await.map_err(|_| VerificationError::Unavailable)
}
