use super::{LookupOperationSnapshot, PaymentStoreError, require_operation_source};

#[test]
fn locked_operation_must_match_the_validated_source_fields() {
    let operation = LookupOperationSnapshot {
        schema_version: 1,
        id: "1".repeat(32),
        profile_id: "pfl_Synthetic".into(),
        provider_id: "tr_Synthetic".into(),
        created_at: 1500,
    };
    operation.validate().expect("well-formed operation");
    for (profile, provider, time) in [
        ("pfl_Other", "tr_Synthetic", 1500),
        ("pfl_Synthetic", "tr_Other", 1500),
        ("pfl_Synthetic", "tr_Synthetic", 1501),
    ] {
        assert_eq!(
            require_operation_source(&operation, profile, provider, time),
            Err(PaymentStoreError::InvalidStoredData)
        );
    }
    assert_eq!(
        require_operation_source(&operation, "pfl_Synthetic", "tr_Synthetic", 1500),
        Ok(())
    );
}
