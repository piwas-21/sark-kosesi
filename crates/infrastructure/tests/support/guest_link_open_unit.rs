use super::*;
use commerce_application::request_context::ContextError;

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
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
