//! Replaceable OS adapters; business/use-case code receives explicit clocks and IDs.
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SystemClock;
pub struct SecureIdentifiers;
fn seconds(time: SystemTime) -> Result<u64, ContextError> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ContextError::ClockUnavailable)
}
impl Clock for SystemClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        seconds(SystemTime::now())
    }
}
impl Identifiers for SecureIdentifiers {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| ContextError::EntropyUnavailable)?;
        Ok(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn epoch_conversion_is_checked_and_deterministic() {
        assert_eq!(seconds(UNIX_EPOCH + Duration::from_secs(120)), Ok(120));
        assert_eq!(
            seconds(
                UNIX_EPOCH
                    .checked_sub(Duration::from_secs(1))
                    .expect("fixture")
            ),
            Err(ContextError::ClockUnavailable)
        );
        assert!(SystemClock.unix_seconds().is_ok());
    }
    #[test]
    fn os_identifiers_produce_distinct_opaque_values() {
        let first = SecureIdentifiers.next().expect("OS entropy available");
        let second = SecureIdentifiers.next().expect("OS entropy available");
        assert_ne!(first, second);
    }
}
