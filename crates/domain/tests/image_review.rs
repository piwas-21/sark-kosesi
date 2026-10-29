use commerce_domain::image_review::{ImageDecision, ImageTarget};
#[test]
fn closed_image_targets_require_positive_exact_revisions_and_safe_asset_identity() {
    let target = ImageTarget {
        asset_id: "photo-1".into(),
        bom_revision: 1,
        specification_revision: 2,
        rights_revision: 3,
    };
    assert!(target.valid());
    for field in 0..4 {
        let mut invalid = target.clone();
        match field {
            0 => invalid.asset_id = "../photo".into(),
            1 => invalid.bom_revision = 0,
            2 => invalid.specification_revision = -1,
            _ => invalid.rights_revision = 0,
        }
        assert!(!invalid.valid());
    }
    assert_eq!(ImageDecision::Approved.code(), "approved");
    assert_eq!(ImageDecision::Rejected.code(), "rejected");
}
