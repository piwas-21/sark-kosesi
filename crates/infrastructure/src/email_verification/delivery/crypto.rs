//! Authenticated envelope bytes; no production opening or delivery operation.
use super::*;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use zeroize::Zeroizing;

pub(super) const VERSION: i16 = 1;
const NONCE_PREFIX: [u8; 4] = *b"PV01";
const AAD_DOMAIN: &[u8] = b"piwas:verification-delivery:aad:v1";

#[derive(Clone, Copy)]
pub(super) struct Context<'a> {
    pub purpose: &'a str,
    pub identity: &'a str,
    pub verifier: &'a [u8; 32],
    pub created_at: i64,
    pub expires_at: i64,
    pub key_id: &'a str,
    pub counter: i64,
    pub version: i16,
}

fn nonce(counter: i64) -> Result<Nonce, VerificationError> {
    let number = u64::try_from(counter).map_err(|_| VerificationError::Unavailable)?;
    if number == 0 {
        return Err(VerificationError::Unavailable);
    }
    let mut bytes = [0_u8; 12];
    bytes[..4].copy_from_slice(&NONCE_PREFIX);
    bytes[4..].copy_from_slice(&number.to_be_bytes());
    Ok(Nonce::assume_unique_for_key(bytes))
}

fn aad(context: &Context<'_>) -> Result<Zeroizing<Vec<u8>>, VerificationError> {
    let mut result = Zeroizing::new(Vec::new());
    for field in [
        AAD_DOMAIN,
        context.purpose.as_bytes(),
        context.identity.as_bytes(),
        context.verifier.as_slice(),
        &context.created_at.to_be_bytes(),
        &context.expires_at.to_be_bytes(),
        &context.version.to_be_bytes(),
        context.key_id.as_bytes(),
        &context.counter.to_be_bytes(),
    ] {
        let len = u32::try_from(field.len()).map_err(|_| VerificationError::Unavailable)?;
        result.extend_from_slice(&len.to_be_bytes());
        result.extend_from_slice(field);
    }
    Ok(result)
}

pub(super) fn seal(
    key: &VerificationDeliveryKey,
    context: &Context<'_>,
    token: &VerificationToken,
    reservation: NonceReservation,
) -> Result<Vec<u8>, VerificationError> {
    if (
        reservation.counter,
        &reservation.verifier,
        reservation.key_id.as_str(),
        &reservation.fingerprint,
    ) != (
        context.counter,
        context.verifier,
        key.key_id.as_str(),
        &key.fingerprint,
    ) || context.key_id != key.key_id
        || context.version != VERSION
        || token.verifier() != reservation.verifier
    {
        return Err(VerificationError::Unavailable);
    }
    let cipher = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, key.bytes.as_slice())
            .map_err(|_| VerificationError::Unavailable)?,
    );
    let mut sealed = Zeroizing::new(token.expose_bytes().to_vec());
    cipher
        .seal_in_place_append_tag(
            nonce(context.counter)?,
            Aad::from(aad(context)?.as_slice()),
            &mut *sealed,
        )
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(std::mem::take(&mut *sealed))
}

#[cfg(test)]
#[path = "../../../tests/support/email_verification_delivery_crypto.rs"]
mod tests;
