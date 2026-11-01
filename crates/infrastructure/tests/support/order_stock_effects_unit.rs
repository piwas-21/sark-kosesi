//! Literal stored movement contracts, independently of SQL source validation.
use super::*;

const HOLD: &str = "11111111111111111111111111111111";
type MovementCorruption = (&'static str, fn(&mut Movement));
fn quantity() -> Quantity {
    Quantity {
        component_code: "SEAT-COVER".into(),
        quantity: 2,
    }
}
fn movement(action: &str) -> Movement {
    let (on_hand_delta, reserved_delta, on_hand_after, reserved_after) = match action {
        "reserve" => (0, 2, 10, 2),
        "consume-hold" => (-2, -2, 8, 0),
        "release-hold" | "expire-hold" => (0, -2, 10, 0),
        _ => panic!("unsupported literal fixture action"),
    };
    Movement {
        hold_id: HOLD.into(),
        component_code: "SEAT-COVER".into(),
        action: action.into(),
        effect_key: format!("hold:{HOLD}:{action}:SEAT-COVER"),
        quantity: 2,
        actor: "fixture-owner".into(),
        reason: "sealed BOM hold transition".into(),
        on_hand_delta,
        quarantine_delta: 0,
        reserved_delta,
        on_hand_after,
        quarantine_after: 1,
        reserved_after,
        sequence: 7,
    }
}
#[test]
fn exact_stored_reserve_consume_release_and_expiry_have_literal_delta_contracts() {
    for action in ["reserve", "consume-hold", "release-hold", "expire-hold"] {
        assert_eq!(exact(&movement(action), HOLD, &quantity(), action), Ok(()));
    }
    let mut bounded_actor = movement("reserve");
    bounded_actor.actor = "ü".repeat(128);
    assert_eq!(exact(&bounded_actor, HOLD, &quantity(), "reserve"), Ok(()));
    bounded_actor.actor.push('ü');
    assert_eq!(
        exact(&bounded_actor, HOLD, &quantity(), "reserve"),
        Err(SettlementError::InvalidStoredData)
    );
}
#[test]
fn each_stored_movement_binding_actor_delta_and_sequence_is_independently_required() {
    let corruptions: [MovementCorruption; 14] = [
        ("hold", |r| {
            r.hold_id = "22222222222222222222222222222222".into()
        }),
        ("component", |r| r.component_code = "OTHER-COVER".into()),
        ("action", |r| r.action = "release-hold".into()),
        ("effect", |r| r.effect_key = "forged-effect".into()),
        ("quantity", |r| r.quantity = 3),
        ("reason", |r| r.reason = "forged reason".into()),
        ("empty actor", |r| r.actor = " \t\n".into()),
        ("oversize actor", |r| r.actor = "x".repeat(129)),
        ("on hand delta", |r| r.on_hand_delta = -1),
        ("quarantine delta", |r| r.quarantine_delta = 1),
        ("reserved delta", |r| r.reserved_delta = -1),
        ("sequence", |r| r.sequence = 0),
        ("invalid after", |r| r.on_hand_after = -1),
        ("unbacked after", |r| {
            r.on_hand_after = 1;
            r.reserved_after = 1;
        }),
    ];
    for (name, change) in corruptions {
        let mut damaged = movement("consume-hold");
        change(&mut damaged);
        assert_eq!(
            exact(&damaged, HOLD, &quantity(), "consume-hold"),
            Err(SettlementError::InvalidStoredData),
            "{name}"
        );
        assert_eq!(
            exact(&movement("consume-hold"), HOLD, &quantity(), "consume-hold"),
            Ok(()),
            "repaired {name}"
        );
    }
}
#[test]
fn reserve_backing_and_each_terminal_delta_remain_distinct() {
    let mut reserve = movement("reserve");
    reserve.reserved_after = 1;
    assert_eq!(
        exact(&reserve, HOLD, &quantity(), "reserve"),
        Err(SettlementError::InvalidStoredData)
    );
    reserve.reserved_after = 2;
    assert_eq!(exact(&reserve, HOLD, &quantity(), "reserve"), Ok(()));
    reserve.reserved_delta = -2;
    assert_eq!(
        exact(&reserve, HOLD, &quantity(), "reserve"),
        Err(SettlementError::InvalidStoredData)
    );
    for action in ["release-hold", "expire-hold"] {
        let mut terminal = movement(action);
        terminal.on_hand_delta = -2;
        assert_eq!(
            exact(&terminal, HOLD, &quantity(), action),
            Err(SettlementError::InvalidStoredData)
        );
        terminal.on_hand_delta = 0;
        assert_eq!(exact(&terminal, HOLD, &quantity(), action), Ok(()));
    }
}

#[path = "order_stock_effects_postgres.rs"]
mod postgres;
