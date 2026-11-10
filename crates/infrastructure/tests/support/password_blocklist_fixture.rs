//! Synthetic test material; not a reviewed or operational password corpus.
use commerce_infrastructure::customer_passwords::{BlocklistManifest, PasswordBlocklist};

pub fn synthetic() -> PasswordBlocklist {
    let manifest = BlocklistManifest {
        version: "synthetic-test-v1".into(),
        provenance: "local test fixture".into(),
        qualification_ref: "synthetic-only".into(),
        valid_from: 0,
        valid_until: 7_776_000,
        digests: vec![[0; 32]],
    };
    let integrity = manifest.integrity_digest();
    PasswordBlocklist::new(manifest, integrity).expect("synthetic fixture")
}
