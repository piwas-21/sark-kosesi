//! Error classes remain independent of the financial evidence being quarantined.
use super::*;

#[test]
fn quarantine_inspection_accepts_one_thousand_closed_operations_and_rejects_overflow_or_malformed_ids()
 {
    let boundary = vec!["a".repeat(32); 1000];
    assert_eq!(
        bounded_operations(boundary.clone()).expect("200 intakes times five generations"),
        boundary
    );
    assert_eq!(
        bounded_operations(vec!["a".repeat(32); 1001]).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        bounded_operations(vec!["malformed".into()]).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert!(
        bounded_operations(Vec::new())
            .expect("empty inspection")
            .is_empty()
    );
}

#[test]
fn quarantine_classifies_only_deterministic_missing_financial_history_as_invalid_custody() {
    for error in [
        PaymentStoreError::NotFound,
        PaymentStoreError::Conflict,
        PaymentStoreError::Unavailable,
    ] {
        assert_eq!(proof_error(error), PaymentStoreError::InvalidStoredData);
    }
    for (error, expected) in [
        (PaymentStoreError::Database, PaymentStoreError::Database),
        (
            PaymentStoreError::ClockUnavailable,
            PaymentStoreError::ClockUnavailable,
        ),
        (
            PaymentStoreError::InvalidRequest,
            PaymentStoreError::InvalidRequest,
        ),
        (
            PaymentStoreError::InvalidStoredData,
            PaymentStoreError::InvalidStoredData,
        ),
    ] {
        assert_eq!(proof_error(error), expected);
    }
}
