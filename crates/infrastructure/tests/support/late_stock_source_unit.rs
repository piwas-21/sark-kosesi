//! Closed captured/current tuples preserve exact references without current-state equality.
use super::*;
use commerce_domain::order_stock::{Member, Quantity};
fn literal(state: OriginalState) -> OriginalMember {
    OriginalMember {
        member: Member {
            hold_id: "1".repeat(32),
            sku: "SET-COVERS".into(),
            revision: 1,
            units: 1,
            components: vec![Quantity {
                component_code: "SEAT-COVER".into(),
                quantity: 2,
            }],
        },
        state,
        pairs: vec![PairReference {
            component_code: "SEAT-COVER".into(),
            reserve_sequence: 10,
            reserve_digest: "a".repeat(64),
            terminal_sequence: if state == OriginalState::Active {
                None
            } else {
                Some(20)
            },
            terminal_digest: if state == OriginalState::Active {
                None
            } else {
                Some("b".repeat(64))
            },
        }],
    }
}
#[test]
fn historical_exact_terminal_references_and_source_membership_are_independently_closed() {
    let captured = literal(OriginalState::Expired);
    assert_eq!(
        historical(
            std::slice::from_ref(&captured),
            std::slice::from_ref(&captured)
        ),
        Ok(())
    );
    let changes: [fn(&mut OriginalMember); 8] = [
        |m| m.member.sku = "OTHER-COVERS".into(),
        |m| m.pairs.clear(),
        |m| m.pairs[0].component_code = "OTHER-COVER".into(),
        |m| m.pairs[0].reserve_sequence = 11,
        |m| m.pairs[0].reserve_digest = "c".repeat(64),
        |m| m.state = OriginalState::Released,
        |m| m.pairs[0].terminal_sequence = Some(21),
        |m| m.pairs[0].terminal_digest = Some("c".repeat(64)),
    ];
    for change in changes {
        let mut current = captured.clone();
        change(&mut current);
        assert_eq!(
            historical(std::slice::from_ref(&captured), &[current]),
            Err(LateStockError::InvalidStoredData)
        );
        assert_eq!(
            historical(
                std::slice::from_ref(&captured),
                std::slice::from_ref(&captured)
            ),
            Ok(())
        );
    }
    assert_eq!(
        historical(std::slice::from_ref(&captured), &[]),
        Err(LateStockError::InvalidStoredData)
    );
    let mut invalid = captured.clone();
    invalid.pairs.clear();
    assert_eq!(
        historical(&[invalid], std::slice::from_ref(&captured)),
        Err(LateStockError::InvalidStoredData)
    );
}
#[test]
fn historical_active_assessment_allows_a_valid_later_terminal_without_capturing_its_new_reference()
{
    let active = literal(OriginalState::Active);
    for state in [
        OriginalState::Active,
        OriginalState::Expired,
        OriginalState::Released,
        OriginalState::Consumed,
    ] {
        let current = literal(state);
        assert_eq!(
            historical(std::slice::from_ref(&active), &[current]),
            Ok(())
        );
    }
    let mut wrong = literal(OriginalState::Expired);
    wrong.pairs[0].reserve_digest = "c".repeat(64);
    assert_eq!(
        historical(&[active], &[wrong]),
        Err(LateStockError::InvalidStoredData)
    );
}

#[test]
fn refreshed_assessment_requires_the_exact_same_original_and_recognition_without_claiming_an_external_lock_race()
 {
    let original = literal(OriginalState::Active);
    let recognition = "2".repeat(32);
    assert_eq!(
        validate_same_assessment(
            std::slice::from_ref(&original),
            std::slice::from_ref(&original),
            &recognition,
            &recognition
        ),
        Ok(())
    );
    let changed = literal(OriginalState::Expired);
    assert_eq!(
        validate_same_assessment(
            std::slice::from_ref(&original),
            &[changed],
            &recognition,
            &recognition
        ),
        Err(LateStockError::StateChanged)
    );
    assert_eq!(
        validate_same_assessment(
            std::slice::from_ref(&original),
            std::slice::from_ref(&original),
            &recognition,
            &"3".repeat(32)
        ),
        Err(LateStockError::StateChanged)
    );
    assert_eq!(
        validate_same_assessment(
            std::slice::from_ref(&original),
            std::slice::from_ref(&original),
            &recognition,
            &recognition
        ),
        Ok(())
    );
}
