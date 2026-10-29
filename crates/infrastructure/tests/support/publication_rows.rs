use super::*;
fn selection() -> ApprovedSelection {
    ApprovedSelection {
        facts: commerce_domain::factual_review::FactTarget {
            bom_revision: 1,
            specification_revision: 1,
            price_revision: 1,
        },
        factual_review_revision: 1,
        image: commerce_domain::image_review::ImageTarget {
            asset_id: "photo".into(),
            bom_revision: 1,
            specification_revision: 1,
            rights_revision: 1,
        },
        image_review_revision: 1,
        translation_revision: 1,
        translation_review_revision: 1,
        source_review_revision: None,
        family_slug: "room".into(),
    }
}
fn row() -> Stored {
    Stored {
        sku: "SET".into(),
        locale: "nl".into(),
        revision: 1,
        choice: r#"{"action":"withdraw"}"#.into(),
        evidence_ref: "evidence".into(),
        actor: "publisher".into(),
        reason: "reason".into(),
        evaluated_at: 1100,
    }
}
#[test]
fn stored_history_has_exact_identity_bounded_audit_closed_choices_and_locale_sources() {
    assert_eq!(
        row().decode("SET", Locale::Nl).expect("withdraw").choice,
        PublicationChoice::Withdraw {}
    );
    let mut active = row();
    active.choice = serde_json::to_string(&PublicationChoice::Activate {
        selection: selection(),
    })
    .expect("encode");
    assert!(active.decode("SET", Locale::Nl).is_ok());
    let mut translated = selection();
    translated.source_review_revision = Some(1);
    let mut active = row();
    active.locale = "de".into();
    active.choice = serde_json::to_string(&PublicationChoice::Activate {
        selection: translated,
    })
    .expect("encode");
    assert!(active.decode("SET", Locale::De).is_ok());
    for field in 0..13 {
        let mut invalid = row();
        match field {
            0 => invalid.sku = "OTHER".into(),
            1 => invalid.locale = "fr".into(),
            2 => invalid.revision = 0,
            3 => invalid.choice = "x".repeat(8193),
            4 => invalid.evaluated_at = -1,
            5 => invalid.actor = "bad/id".into(),
            6 => invalid.evidence_ref = " ".into(),
            7 => invalid.reason = " ".into(),
            8 => invalid.reason = "hidden\0reason".into(),
            9 => invalid.choice = "{}".into(),
            10 => invalid.choice = r#"{"action":"withdraw","actor":"owner"}"#.into(),
            11 => {
                let mut s = selection();
                s.source_review_revision = Some(1);
                invalid.choice =
                    serde_json::to_string(&PublicationChoice::Activate { selection: s })
                        .expect("encode");
            }
            _ => {
                let mut s = selection();
                s.family_slug = "../bad".into();
                invalid.choice =
                    serde_json::to_string(&PublicationChoice::Activate { selection: s })
                        .expect("encode");
            }
        }
        assert!(
            matches!(
                invalid.decode("SET", Locale::Nl),
                Err(PublicationError::InvalidStoredData)
            ),
            "field {field}"
        );
    }
}
