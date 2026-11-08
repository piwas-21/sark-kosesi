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
