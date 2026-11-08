use super::*;

fn open(
    key: &VerificationDeliveryKey,
    context: &Context<'_>,
    sealed: &[u8],
) -> Result<Vec<u8>, VerificationError> {
    let cipher = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, key.bytes.as_slice())
            .map_err(|_| VerificationError::Unavailable)?,
    );
    let mut copy = Zeroizing::new(sealed.to_vec());
    let opened = cipher
        .open_in_place(
            nonce(context.counter)?,
            Aad::from(aad(context)?.as_slice()),
            &mut copy,
        )
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(opened.to_vec())
}

fn reservation(key: &VerificationDeliveryKey, context: &Context<'_>) -> NonceReservation {
    NonceReservation {
        counter: context.counter,
        verifier: *context.verifier,
        key_id: key.key_id.clone(),
        fingerprint: key.fingerprint,
    }
}

#[test]
fn encrypted_bearer_round_trips_only_with_exact_authenticated_context() {
    let key = VerificationDeliveryKey::capture("mail-1", [3; 32]).expect("key");
    let other = VerificationDeliveryKey::capture("mail-2", [4; 32]).expect("other key");
    let token = VerificationToken::from_bytes([9; 32]);
    let verifier = token.verifier();
    let different_verifier = [7; 32];
    let original = Context {
        purpose: PURPOSE,
        identity: "person@example.test",
        verifier: &verifier,
        created_at: 100,
        expires_at: 1000,
        key_id: "mail-1",
        counter: 1,
        version: VERSION,
    };
    let sealed = seal(&key, &original, &token, reservation(&key, &original)).expect("seal");
    assert_eq!(sealed.len(), 48);
    assert_ne!(&sealed[..32], token.expose_bytes());
    assert_eq!(
        open(&key, &original, &sealed)
            .expect("authentic")
            .as_slice(),
        token.expose_bytes().as_slice()
    );
    let changed = [
        Context {
            purpose: "other-purpose",
            ..original
        },
        Context {
            identity: "other@example.test",
            ..original
        },
        Context {
            verifier: &different_verifier,
            ..original
        },
        Context {
            created_at: 101,
            ..original
        },
        Context {
            expires_at: 1001,
            ..original
        },
        Context {
            key_id: "mail-2",
            ..original
        },
        Context {
            counter: 2,
            ..original
        },
        Context {
            version: 2,
            ..original
        },
    ];
    for context in changed {
        assert!(open(&key, &context, &sealed).is_err());
    }
    assert!(open(&other, &original, &sealed).is_err());
    let mut wrong_counter = reservation(&key, &original);
    wrong_counter.counter += 1;
    assert!(seal(&key, &original, &token, wrong_counter).is_err());
    let mut wrong_verifier = reservation(&key, &original);
    wrong_verifier.verifier = [7; 32];
    assert!(seal(&key, &original, &token, wrong_verifier).is_err());
    let mut wrong_key_id = reservation(&key, &original);
    wrong_key_id.key_id = "other".into();
    assert!(seal(&key, &original, &token, wrong_key_id).is_err());
    let mut wrong_key = reservation(&key, &original);
    wrong_key.fingerprint = other.fingerprint;
    assert!(seal(&key, &original, &token, wrong_key).is_err());
    let other_token = VerificationToken::from_bytes([8; 32]);
    assert!(seal(&key, &original, &other_token, reservation(&key, &original)).is_err());
    let future = Context {
        version: 2,
        ..original
    };
    assert!(seal(&key, &future, &token, reservation(&key, &future)).is_err());
    let mut damaged = sealed.clone();
    damaged[0] ^= 1;
    assert!(open(&key, &original, &damaged).is_err());
    let mut damaged_tag = sealed.clone();
    damaged_tag[47] ^= 1;
    assert!(open(&key, &original, &damaged_tag).is_err());
    let second = seal(
        &key,
        &Context {
            counter: 2,
            ..original
        },
        &token,
        reservation(
            &key,
            &Context {
                counter: 2,
                ..original
            },
        ),
    )
    .expect("next nonce");
    assert_ne!(sealed, second);
    assert!(
        seal(
            &key,
            &Context {
                counter: 0,
                ..original
            },
            &token,
            reservation(
                &key,
                &Context {
                    counter: 0,
                    ..original
                }
            )
        )
        .is_err()
    );
}

#[test]
fn delivery_key_rejects_zero_and_invalid_identifier() {
    assert!(VerificationDeliveryKey::capture("mail", [0; 32]).is_err());
    for id in ["", "_mail", "mail.key", "bad key", &"x".repeat(65)] {
        assert!(VerificationDeliveryKey::capture(id, [5; 32]).is_err());
    }
    assert!(VerificationDeliveryKey::capture("a_Valid-1", [5; 32]).is_ok());
}
