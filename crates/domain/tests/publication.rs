use commerce_domain::{factual_review::FactTarget, image_review::ImageTarget, publication::*};
fn selection() -> ApprovedSelection {
    ApprovedSelection {
        facts: FactTarget {
            bom_revision: 1,
            specification_revision: 2,
            price_revision: 3,
        },
        factual_review_revision: 1,
        image: ImageTarget {
            asset_id: "photo".into(),
            bom_revision: 1,
            specification_revision: 2,
            rights_revision: 1,
        },
        image_review_revision: 1,
        translation_revision: 1,
        translation_review_revision: 1,
        source_review_revision: None,
        family_slug: "a-family-1".into(),
    }
}
#[test]
fn closed_selection_requires_matching_exact_sources_positive_decisions_and_safe_family_path() {
    assert!(selection().valid());
    assert!(PublicationChoice::Withdraw {}.valid());
    assert!(
        PublicationChoice::Activate {
            selection: selection()
        }
        .valid()
    );
    for field in 0..10 {
        let mut invalid = selection();
        match field {
            0 => invalid.facts.bom_revision = 0,
            1 => invalid.image.asset_id = "../image".into(),
            2 => invalid.image.bom_revision = 2,
            3 => invalid.image.specification_revision = 1,
            4 => invalid.factual_review_revision = 0,
            5 => invalid.image_review_revision = 0,
            6 => invalid.translation_revision = 0,
            7 => invalid.translation_review_revision = 0,
            8 => invalid.source_review_revision = Some(0),
            _ => invalid.family_slug = "../family".into(),
        };
        assert!(!invalid.valid(), "field {field}");
    }
    let mut translated = selection();
    translated.source_review_revision = Some(2);
    assert!(translated.valid());
    for path in [
        "",
        "-room",
        "room-",
        "room--one",
        "Room",
        "room/name",
        "room?name",
        "røøm",
    ] {
        assert!(!valid_slug(path), "{path}");
    }
    assert!(!valid_slug(&"a".repeat(101)));
    assert!(valid_slug(&"a".repeat(100)));
    assert!(valid_slug("1-room"));
}
