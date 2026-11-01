//! Literal original-reservation conservation, bounds and clock policy.
use commerce_domain::order_stock::*;
fn quantity(code: &str, quantity: i64) -> Quantity {
    Quantity {
        component_code: code.into(),
        quantity,
    }
}
fn member(id: u128, sku: &str, components: Vec<Quantity>) -> Member {
    Member {
        hold_id: format!("{id:032x}"),
        sku: sku.into(),
        revision: 1,
        units: 1,
        components,
    }
}
fn footprint() -> Footprint {
    Footprint {
        schema_version: 1,
        members: vec![
            member(1, "SET-COVERS", vec![quantity("SEAT-COVER", 2)]),
            member(
                2,
                "SET-FILLED",
                vec![quantity("SEAT-COVER", 2), quantity("SEAT-FOAM", 2)],
            ),
        ],
        components: vec![
            Component {
                component_code: "SEAT-COVER".into(),
                quantity: 4,
                before: Balance {
                    on_hand: 10,
                    quarantine: 1,
                    reserved: 4,
                },
                after: Balance {
                    on_hand: 6,
                    quarantine: 1,
                    reserved: 0,
                },
            },
            Component {
                component_code: "SEAT-FOAM".into(),
                quantity: 2,
                before: Balance {
                    on_hand: 4,
                    quarantine: 1,
                    reserved: 2,
                },
                after: Balance {
                    on_hand: 2,
                    quarantine: 1,
                    reserved: 0,
                },
            },
        ],
    }
}
#[test]
fn literal_two_member_footprint_aggregates_shared_components_and_preserves_quarantine() {
    let value = footprint();
    assert_eq!(value.validate(), Ok(()));
    assert_eq!(
        Footprint::demand(&value.members)
            .expect("closed membership")
            .into_iter()
            .collect::<Vec<_>>(),
        vec![("SEAT-COVER".into(), 4), ("SEAT-FOAM".into(), 2)]
    );
    assert!(
        Balance {
            on_hand: 10,
            quarantine: 1,
            reserved: 4
        }
        .consume(4)
            == Ok(Balance {
                on_hand: 6,
                quarantine: 1,
                reserved: 0
            })
    );
    for balance in [
        Balance {
            on_hand: 1,
            quarantine: 1,
            reserved: 1,
        },
        Balance {
            on_hand: -1,
            quarantine: 0,
            reserved: 0,
        },
        Balance {
            on_hand: i64::MAX,
            quarantine: i64::MAX,
            reserved: 1,
        },
    ] {
        assert!(balance.stock().is_err());
    }
    for quantity in [0, -1, 5, i64::MAX] {
        assert!(value.components[0].before.consume(quantity).is_err());
    }
}
#[test]
fn malformed_duplicate_incomplete_and_nonconserved_member_footprints_fail_closed() {
    let changes: [fn(&mut Footprint); 18] = [
        |f| f.schema_version = 2,
        |f| f.members.clear(),
        |f| f.members[0].hold_id = "bad".into(),
        |f| f.members[0].sku = "bad sku".into(),
        |f| f.members[0].revision = 0,
        |f| f.members[0].units = 0,
        |f| f.members[0].units = 1001,
        |f| f.members[1].hold_id = f.members[0].hold_id.clone(),
        |f| f.members[1].sku = f.members[0].sku.clone(),
        |f| f.members[0].components.clear(),
        |f| f.members[0].components[0].component_code = "bad code".into(),
        |f| f.members[0].components[0].quantity = 0,
        |f| f.members[0].components[0].quantity = 10_000_001,
        |f| {
            let duplicate = f.members[0].components[0].clone();
            f.members[0].components.push(duplicate)
        },
        |f| {
            f.components.pop();
        },
        |f| f.components[0].quantity = 3,
        |f| f.components[0].after.quarantine = 0,
        |f| f.components[0].after.on_hand = 7,
    ];
    for change in changes {
        let mut bad = footprint();
        change(&mut bad);
        assert_eq!(bad.validate(), Err(SettlementPolicyError::InvalidSnapshot));
    }
    let mut duplicate = footprint();
    duplicate.components[1] = duplicate.components[0].clone();
    assert!(duplicate.validate().is_err());
}
#[test]
fn twenty_members_with_sixty_four_distinct_components_each_are_valid_without_kind_limits() {
    let mut value = Footprint {
        schema_version: 1,
        members: vec![],
        components: vec![],
    };
    for m in 0..20 {
        let mut lines = vec![];
        for line in 0..64 {
            let code = format!("COVER-{m:02}-{line:02}");
            lines.push(quantity(&code, 1));
            value.components.push(Component {
                component_code: code,
                quantity: 1,
                before: Balance {
                    on_hand: 10,
                    quarantine: 1,
                    reserved: 1,
                },
                after: Balance {
                    on_hand: 9,
                    quarantine: 1,
                    reserved: 0,
                },
            });
        }
        value
            .members
            .push(member(m + 1, &format!("SET-{m:02}"), lines));
    }
    assert_eq!(value.validate(), Ok(()));
    assert_eq!(
        Footprint::demand(&value.members)
            .expect("full bounded source")
            .len(),
        1280
    );
    let mut too_many_members = value.clone();
    too_many_members
        .members
        .push(member(99, "SET-EXTRA", vec![quantity("EXTRA", 1)]));
    assert!(too_many_members.validate().is_err());
    let mut too_many_lines = value.clone();
    too_many_lines.members[0]
        .components
        .push(quantity("COVER-EXTRA", 1));
    assert!(too_many_lines.validate().is_err());
    let mut too_many_union = value;
    too_many_union.components.push(Component {
        component_code: "EXTRA".into(),
        quantity: 1,
        before: Balance {
            on_hand: 1,
            quarantine: 0,
            reserved: 1,
        },
        after: Balance {
            on_hand: 0,
            quarantine: 0,
            reserved: 0,
        },
    });
    assert!(too_many_union.validate().is_err());
}
#[test]
fn settlement_identity_actor_and_explicit_expiry_clock_are_closed() {
    let value = Settlement {
        id: "1".repeat(32),
        order_id: "2".repeat(32),
        attempt_id: "3".repeat(32),
        recognition_id: "4".repeat(32),
        group_id: "5".repeat(32),
        actor: "payment-settlement".into(),
        settled_at: 1500,
        footprint: footprint(),
    };
    assert_eq!(value.validate(), Ok(()));
    let changes: [fn(&mut Settlement); 8] = [
        |s| s.id = "bad".into(),
        |s| s.order_id = "bad".into(),
        |s| s.attempt_id = "bad".into(),
        |s| s.recognition_id = "bad".into(),
        |s| s.group_id = "bad".into(),
        |s| s.actor = "customer".into(),
        |s| s.settled_at = -1,
        |s| s.settled_at = 9_007_199_254_740_992,
    ];
    for change in changes {
        let mut bad = value.clone();
        change(&mut bad);
        assert!(bad.validate().is_err());
    }
    for at in [1500, 1999] {
        assert_eq!(check_time(at, 1100, 2000, 1500), Ok(()));
    }
    assert_eq!(
        check_time(2000, 1100, 2000, 1500),
        Err(SettlementPolicyError::RecoveryRequired)
    );
    for at in [1099, 1499, i64::MAX] {
        assert_eq!(
            check_time(at, 1100, 2000, 1500),
            Err(SettlementPolicyError::InvalidClock)
        );
    }
    assert_eq!(
        check_time(1500, 2000, 1100, 1500),
        Err(SettlementPolicyError::InvalidSnapshot)
    );
    let mut encoded = serde_json::to_value(&value).expect("closed receipt");
    encoded["customer_authorized"] = true.into();
    assert!(serde_json::from_value::<Settlement>(encoded).is_err());
}
