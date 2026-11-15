use super::*;

fn reservation(key: &DeliveryKey, c: &Context<'_>) -> NonceReservation {
    NonceReservation {
        counter: c.counter,
        verifier: *c.verifier,
        key_id: key.id.clone(),
        fingerprint: key.fingerprint,
    }
}
fn open(key: &DeliveryKey, c: &Context<'_>, bytes: &[u8]) -> Result<Vec<u8>, LinkError> {
    super::open(key, c, bytes).map(|token| token.expose_bytes().to_vec())
}
#[test]
fn guest_order_link_delivery_purpose_derived_key_matches_independent_hkdf_vector_and_context_is_authenticated()
 {
    let key = DeliveryKey::capture("fixture-key", [7; 32]).expect("key");
    let expected = [
        0x2b, 0x71, 0x6f, 0xcf, 0xd4, 0x52, 0x3f, 0xc3, 0x8b, 0x21, 0xdd, 0x1b, 0xec, 0xde, 0xba,
        0x8a, 0x4f, 0x11, 0xb7, 0xdf, 0x1d, 0xce, 0xaa, 0x1a, 0xc5, 0xcf, 0x8c, 0x8a, 0xe1, 0xa1,
        0x51, 0x81,
    ];
    assert_eq!(key.derived.as_slice(), expected);
    assert_ne!(key.derived.as_slice(), &[7; 32]);
    let token = GuestOrderLinkToken::from_bytes([9; 32]);
    let verifier = token.verifier();
    let context = Context {
        basket: "basket",
        order: "order",
        identity: "buyer@example.test",
        verifier: &verifier,
        created_at: 1100,
        expires_at: 2000,
        key_id: &key.id,
        fingerprint: &key.fingerprint,
        counter: 1,
    };
    let sealed = seal(&key, &context, &token, reservation(&key, &context)).expect("seal");
    assert_eq!(sealed.len(), 48);
    assert_ne!(&sealed[..32], token.expose_bytes());
    assert_eq!(
        open(&key, &context, &sealed).expect("open"),
        token.expose_bytes()
    );
    // The narrow mailbox policy permits at most 254 ASCII bytes; the framing
    // format independently rejects a field that cannot fit its V1 bound.
    let maximum_email = format!(
        "{}@{}.{}.{}",
        "a".repeat(64),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    assert_eq!(maximum_email.len(), 254);
    assert!(commerce_domain::email_identity::normalize_email(&maximum_email).is_some());
    let maximum = Context {
        identity: &maximum_email,
        ..context
    };
    let max_sealed =
        seal(&key, &maximum, &token, reservation(&key, &maximum)).expect("maximum mailbox");
    assert_eq!(
        open(&key, &maximum, &max_sealed).expect("open maximum mailbox"),
        token.expose_bytes()
    );
    let too_long = "x".repeat(256);
    let oversized = Context {
        identity: &too_long,
        ..context
    };
    assert!(aad(&oversized).is_err());
    assert!(seal(&key, &oversized, &token, reservation(&key, &oversized)).is_err());
    let other_verifier = [4; 32];
    let other_fingerprint = [5; 32];
    for changed in [
        Context {
            basket: "other",
            ..context
        },
        Context {
            order: "other",
            ..context
        },
        Context {
            identity: "other@example.test",
            ..context
        },
        Context {
            verifier: &other_verifier,
            ..context
        },
        Context {
            created_at: 1101,
            ..context
        },
        Context {
            expires_at: 2001,
            ..context
        },
        Context {
            key_id: "other",
            ..context
        },
        Context {
            fingerprint: &other_fingerprint,
            ..context
        },
        Context {
            counter: 2,
            ..context
        },
    ] {
        assert!(open(&key, &changed, &sealed).is_err());
    }
    let mut damaged = sealed.clone();
    damaged[0] ^= 1;
    assert_eq!(open(&key, &context, &damaged), Err(LinkError::Unavailable));
    let other = DeliveryKey::capture("other", [8; 32]).expect("other");
    assert!(open(&other, &context, &sealed).is_err());
    let first_nonce = nonce(1).expect("first nonce");
    assert_eq!(&first_nonce.as_ref()[..4], b"PL01");
    assert_ne!(&first_nonce.as_ref()[..4], b"PV01");
    assert_eq!(&first_nonce.as_ref()[4..], &1_u64.to_be_bytes());
    assert!(nonce(0).is_err());
    assert!(nonce(-1).is_err());
    let zero_counter = Context {
        counter: 0,
        ..context
    };
    assert!(
        seal(
            &key,
            &zero_counter,
            &token,
            reservation(&key, &zero_counter)
        )
        .is_err()
    );
    let mut wrong = reservation(&key, &context);
    wrong.counter += 1;
    assert!(seal(&key, &context, &token, wrong).is_err());
    let mut wrong = reservation(&key, &context);
    wrong.verifier = other_verifier;
    assert!(seal(&key, &context, &token, wrong).is_err());
    let mut wrong = reservation(&key, &context);
    wrong.key_id = "other".into();
    assert!(seal(&key, &context, &token, wrong).is_err());
    let mut wrong = reservation(&key, &context);
    wrong.fingerprint = other_fingerprint;
    assert!(seal(&key, &context, &token, wrong).is_err());
    let changed = Context {
        key_id: "other",
        ..context
    };
    assert!(seal(&key, &changed, &token, reservation(&key, &changed)).is_err());
    let changed = Context {
        fingerprint: &other_fingerprint,
        ..context
    };
    assert!(seal(&key, &changed, &token, reservation(&key, &changed)).is_err());
    let wrong_token = GuestOrderLinkToken::from_bytes([8; 32]);
    assert!(seal(&key, &context, &wrong_token, reservation(&key, &context)).is_err());
}
