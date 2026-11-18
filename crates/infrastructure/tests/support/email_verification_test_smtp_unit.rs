use super::*;

#[test]
fn authenticated_identity_must_be_the_reserved_recipient() {
    assert!(require_synthetic_recipient(RECIPIENT).is_ok());
    assert_eq!(
        require_synthetic_recipient("other@example.test"),
        Err(VerificationError::Unavailable)
    );
}

#[test]
fn proof_encoding_preserves_leading_zeroes_and_all_nibbles() {
    let mut bearer = [0_u8; 32];
    bearer[..4].copy_from_slice(&[0, 15, 16, 255]);
    let mut message = Zeroizing::new(String::new());
    append_proof_hex(&mut message, &bearer);
    assert_eq!(message.len(), 64);
    assert!(message.starts_with("000f10ff"));
    assert!(message[8..].bytes().all(|byte| byte == b'0'));
}
