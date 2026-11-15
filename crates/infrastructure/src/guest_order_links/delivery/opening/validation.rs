//! Validation of decoded proof and encrypted envelope custody.
use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct Proof {
    pub(super) order_id: String,
    pub(super) basket_id: String,
    pub(super) created_at: i64,
    pub(super) expires_at: i64,
    pub(super) consumed_at: Option<i64>,
    pub(super) consumed_account_id: Option<i64>,
    pub(super) unconsumed_xid: bool,
}
impl Proof {
    pub(super) fn validate(&self) -> Result<(), LinkError> {
        if !valid_hex(&self.order_id, 32)
            || !valid_hex(&self.basket_id, 32)
            || self.consumed_at.is_some()
            || self.consumed_account_id.is_some()
            || !self.unconsumed_xid
        {
            return Err(LinkError::Denied);
        }
        if self.created_at < 0
            || self.created_at.checked_add(LIFETIME) != Some(self.expires_at)
            || self.expires_at > MAX_QUOTE_UNIX_SECONDS
        {
            return Err(LinkError::Unavailable);
        }
        Ok(())
    }
}

#[derive(sqlx::FromRow, PartialEq, Eq)]
pub(super) struct Envelope {
    pub(super) key_id: String,
    pub(super) nonce_counter: i64,
    pub(super) fingerprint: Vec<u8>,
    pub(super) ciphertext: Vec<u8>,
    pub(super) version: i16,
    pub(super) proof_xid: String,
    pub(super) envelope_xid: String,
    pub(super) nonce_xid: String,
    pub(super) nonce_verifier: Vec<u8>,
    pub(super) reserved_at: i64,
    pub(super) registered_at: i64,
}
impl Envelope {
    pub(super) fn validate(
        &self,
        key: &DeliveryKey,
        verifier: &[u8; 32],
        created_at: i64,
    ) -> Result<(), LinkError> {
        if self.key_id != key.id || self.fingerprint.as_slice() != key.fingerprint {
            return Err(LinkError::Denied);
        }
        if !(1..=1_000_000).contains(&self.nonce_counter)
            || self.version != crypto::VERSION
            || self.ciphertext.len() != 48
            || self.nonce_verifier.as_slice() != verifier
            || self.proof_xid != self.envelope_xid
            || self.nonce_xid == self.envelope_xid
            || self.registered_at > self.reserved_at
            || self.reserved_at > created_at
        {
            return Err(LinkError::Unavailable);
        }
        Ok(())
    }
}
