use super::*;

const ID: &str = "0123456789abcdef0123456789abcdef";
const OTHER_ID: &str = "abcdef0123456789abcdef0123456789";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn row() -> Row {
    Row {
        id: ID.into(),
        case_id: ID.into(),
        case_digest: DIGEST.into(),
        order_id: ID.into(),
        attempt_id: ID.into(),
        recognition_id: ID.into(),
        actor_id: "staff-1".into(),
        effect_key: ID.into(),
        generation: 1,
        reason: "Stock was counted".into(),
        recorded_at: 42,
        outcome: "still_scarce".into(),
        allocation_id: None,
        valid: true,
    }
}
fn invalid(row: Row) {
    assert!(matches!(
        row.receipt(),
        Err(LateStockError::InvalidStoredData)
    ));
}

#[test]
fn decoder_rejects_invalid_proof_and_identity_fields() {
    assert_eq!(
        row().receipt().unwrap().outcome,
        DecisionOutcome::StillScarce
    );
    let mut value = row();
    value.valid = false;
    invalid(value);
    for field in 0..5 {
        let mut value = row();
        match field {
            0 => value.id = "bad".into(),
            1 => value.case_id = "bad".into(),
            2 => value.order_id = "bad".into(),
            3 => value.attempt_id = "bad".into(),
            _ => value.recognition_id = "bad".into(),
        }
        invalid(value);
    }
    let mut value = row();
    value.actor_id = "invalid actor".into();
    invalid(value);
    let mut value = row();
    value.case_digest = "A".repeat(64);
    invalid(value);
    let mut value = row();
    value.effect_key = "g".repeat(32);
    invalid(value);
    for generation in [0, 4] {
        let mut value = row();
        value.generation = generation;
        invalid(value);
    }
    for reason in ["", " reason", "reason ", &"x".repeat(241)] {
        let mut value = row();
        value.reason = reason.into();
        invalid(value);
    }
    let mut value = row();
    value.reason = "x".repeat(240);
    value.generation = 3;
    assert!(value.receipt().is_ok());
}

#[test]
fn decoder_binds_exact_outcome_and_allocation() {
    let mut allocated = row();
    allocated.outcome = "allocated".into();
    allocated.allocation_id = Some(OTHER_ID.into());
    let receipt = allocated.receipt().unwrap();
    assert_eq!(
        receipt.outcome,
        DecisionOutcome::Allocated {
            allocation_id: OTHER_ID.into()
        }
    );
    assert!(!receipt.replayed);
    for (outcome, allocation) in [
        ("allocated", None),
        ("allocated", Some("bad")),
        ("still_scarce", Some(OTHER_ID)),
        ("other", None),
    ] {
        let mut value = row();
        value.outcome = outcome.into();
        value.allocation_id = allocation.map(str::to_owned);
        invalid(value);
    }
}

#[test]
fn history_requires_same_case_contiguous_generations_and_terminal_allocation() {
    let first = row().receipt().unwrap();
    let mut second = row().receipt().unwrap();
    second.generation = 2;
    let mut third = row().receipt().unwrap();
    third.generation = 3;
    third.outcome = DecisionOutcome::Allocated {
        allocation_id: OTHER_ID.into(),
    };
    assert!(validate_sequence(&[], ID).is_ok());
    assert!(validate_sequence(&[first, second, third], ID).is_ok());
    let first = row().receipt().unwrap();
    assert_eq!(
        validate_sequence(&[first], OTHER_ID),
        Err(LateStockError::InvalidStoredData)
    );
    let mut gap = row().receipt().unwrap();
    gap.generation = 2;
    assert_eq!(
        validate_sequence(&[gap], ID),
        Err(LateStockError::InvalidStoredData)
    );
    let mut allocated = row().receipt().unwrap();
    allocated.outcome = DecisionOutcome::Allocated {
        allocation_id: OTHER_ID.into(),
    };
    let mut later = row().receipt().unwrap();
    later.generation = 2;
    assert_eq!(
        validate_sequence(&[allocated, later], ID),
        Err(LateStockError::InvalidStoredData)
    );
}
