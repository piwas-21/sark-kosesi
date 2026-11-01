//! Typed private source header controls with paired healthy boundaries.
use super::*;
fn header() -> Header {
    Header {
        id: "a".repeat(32),
        profile_id: "pfl_BOUND".into(),
        provider_id: "tr_BOUND".into(),
        received_at: 1500,
        exact: true,
    }
}
#[test]
fn global_header_boundary_is_two_hundred_and_every_header_is_typed_and_exact() {
    assert!(validate(Vec::new()).is_ok());
    assert!(validate((0..200).map(|_| header()).collect()).is_ok());
    assert_eq!(
        validate((0..201).map(|_| header()).collect()).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    for field in ["id", "profile", "provider", "time", "exact"] {
        let mut bad = header();
        match field {
            "id" => bad.id = "x".repeat(32),
            "profile" => bad.profile_id = "pfl_".into(),
            "provider" => bad.provider_id = "tr_".into(),
            "time" => bad.received_at = -1,
            _ => bad.exact = false,
        }
        assert_eq!(
            validate(vec![header(), bad]).err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{field}"
        );
    }
    let mut boundary = header();
    boundary.received_at = 9_007_199_254_740_991;
    assert!(validate(vec![boundary]).is_ok());
    assert!(validate(vec![header()]).is_ok());
}
