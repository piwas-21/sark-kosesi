use commerce_domain::factual_review::{FactDecision, FactTarget};
#[test]
fn decisions_and_all_exact_source_versions_are_explicit() {
    let target = FactTarget {
        bom_revision: 1,
        specification_revision: 1,
        price_revision: i32::MAX,
    };
    assert!(target.valid());
    for invalid in [
        FactTarget {
            bom_revision: 0,
            ..target
        },
        FactTarget {
            specification_revision: -1,
            ..target
        },
        FactTarget {
            price_revision: 0,
            ..target
        },
    ] {
        assert!(!invalid.valid());
    }
    assert_eq!(FactDecision::Approved.code(), "approved");
    assert_eq!(FactDecision::Rejected.code(), "rejected");
}
