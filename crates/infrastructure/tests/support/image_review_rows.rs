use super::*;
include!("../../../../tests/fixtures/specification.rs");
fn bundle() -> ImageReviewBundle {
    let event = |actor: &str| DefinitionEvent {
        command: commerce_domain::catalogue_authoring::CatalogueCommand::Family {
            code: "ROOM".into(),
        },
        bom_revision: None,
        actor: actor.into(),
        source_ref: "source".into(),
        reason: "reason".into(),
    };
    ImageReviewBundle {
        sku: "SET".into(),
        target: ImageTarget {
            asset_id: "photo".into(),
            bom_revision: 1,
            specification_revision: 1,
            rights_revision: 1,
        },
        identity: event("identity"),
        bom: event("bom"),
        specification: SpecificationRecord {
            bom_revision: 1,
            draft: draft(true),
        },
        specification_actor: "spec".into(),
        registration_actor: "register".into(),
        ingestion_actor: "ingest".into(),
        rights_actor: "rights".into(),
        asset: MediaAssetDraft {
            sha256: [1; 32],
            image_type: commerce_domain::media::ImageType::Jpeg,
            width_px: 32,
            height_px: 24,
            kind: commerce_domain::media::MediaKind::OriginalPhoto,
            fixture: false,
            source_ref: "ref".into(),
        },
        rights: MediaRightsChange::Revoke,
        image: ImageReceipt {
            asset_id: "photo".into(),
            recipe: "image-v1",
            display_width: 32,
            display_height: 24,
            renditions: vec![],
            replayed: false,
        },
        review: None,
    }
}
fn row() -> StoredReview {
    StoredReview {
        sku: "SET".into(),
        asset_id: "photo".into(),
        bom_revision: 1,
        specification_revision: 1,
        rights_revision: 1,
        revision: 1,
        decision: "approved".into(),
        evidence_ref: "evidence".into(),
        qualification_ref: "qualification".into(),
        actor: "reviewer".into(),
        reason: "reason".into(),
    }
}
#[test]
fn stored_reviews_revalidate_every_target_decision_evidence_and_independent_author() {
    let bundle = bundle();
    assert_eq!(
        row().validated(&bundle).expect("valid").decision,
        ImageDecision::Approved
    );
    let mut rejected = row();
    rejected.decision = "rejected".into();
    assert_eq!(
        rejected.validated(&bundle).expect("reject").decision,
        ImageDecision::Rejected
    );
    for field in 0..18 {
        let mut invalid = row();
        match field {
            0 => invalid.sku = "OTHER".into(),
            1 => invalid.asset_id = "other".into(),
            2 => invalid.bom_revision = 2,
            3 => invalid.specification_revision = 2,
            4 => invalid.rights_revision = 2,
            5 => invalid.revision = 0,
            6 => invalid.actor = "bad/id".into(),
            7 => invalid.actor = "identity".into(),
            8 => invalid.actor = "bom".into(),
            9 => invalid.actor = "spec".into(),
            10 => invalid.actor = "register".into(),
            11 => invalid.actor = "ingest".into(),
            12 => invalid.actor = "rights".into(),
            13 => invalid.evidence_ref = " ".into(),
            14 => invalid.qualification_ref = " ".into(),
            15 => invalid.reason = " ".into(),
            17 => invalid.evidence_ref = "hidden\0ref".into(),
            _ => invalid.decision = "public".into(),
        }
        assert!(
            matches!(
                invalid.validated(&bundle),
                Err(ImageReviewError::InvalidStoredData)
            ),
            "field {field}"
        );
    }
}
