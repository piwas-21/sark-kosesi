use commerce_application::request_context::{Clock, ContextError, ObservedClock};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
struct Source {
    at: AtomicU64,
    failed: AtomicBool,
}
impl Clock for Source {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.failed.load(Ordering::SeqCst) {
            Err(ContextError::ClockUnavailable)
        } else {
            Ok(self.at.load(Ordering::SeqCst))
        }
    }
}
#[test]
fn shared_use_case_observations_reject_reverse_time_and_never_cache_through_source_failure() {
    let source = Arc::new(Source {
        at: AtomicU64::new(1100),
        failed: AtomicBool::new(false),
    });
    let clock = Arc::new(ObservedClock::new(source.clone(), 1100));
    let composed = clock.clone();
    assert_eq!(clock.unix_seconds(), Ok(1100));
    source.at.store(1101, Ordering::SeqCst);
    assert_eq!(composed.unix_seconds(), Ok(1101));
    source.at.store(1100, Ordering::SeqCst);
    assert_eq!(clock.unix_seconds(), Err(ContextError::ClockUnavailable));
    source.at.store(1102, Ordering::SeqCst);
    assert_eq!(composed.unix_seconds(), Ok(1102));
    source.failed.store(true, Ordering::SeqCst);
    assert_eq!(clock.unix_seconds(), Err(ContextError::ClockUnavailable));
    source.failed.store(false, Ordering::SeqCst);
    source.at.store(1101, Ordering::SeqCst);
    assert_eq!(composed.unix_seconds(), Err(ContextError::ClockUnavailable));
    let higher = ObservedClock::new(source, 1103);
    assert_eq!(higher.unix_seconds(), Err(ContextError::ClockUnavailable));
}
