//! Purpose-separated AEAD for private guest-link custody.
use super::*;
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    hkdf,
};

const HKDF_SALT: &[u8] = b"piwas:delivery:hkdf-salt:v1";
const HKDF_INFO: &[u8] = b"piwas:guest-order-link:delivery:aes256gcm:v1";
const AAD_DOMAIN: &[u8] = b"piwas:guest-order-link:delivery:aad:v1";
const PREFIX: [u8; 4] = *b"PL01"; // Distinct from verification's PV01.
pub(super) const VERSION: i16 = 1;
struct KeyLength;
impl hkdf::KeyType for KeyLength {
    fn len(&self) -> usize {
        32
    }
}

pub(super) fn derive(input: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, LinkError> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, HKDF_SALT);
    let prk = salt.extract(input);
    let info = [HKDF_INFO];
    let output = prk.expand(&info, KeyLength)?;
    let mut derived = Zeroizing::new([0; 32]);
    output.fill(derived.as_mut())?;
    Ok(derived)
}

#[derive(Clone, Copy)]
pub(super) struct Context<'a> {
    pub basket: &'a str,
    pub order: &'a str,
    pub identity: &'a str,
    pub verifier: &'a [u8; 32],
    pub created_at: i64,
    pub expires_at: i64,
    pub key_id: &'a str,
    pub fingerprint: &'a [u8; 32],
    pub counter: i64,
}
fn aad(c: &Context<'_>) -> Result<Zeroizing<Vec<u8>>, LinkError> {
    let mut out = Zeroizing::new(Vec::new());
    for field in [
        AAD_DOMAIN,
        c.basket.as_bytes(),
        c.order.as_bytes(),
        c.identity.as_bytes(),
        c.verifier.as_slice(),
        &c.created_at.to_be_bytes(),
        &c.expires_at.to_be_bytes(),
        c.key_id.as_bytes(),
        c.fingerprint.as_slice(),
        &c.counter.to_be_bytes(),
        &VERSION.to_be_bytes(),
    ] {
        // Every V1 field is bounded to 255 bytes; keep the four-byte wire length.
        let len = u8::try_from(field.len()).map_err(|_| LinkError::Unavailable)?;
        out.extend_from_slice(&u32::from(len).to_be_bytes());
        out.extend_from_slice(field);
    }
    Ok(out)
}
fn nonce(counter: i64) -> Result<Nonce, LinkError> {
    let n = u64::try_from(counter).map_err(|_| LinkError::Unavailable)?;
    if n == 0 {
        return Err(LinkError::Unavailable);
    }
    let mut value = [0; 12];
    value[..4].copy_from_slice(&PREFIX);
    value[4..].copy_from_slice(&n.to_be_bytes());
    Ok(Nonce::assume_unique_for_key(value))
}
pub(super) fn seal(
    key: &DeliveryKey,
    c: &Context<'_>,
    token: &GuestOrderLinkToken,
    reservation: NonceReservation,
) -> Result<Vec<u8>, LinkError> {
    if reservation.verifier != *c.verifier
        || reservation.counter != c.counter
        || reservation.key_id != key.id
        || reservation.fingerprint != key.fingerprint
        || c.key_id != key.id
        || c.fingerprint != &key.fingerprint
        || token.verifier() != *c.verifier
    {
        return Err(LinkError::Unavailable);
    }
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key.derived.as_slice())?);
    let mut sealed = Zeroizing::new(token.expose_bytes().to_vec());
    cipher.seal_in_place_append_tag(
        nonce(c.counter)?,
        Aad::from(aad(c)?.as_slice()),
        &mut *sealed,
    )?;
    Ok(std::mem::take(&mut *sealed))
}

/// Authenticate the complete stored context before constructing the private bearer.
pub(super) fn open(
    key: &DeliveryKey,
    c: &Context<'_>,
    ciphertext: &[u8],
) -> Result<GuestOrderLinkToken, LinkError> {
    if c.key_id != key.id || c.fingerprint != &key.fingerprint || ciphertext.len() != 48 {
        return Err(LinkError::Unavailable);
    }
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key.derived.as_slice())?);
    let mut buffer = Zeroizing::new(ciphertext.to_vec());
    let bytes = cipher.open_in_place(
        nonce(c.counter)?,
        Aad::from(aad(c)?.as_slice()),
        &mut buffer,
    )?;
    use subtle::ConstantTimeEq;
    let bytes: &[u8; 32] = (&*bytes).try_into().map_err(|_| LinkError::Unavailable)?;
    let mut token = GuestOrderLinkToken(Zeroizing::new([0; 32]));
    token.0.copy_from_slice(bytes);
    if !bool::from(token.verifier().ct_eq(c.verifier)) {
        return Err(LinkError::Unavailable);
    }
    Ok(token)
}

#[cfg(test)]
#[path = "../../../tests/support/guest_link_delivery_crypto.rs"]
mod tests;
