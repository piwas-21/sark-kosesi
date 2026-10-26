use commerce_application::request_context::{Clock, ContextError, Identifiers, RequestContext};
struct FixedClock(Result<u64, ContextError>);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        self.0
    }
}
struct FixedIdentifiers(Result<[u8; 16], ContextError>);
impl Identifiers for FixedIdentifiers {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        self.0
    }
}
#[test]
fn context_has_exact_injected_clock_and_opaque_id() {
    let context = RequestContext::capture(
        &FixedClock(Ok(1_700_000_000)),
        &FixedIdentifiers(Ok([0xab; 16])),
    )
    .expect("fixture");
    assert_eq!(context.unix_seconds, 1_700_000_000);
    assert_eq!(context.id.to_string(), "abababababababababababababababab");
}
#[test]
fn unavailable_sources_are_explicit_errors() {
    assert_eq!(
        RequestContext::capture(
            &FixedClock(Err(ContextError::ClockUnavailable)),
            &FixedIdentifiers(Ok([0; 16]))
        ),
        Err(ContextError::ClockUnavailable)
    );
    assert_eq!(
        RequestContext::capture(
            &FixedClock(Ok(0)),
            &FixedIdentifiers(Err(ContextError::EntropyUnavailable))
        ),
        Err(ContextError::EntropyUnavailable)
    );
}
