use super::review::StoredReview;
use super::*;
fn draft() -> StoredDraft {
    StoredDraft {
        policy: Some(
            serde_json::json!({"currency":"EUR","promotion":{"id":"COURTYARD","revision":1,"valid_from":1000,"expires_at":2000,"countries":["NL"],"products":[{"sku":"SET-FILLED","publication_revision":1}],"minimum_eligible_gross_minor":1000,"maximum_discount_minor":500,"discount":{"kind":"fixed","minor_units":500},"evidence_ref":"synthetic"},"limits":{"total_discount_minor":10000,"total_redemptions":20,"per_customer_redemptions":null}}),
        ),
        actor: "author".into(),
        reason: "synthetic".into(),
    }
}
#[test]
fn stored_promotion_rows_reject_invalid_actors_and_reviews_bound_to_other_sources() {
    let d = draft();
    assert!(d.decode("COURTYARD", 1).is_ok());
    for actor in ["", "invalid\nactor"] {
        let mut d = draft();
        d.actor = actor.into();
        assert!(matches!(
            d.decode("COURTYARD", 1),
            Err(PolicyError::InvalidStoredData)
        ));
    }
    let review = || StoredReview {
        draft_revision: 1,
        revision: 1,
        decision: "approved".into(),
        evidence_ref: "synthetic".into(),
        qualification_ref: "synthetic".into(),
        actor: "reviewer".into(),
        reason: "synthetic".into(),
    };
    assert!(review().valid(&d, 1));
    assert!(!review().valid(&d, 2));
    for n in 0..8 {
        let mut r = review();
        match n {
            0 => r.draft_revision = 0,
            1 => r.revision = 0,
            2 => r.decision = "unknown".into(),
            3 => r.actor = "".into(),
            4 => r.actor = "author".into(),
            5 => r.evidence_ref = "\n".into(),
            6 => r.qualification_ref = "\n".into(),
            _ => r.reason = "\n".into(),
        };
        assert!(!r.valid(&d, 1), "corrupt header {n}");
    }
}
