use super::PasswordError;
use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
use std::time::Duration;
use tokio::sync::{Semaphore, SemaphorePermit};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

const MAX_PASSWORD_BYTES: usize = 256;
const MIN_PASSWORD_CHARACTERS: usize = 15;
static PASSWORD_WORK: Semaphore = Semaphore::const_new(2);

async fn acquire_capacity(work: &Semaphore) -> Result<SemaphorePermit<'_>, PasswordError> {
    tokio::time::timeout(Duration::from_secs(2), work.acquire())
        .await
        .map_err(|_| PasswordError::Unavailable)?
        .map_err(|_| PasswordError::Unavailable)
}

pub(super) async fn hash_password(password: &str) -> Result<String, PasswordError> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(PasswordError::Denied);
    }
    let normalized = Zeroizing::new(password.nfc().collect::<String>());
    if normalized.len() > MAX_PASSWORD_BYTES || normalized.chars().count() < MIN_PASSWORD_CHARACTERS
    {
        return Err(PasswordError::Denied);
    }
    let permit = acquire_capacity(&PASSWORD_WORK).await?;
    tokio::task::spawn_blocking(move || {
        // Retain the permit even if the awaiting caller is cancelled.
        let _permit = permit;
        let mut salt = [0; 16];
        getrandom::fill(&mut salt).map_err(|_| PasswordError::Unavailable)?;
        let params = Params::new(19_456, 2, 1, Some(32)).map_err(|_| PasswordError::Unavailable)?;
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_with_salt(normalized.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| PasswordError::Unavailable)
    })
    .await
    .map_err(|_| PasswordError::Unavailable)?
}

#[cfg(test)]
#[path = "../../tests/support/customer_password_hash_unit.rs"]
mod tests;
