use super::*;
fn row() -> StoredDraft {
    StoredDraft {sku:"DEMO-FILLED".into(),locale:"nl".into(),revision:1,specification_revision:1,source_revision:None,payload:serde_json::json!({"title":"fixture","description":"fixture","materials":"fixture","care":"fixture","safety":"fixture","assembly":"fixture"}).to_string(),actor:"author".into(),reason:"fixture".into()}
}
fn review_row() -> StoredReview {
    StoredReview {
        sku: "DEMO-FILLED".into(),
        locale: "nl".into(),
        translation_revision: 1,
        revision: 1,
        decision: "approved".into(),
        actor: "reader".into(),
        evidence_ref: "fixture".into(),
        qualification_ref: "fixture".into(),
        reason: "fixture".into(),
    }
}
#[test]
fn stored_review_requires_an_independent_actor_and_explicit_known_decision() {
    assert_eq!(
        decode_review(review_row(), "author")
            .expect("valid history")
            .decision,
        TranslationDecision::Approved
    );
    let mut rejected = review_row();
    rejected.decision = "rejected".into();
    assert_eq!(
        decode_review(rejected, "author")
            .expect("rejection history")
            .decision,
        TranslationDecision::Rejected
    );
    for case in 0..10 {
        let mut r = review_row();
        match case {
            0 => r.revision = 0,
            1 => r.actor = "/".into(),
            2 => r.actor = "author".into(),
            3 => r.evidence_ref = " ".into(),
            4 => r.qualification_ref = " ".into(),
            5 => r.reason = " ".into(),
            6 => r.decision = "auto-approved".into(),
            7 => r.evidence_ref = "x".repeat(501),
            8 => r.qualification_ref = "x".repeat(501),
            9 => r.reason = "nul\0ref".into(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                decode_review(r, "author"),
                Err(TranslationError::InvalidStoredData)
            ),
            "stored review boundary {case}"
        );
    }
}
#[test]
fn stored_identity_sources_and_closed_content_are_revalidated() {
    assert_eq!(
        decode(row()).expect("valid immutable record").locale,
        Locale::Nl
    );
    for case in 0..12 {
        let mut r = row();
        match case {
            0 => r.locale = "ku".into(),
            1 => r.sku = "".into(),
            2 => r.revision = 0,
            3 => r.specification_revision = 0,
            4 => r.source_revision = Some(1),
            5 => {
                r.locale = "de".into();
                r.source_revision = None;
            }
            6 => {
                r.locale = "de".into();
                r.source_revision = Some(0);
            }
            7 => r.actor = "/".into(),
            8 => r.reason = " ".into(),
            9 => r.payload = "x".repeat(131073),
            10 => r.payload = "{}".into(),
            11 => r.payload = r.payload.replace("\"care\":\"fixture\"", "\"care\":\" \""),
            _ => unreachable!(),
        }
        assert!(
            matches!(decode(r), Err(TranslationError::InvalidStoredData)),
            "stored boundary {case}"
        );
    }
    let mut translated = row();
    translated.locale = "ar".into();
    translated.source_revision = Some(1);
    assert_eq!(
        decode(translated).expect("explicit source").source_revision,
        Some(1)
    );
}
