use super::*;

// Literal SHA-256 values are independently calculated over the stated UTF-8 bytes.
const BLOCKED: [u8; 32] = [
    0x34, 0xab, 0x20, 0x08, 0x7a, 0x69, 0x33, 0xd7, 0x76, 0x8e, 0xdf, 0x56, 0xb6, 0xf1, 0x26, 0xf6,
    0x92, 0x3c, 0x21, 0x1b, 0x52, 0xb2, 0x04, 0x61, 0x48, 0x6f, 0x0a, 0xd6, 0x79, 0xfe, 0xb3, 0xe2,
];
const INTEGRITY: [u8; 32] = [
    0x2e, 0xdd, 0x3d, 0xf2, 0x3a, 0x69, 0x4f, 0x55, 0x33, 0xbc, 0xc2, 0x1a, 0x16, 0x67, 0x08, 0x50,
    0x50, 0x41, 0x8c, 0x03, 0xf6, 0xe9, 0x7f, 0xaa, 0xa5, 0x0d, 0xe6, 0xb5, 0x01, 0x67, 0xa1, 0x24,
];
fn manifest() -> BlocklistManifest {
    BlocklistManifest {
        version: "v1".into(),
        provenance: "fixture".into(),
        qualification_ref: "test-review".into(),
        valid_from: 100,
        valid_until: 200,
        digests: vec![BLOCKED],
    }
}
#[test]
fn exact_nfc_membership_preserves_spaces_and_fails_closed_at_window_edges() {
    assert_eq!(manifest().integrity_digest(), INTEGRITY);
    let list = PasswordBlocklist::new(manifest(), INTEGRITY).expect("valid literal manifest");
    assert_eq!(
        list.check("café password is blocked", 100),
        Err(PasswordError::Denied)
    );
    assert_eq!(
        list.check("cafe\u{301} password is blocked", 199),
        Err(PasswordError::Denied)
    );
    assert_eq!(list.check(" café password is blocked ", 150), Ok(()));
    assert_eq!(list.check("another lengthy password", 150), Ok(()));
    assert_eq!(
        list.check("another lengthy password", 99),
        Err(PasswordError::Unavailable)
    );
    assert_eq!(
        list.check("another lengthy password", 200),
        Err(PasswordError::Unavailable)
    );
    assert_eq!(list.check("short", 150), Err(PasswordError::Denied));
}
#[test]
fn corrupted_or_unbounded_manifests_are_not_accepted() {
    assert!(PasswordBlocklist::new(manifest(), [0; 32]).is_err());
    let mut m = manifest();
    m.digests[0][0] ^= 1;
    assert!(PasswordBlocklist::new(m, INTEGRITY).is_err());
    let mut m = manifest();
    m.qualification_ref.clear();
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.provenance = " source ".into();
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.version = "bad\nversion".into();
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.valid_until = m.valid_from;
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.valid_until = m.valid_from + MAX_WINDOW_SECONDS + 1;
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    for (valid_from, valid_until) in [(-1, 200), (100, MAX_TIME + 1), (100, i64::MIN)] {
        let mut m = manifest();
        m.valid_from = valid_from;
        m.valid_until = valid_until;
        let digest = m.integrity_digest();
        assert!(matches!(
            PasswordBlocklist::new(m, digest),
            Err(PasswordError::Unavailable)
        ));
    }
    let mut m = manifest();
    m.digests.clear();
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.digests.push(BLOCKED);
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.digests = vec![[2; 32], [1; 32]];
    let d = m.integrity_digest();
    assert!(PasswordBlocklist::new(m, d).is_err());
    let mut m = manifest();
    m.digests = vec![[0; 32]; MAX_ENTRIES + 1];
    assert!(PasswordBlocklist::new(m, [0; 32]).is_err());
}

// These bytes and SHA-256 digests were constructed independently of the manifest encoder.
fn canonical_fixture() -> Vec<u8> {
    let mut bytes = b"piwas:password-blocklist:snapshot:v1\0".to_vec();
    for field in [b"v1".as_slice(), b"fixture", b"test-review"] {
        bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
        bytes.extend_from_slice(field);
    }
    bytes.extend_from_slice(&100_i64.to_be_bytes());
    bytes.extend_from_slice(&200_i64.to_be_bytes());
    bytes.extend_from_slice(&2_u64.to_be_bytes());
    bytes.extend_from_slice(&[1; 32]);
    bytes.extend_from_slice(&[2; 32]);
    bytes
}
const CANONICAL_DIGEST: [u8; 32] = [
    0x92, 0x8a, 0x5b, 0x00, 0x56, 0x7c, 0x22, 0xff, 0xea, 0x98, 0xb4, 0xf5, 0x24, 0xae, 0x1f, 0x9c,
    0x63, 0xfe, 0x2e, 0x6e, 0x4a, 0xd6, 0x54, 0xcc, 0x57, 0xdb, 0xac, 0x58, 0x1b, 0x3f, 0x2d, 0xb5,
];

#[test]
fn canonical_decoder_accepts_only_complete_integrity_checked_snapshots() {
    let bytes = canonical_fixture();
    assert_eq!(bytes.len(), 169);
    let list = PasswordBlocklist::from_canonical_bytes(&bytes, CANONICAL_DIGEST)
        .expect("independently hashed canonical snapshot");
    assert_eq!(list.0.digests, vec![[1; 32], [2; 32]]);
    assert_eq!(list.0.version, "v1");
    assert!(PasswordBlocklist::from_canonical_bytes(&bytes, [0; 32]).is_err());
    for end in 0..bytes.len() {
        assert!(
            PasswordBlocklist::from_canonical_bytes(&bytes[..end], CANONICAL_DIGEST).is_err(),
            "truncated at {end}"
        );
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(PasswordBlocklist::from_canonical_bytes(&extra, CANONICAL_DIGEST).is_err());
    let mut invalid_domain = bytes.clone();
    invalid_domain[0] ^= 1;
    assert!(PasswordBlocklist::from_canonical_bytes(&invalid_domain, CANONICAL_DIGEST).is_err());
}

#[test]
fn canonical_decoder_bounds_lengths_counts_and_utf8_before_allocation() {
    let bytes = canonical_fixture();
    let metadata_length = DOMAIN.len();
    for length in [0_u64, 241, u64::MAX] {
        let mut invalid = bytes.clone();
        invalid[metadata_length..metadata_length + 8].copy_from_slice(&length.to_be_bytes());
        assert!(PasswordBlocklist::from_canonical_bytes(&invalid, CANONICAL_DIGEST).is_err());
    }
    let mut invalid = bytes.clone();
    invalid[metadata_length + 8] = 0xff;
    assert!(PasswordBlocklist::from_canonical_bytes(&invalid, CANONICAL_DIGEST).is_err());
    let count_offset = bytes.len() - 64 - 8;
    for count in [0_u64, 1, 3, 1_000_001, u64::MAX] {
        let mut invalid = bytes.clone();
        invalid[count_offset..count_offset + 8].copy_from_slice(&count.to_be_bytes());
        assert!(PasswordBlocklist::from_canonical_bytes(&invalid, CANONICAL_DIGEST).is_err());
    }
}

#[test]
fn canonical_decoder_rejects_unsorted_and_duplicate_entries_even_with_matching_digest() {
    let bytes = canonical_fixture();
    let unsorted_digest = [
        0xa8, 0xc7, 0x3f, 0x77, 0x0e, 0x8e, 0x67, 0xde, 0x53, 0x7b, 0x6a, 0x37, 0xfb, 0x49, 0x51,
        0x0c, 0x6c, 0x90, 0xac, 0x84, 0xa0, 0x00, 0x37, 0x3d, 0x8f, 0x0a, 0xe6, 0x20, 0x51, 0x20,
        0xe2, 0x54,
    ];
    let duplicate_digest = [
        0x2b, 0xf4, 0xf1, 0x21, 0xec, 0x31, 0x2d, 0x7c, 0xf4, 0x89, 0xce, 0x8f, 0x81, 0xbf, 0x93,
        0xce, 0x6e, 0x58, 0x38, 0xed, 0xd0, 0x7b, 0xd5, 0x40, 0x55, 0x1b, 0x4c, 0x80, 0xa1, 0x94,
        0x13, 0xae,
    ];
    let mut unsorted = bytes.clone();
    let start = bytes.len() - 64;
    unsorted[start..start + 32].fill(2);
    unsorted[start + 32..].fill(1);
    assert!(PasswordBlocklist::from_canonical_bytes(&unsorted, unsorted_digest).is_err());
    let mut duplicate = bytes;
    duplicate[start + 32..].fill(1);
    assert!(PasswordBlocklist::from_canonical_bytes(&duplicate, duplicate_digest).is_err());
}
