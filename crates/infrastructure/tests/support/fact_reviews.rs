use super::*;
include!("../../../../tests/fixtures/specification.rs");
fn bundle() -> FactBundle {
    let event = |actor: &str| DefinitionEvent {
        command: commerce_domain::catalogue_authoring::CatalogueCommand::Family {
            code: "ROOM".into(),
        },
        bom_revision: None,
        actor: actor.into(),
        source_ref: "source".into(),
        reason: "reason".into(),
    };
    FactBundle {
        sku: "SET".into(),
        target: FactTarget {
            bom_revision: 1,
            specification_revision: 1,
            price_revision: 1,
        },
        identity: event("identity-author"),
        bom: event("bom-author"),
        specification: SpecificationRecord {
            bom_revision: 1,
            draft: draft(true),
        },
        specification_actor: "spec-author".into(),
        specification_reason: "reason".into(),
        price: Money::eur(12345).expect("price"),
        price_actor: "price-author".into(),
        price_evidence_ref: "ref".into(),
        price_reason: "reason".into(),
        review: None,
    }
}
fn row() -> StoredReview {
    StoredReview {
        sku: "SET".into(),
        bom_revision: 1,
        specification_revision: 1,
        price_revision: 1,
        revision: 1,
        decision: "approved".into(),
        evidence_ref: "evidence".into(),
        qualification_ref: "qualification".into(),
        actor: "reviewer".into(),
        reason: "reason".into(),
    }
}
#[test]
fn stored_reviews_bind_full_target_independent_authors_and_closed_decisions() {
    let bundle = bundle();
    assert_eq!(
        row().validated(&bundle).expect("valid").decision,
        FactDecision::Approved
    );
    let mut rejected = row();
    rejected.decision = "rejected".into();
    assert_eq!(
        rejected.validated(&bundle).expect("rejected").decision,
        FactDecision::Rejected
    );
    for field in 0..13 {
        let mut invalid = row();
        match field {
            0 => invalid.sku = "OTHER".into(),
            1 => invalid.bom_revision = 2,
            2 => invalid.revision = 0,
            3 => invalid.actor = "bad/id".into(),
            4 => invalid.actor = "identity-author".into(),
            5 => invalid.actor = "bom-author".into(),
            6 => invalid.actor = "spec-author".into(),
            7 => invalid.actor = "price-author".into(),
            8 => invalid.evidence_ref = " ".into(),
            9 => invalid.qualification_ref = " ".into(),
            10 => invalid.reason = " ".into(),
            11 => invalid.decision = "active".into(),
            12 => invalid.evidence_ref = "hidden\0ref".into(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                invalid.validated(&bundle),
                Err(FactError::InvalidStoredData)
            ),
            "field {field}"
        );
    }
}
