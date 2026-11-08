//! Isolated header controls for damaged imported start custody.
use super::*;

fn key() -> VerificationDeliveryKey {
    VerificationDeliveryKey::capture("delivery-1", [7; 32]).expect("fixture key")
}
fn source(key: &VerificationDeliveryKey) -> Source {
    Source {
        created_at: 1000,
        expires_at: 1900,
        identity: Some("owner@example.test".into()),
        mode: "envelope".into(),
        consumed_at: None,
        consumed_xid_empty: true,
        envelope_key: Some(key.key_id.clone()),
        nonce_counter: Some(1),
        fingerprint: Some(key.fingerprint.to_vec()),
        ciphertext: Some(vec![0; 48]),
        envelope_version: Some(1),
        challenge_xid: Some("11".into()),
        envelope_xid: Some("11".into()),
        nonce_xid: Some("10".into()),
        nonce_verifier: Some(vec![2; 32]),
        reserved_at: Some(999),
        registered_at: Some(998),
    }
}
fn started(key: &VerificationDeliveryKey) -> Started {
    Started {
        key_id: key.key_id.clone(),
        nonce_counter: 1,
        key_fingerprint: key.fingerprint.to_vec(),
        started_at: 1100,
        current_transaction: false,
        valid_source: true,
        transaction_id: Some("12".into()),
    }
}

#[test]
fn source_header_requires_live_mode_exact_key_and_bounded_nonce() {
    let k = key();
    assert_eq!(source(&k).validate(&k), Ok(1));
    let mut row = source(&k);
    row.mode = "direct".into();
    assert_eq!(row.validate(&k), Err(VerificationError::Denied));
    let mut row = source(&k);
    row.consumed_at = Some(1100);
    assert_eq!(row.validate(&k), Err(VerificationError::Denied));
    let mut row = source(&k);
    row.consumed_xid_empty = false;
    assert_eq!(row.validate(&k), Err(VerificationError::Denied));
    let mut row = source(&k);
    row.envelope_key = Some("wrong-key".into());
    assert_eq!(row.validate(&k), Err(VerificationError::Denied));
    let mut row = source(&k);
    row.fingerprint = None;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    let mut row = source(&k);
    row.fingerprint = Some(vec![8; 32]);
    assert_eq!(row.validate(&k), Err(VerificationError::Denied));
    let mut row = source(&k);
    row.nonce_counter = None;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    for counter in [0, 1_000_001] {
        let mut row = source(&k);
        row.nonce_counter = Some(counter);
        assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    }
}

#[test]
fn source_header_rejects_noncanonical_mailbox_and_malformed_lifetime() {
    let k = key();
    let mut row = source(&k);
    row.identity = None;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    let mut row = source(&k);
    row.identity = Some("owner@EXAMPLE.TEST".into());
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    let mut row = source(&k);
    row.created_at = -1;
    row.expires_at = 899;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    let mut row = source(&k);
    row.expires_at += 1;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    let mut row = source(&k);
    row.created_at = i64::MAX;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
    let mut row = source(&k);
    row.created_at = MAX_QUOTE_UNIX_SECONDS - LIFETIME + 1;
    row.expires_at = MAX_QUOTE_UNIX_SECONDS + 1;
    assert_eq!(row.validate(&k), Err(VerificationError::Unavailable));
}

#[test]
fn live_clock_and_historical_start_binding_fail_closed() {
    let k = key();
    let row = source(&k);
    assert_eq!(check_live(&row, 999), Err(VerificationError::Unavailable));
    assert_eq!(check_live(&row, 1900), Err(VerificationError::Denied));
    assert_eq!(check_live(&row, 1899), Ok(()));
    assert_eq!(validate_start(&started(&k), &k, 1, &row, 1100), Ok(()));
    let mut value = started(&k);
    value.key_id = "other".into();
    assert_eq!(
        validate_start(&value, &k, 1, &row, 1100),
        Err(VerificationError::Unavailable)
    );
    let mut value = started(&k);
    value.nonce_counter = 2;
    assert_eq!(
        validate_start(&value, &k, 1, &row, 1100),
        Err(VerificationError::Unavailable)
    );
    let mut value = started(&k);
    value.key_fingerprint = vec![1; 32];
    assert_eq!(
        validate_start(&value, &k, 1, &row, 1100),
        Err(VerificationError::Unavailable)
    );
    for at in [999, 1900] {
        let mut value = started(&k);
        value.started_at = at;
        assert_eq!(
            validate_start(&value, &k, 1, &row, 2000),
            Err(VerificationError::Unavailable)
        );
    }
    assert_eq!(
        validate_start(&started(&k), &k, 1, &row, 1099),
        Err(VerificationError::Unavailable)
    );
    let mut value = started(&k);
    value.valid_source = false;
    assert_eq!(
        validate_start(&value, &k, 1, &row, 1100),
        Err(VerificationError::Unavailable)
    );
}
