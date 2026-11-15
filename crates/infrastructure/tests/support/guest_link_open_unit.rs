use super::*;
use commerce_application::request_context::ContextError;

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}

fn proof() -> Proof {
    Proof {
        order_id: "a".repeat(32),
        basket_id: "b".repeat(32),
        created_at: 100,
        expires_at: 1000,
        consumed_at: None,
        consumed_account_id: None,
        unconsumed_xid: true,
    }
}

#[test]
fn decoded_proof_requires_exact_scope_and_bounded_lifetime() {
    assert!(proof().validate().is_ok());
    let mut value = proof();
    value.order_id = "invalid".into();
    assert_eq!(value.validate(), Err(LinkError::Denied));
    let mut value = proof();
    value.basket_id = "invalid".into();
    assert_eq!(value.validate(), Err(LinkError::Denied));
    let mut value = proof();
    value.consumed_at = Some(100);
    assert_eq!(value.validate(), Err(LinkError::Denied));
    let mut value = proof();
    value.consumed_account_id = Some(1);
    assert_eq!(value.validate(), Err(LinkError::Denied));
    let mut value = proof();
    value.unconsumed_xid = false;
    assert_eq!(value.validate(), Err(LinkError::Denied));
    let mut value = proof();
    value.created_at = -1;
    assert_eq!(value.validate(), Err(LinkError::Unavailable));
    let mut value = proof();
    value.expires_at = 1001;
    assert_eq!(value.validate(), Err(LinkError::Unavailable));
    let mut value = proof();
    value.created_at = MAX_QUOTE_UNIX_SECONDS - LIFETIME + 1;
    value.expires_at = MAX_QUOTE_UNIX_SECONDS + 1;
    assert_eq!(value.validate(), Err(LinkError::Unavailable));
    let mut value = proof();
    value.created_at = i64::MAX;
    assert_eq!(value.validate(), Err(LinkError::Unavailable));
}

#[test]
fn decoded_envelope_requires_key_exact_nonce_transaction_and_time() {
    let (key, source, _) = fixture();
    let verifier = [3; 32];
    assert!(
        source
            .envelope
            .validate(&key, &verifier, source.created_at)
            .is_ok()
    );
    let (_, mut source, _) = fixture();
    source.envelope.key_id = "other".into();
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Denied)
    );
    let (_, mut source, _) = fixture();
    source.envelope.fingerprint[0] ^= 1;
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Denied)
    );
    for counter in [0, 1_000_001] {
        let (_, mut source, _) = fixture();
        source.envelope.nonce_counter = counter;
        assert_eq!(
            source.envelope.validate(&key, &verifier, 100),
            Err(LinkError::Unavailable)
        );
    }
    let (_, mut source, _) = fixture();
    source.envelope.version = 2;
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
    let (_, mut source, _) = fixture();
    source.envelope.ciphertext.pop();
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
    let (_, mut source, _) = fixture();
    source.envelope.nonce_verifier[0] ^= 1;
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
    let (_, mut source, _) = fixture();
    source.envelope.proof_xid = "3".into();
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
    let (_, mut source, _) = fixture();
    source.envelope.nonce_xid = source.envelope.envelope_xid.clone();
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
    let (_, mut source, _) = fixture();
    source.envelope.registered_at = 100;
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
    let (_, mut source, _) = fixture();
    source.envelope.reserved_at = 101;
    assert_eq!(
        source.envelope.validate(&key, &verifier, 100),
        Err(LinkError::Unavailable)
    );
}

#[test]
fn fresh_start_and_reopened_source_must_match_exact_custody() {
    let (_, original, start) = fixture();
    assert_eq!(check_new_start(&start, 100), Ok(()));
    let (_, _, mut changed) = fixture();
    changed.current_transaction = false;
    assert_eq!(check_new_start(&changed, 100), Err(LinkError::Unavailable));
    assert_eq!(check_new_start(&start, 101), Err(LinkError::Unavailable));

    let (_, same, _) = fixture();
    assert_eq!(check_same_source(&same, &original), Ok(()));
    let (_, mut changed, _) = fixture();
    changed.envelope.ciphertext[0] ^= 1;
    assert_eq!(
        check_same_source(&changed, &original),
        Err(LinkError::Unavailable)
    );

    let (_, _, mut saved) = fixture();
    assert_eq!(check_same_start(&saved, &start), Ok(()));
    saved.transaction_id = "4".into();
    assert_eq!(
        check_same_start(&saved, &start),
        Err(LinkError::Unavailable)
    );
    saved.transaction_id = start.transaction_id.clone();
    saved.started_at += 1;
    assert_eq!(
        check_same_start(&saved, &start),
        Err(LinkError::Unavailable)
    );
}

fn fixture() -> (DeliveryKey, Source, Started) {
    let key = DeliveryKey::capture("open-key", [7; 32]).expect("key");
    let source = Source {
        basket: "a".repeat(32),
        order: "b".repeat(32),
        identity: Zeroizing::new("buyer@example.test".into()),
        created_at: 100,
        expires_at: 1000,
        envelope: Envelope {
            key_id: key.id.clone(),
            nonce_counter: 1,
            fingerprint: key.fingerprint.to_vec(),
            ciphertext: vec![0; 48],
            version: 1,
            proof_xid: "1".into(),
            envelope_xid: "1".into(),
            nonce_xid: "2".into(),
            nonce_verifier: vec![3; 32],
            reserved_at: 99,
            registered_at: 98,
        },
    };
    let started = Started {
        key_id: key.id.clone(),
        nonce_counter: 1,
        fingerprint: key.fingerprint.to_vec(),
        started_at: 100,
        transaction_id: "3".into(),
        current_transaction: true,
        valid_source: true,
    };
    (key, source, started)
}

#[test]
fn start_custody_checks_each_identity_time_and_valid_source() {
    let (key, source, mut started) = fixture();
    assert!(check_start(&started, &source, &key, &At(100)).is_ok());
    started.key_id = "different".into();
    assert!(check_start(&started, &source, &key, &At(100)).is_err());
    started.key_id = key.id.clone();
    started.nonce_counter = 2;
    assert!(check_start(&started, &source, &key, &At(100)).is_err());
    started.nonce_counter = 1;
    started.fingerprint[0] ^= 1;
    assert!(check_start(&started, &source, &key, &At(100)).is_err());
    started.fingerprint = key.fingerprint.to_vec();
    started.valid_source = false;
    assert!(check_start(&started, &source, &key, &At(100)).is_err());
    started.valid_source = true;
    for at in [99, 1000, 101] {
        started.started_at = at;
        assert!(check_start(&started, &source, &key, &At(100)).is_err());
    }
    started.started_at = 100;
    assert!(matches!(source.live(&At(99)), Err(LinkError::Unavailable)));
    assert!(matches!(source.live(&At(1000)), Err(LinkError::Denied)));
    assert!(source.live(&At(999)).is_ok());
    assert!(check_start(&started, &source, &key, &At(u64::MAX)).is_err());
}
