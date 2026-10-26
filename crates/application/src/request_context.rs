//! Explicit inputs for deterministic request diagnostics, without logging request data.
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextError {
    ClockUnavailable,
    EntropyUnavailable,
}

pub trait Clock: Send + Sync {
    fn unix_seconds(&self) -> Result<u64, ContextError>;
}
pub trait Identifiers: Send + Sync {
    fn next(&self) -> Result<[u8; 16], ContextError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrelationId([u8; 16]);
impl fmt::Display for CorrelationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:032x}", u128::from_be_bytes(self.0))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestContext {
    pub id: CorrelationId,
    pub unix_seconds: u64,
}
impl RequestContext {
    pub fn capture(clock: &dyn Clock, identifiers: &dyn Identifiers) -> Result<Self, ContextError> {
        let unix_seconds = clock.unix_seconds()?;
        let id = CorrelationId(identifiers.next()?);
        Ok(Self { id, unix_seconds })
    }
}
