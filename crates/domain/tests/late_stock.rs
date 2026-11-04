//! Literal late-paid conservation and frozen original/fresh custody.
use commerce_domain::{
    late_stock::*,
    order_stock::{Balance, Member, Quantity},
};

fn original(id: u128, sku: &str, state: OriginalState, cover: i64, foam: bool) -> OriginalMember {
    let mut components = vec![Quantity {
        component_code: "SEAT-COVER".into(),
        quantity: cover,
    }];
    if foam {
        components.push(Quantity {
            component_code: "SEAT-FOAM".into(),
            quantity: 2,
        });
    }
    let pairs = components
        .iter()
        .enumerate()
        .map(|(index, line)| PairReference {
            component_code: line.component_code.clone(),
            reserve_sequence: (id as i64) * 10 + index as i64,
            reserve_digest: "a".repeat(64),
            terminal_digest: if state == OriginalState::Active {
                None
            } else {
                Some("b".repeat(64))
            },
            terminal_sequence: if state == OriginalState::Active {
                None
            } else {
                Some((id as i64) * 10 + 5 + index as i64)
            },
        })
        .collect();
    OriginalMember {
        member: Member {
            hold_id: format!("{id:032x}"),
            sku: sku.into(),
            revision: 1,
            units: 1,
            components,
        },
        state,
        pairs,
    }
}
fn assessment() -> Vec<AssessedComponent> {
    vec![
        AssessedComponent {
            component_code: "SEAT-COVER".into(),
            demand: 4,
            active: 4,
            before: Balance {
                on_hand: 10,
                quarantine: 1,
                reserved: 4,
            },
        },
        AssessedComponent {
            component_code: "SEAT-FOAM".into(),
            demand: 2,
            active: 2,
            before: Balance {
                on_hand: 4,
                quarantine: 1,
                reserved: 2,
            },
        },
    ]
}
fn source() -> Vec<OriginalMember> {
    vec![
        original(1, "SET-COVERS", OriginalState::Active, 2, false),
        original(2, "SET-FILLED", OriginalState::Active, 2, true),
    ]
}
fn late() -> LateFootprint {
    let original = source();
    let mut fresh: Vec<_> = original.iter().map(|m| m.member.clone()).collect();
    fresh[0].hold_id = "3".repeat(32);
    fresh[1].hold_id = "4".repeat(32);
    LateFootprint {
        schema_version: 1,
        original,
        fresh,
        components: assessment()
            .iter()
            .map(|c| {
                StageComponent::from_assessment(c)
                    .expect("valid physical assessment")
                    .expect("full demand fits")
            })
            .collect(),
    }
}
#[test]
fn late_stock_literal_active_reclaim_expires_reserves_then_consumes_full_shared_demand() {
    let c = &assessment()[0];
    assert_eq!(c.available(), Ok(9));
    assert_eq!(c.can_allocate(), Ok(true));
    let stage = StageComponent::from_assessment(c)
        .expect("physical policy")
        .expect("whole demand fits");
    assert!(
        stage.expired
            == Balance {
                on_hand: 10,
                quarantine: 1,
                reserved: 0
            }
    );
    assert!(
        stage.reserved
            == Balance {
                on_hand: 10,
                quarantine: 1,
                reserved: 4
            }
    );
    assert!(
        stage.after
            == Balance {
                on_hand: 6,
                quarantine: 1,
                reserved: 0
            }
    );
    let f = late();
    assert_eq!(f.validate(), Ok(()));
    let (full, active) = summarize(&f.original).expect("bounded exact source");
    assert_eq!(
        full.into_iter().collect::<Vec<_>>(),
        vec![("SEAT-COVER".into(), 4), ("SEAT-FOAM".into(), 2)]
    );
    assert_eq!(
        active.into_iter().collect::<Vec<_>>(),
        vec![("SEAT-COVER".into(), 4), ("SEAT-FOAM".into(), 2)]
    );
}
#[test]
fn late_stock_expired_source_and_other_reservations_obey_the_same_closed_formula() {
    let c = AssessedComponent {
        component_code: "SEAT-COVER".into(),
        demand: 4,
        active: 0,
        before: Balance {
            on_hand: 10,
            quarantine: 1,
            reserved: 3,
        },
    };
    assert_eq!(c.available(), Ok(6));
    let stage = StageComponent::from_assessment(&c)
        .expect("valid physical stock")
        .expect("four of six available");
    assert!(
        stage.expired
            == Balance {
                on_hand: 10,
                quarantine: 1,
                reserved: 3
            }
    );
    assert!(
        stage.reserved
            == Balance {
                on_hand: 10,
                quarantine: 1,
                reserved: 7
            }
    );
    assert!(
        stage.after
            == Balance {
                on_hand: 6,
                quarantine: 1,
                reserved: 3
            }
    );
    let scarce = AssessedComponent {
        component_code: "SEAT-FOAM".into(),
        demand: 2,
        active: 0,
        before: Balance {
            on_hand: 2,
            quarantine: 1,
            reserved: 0,
        },
    };
    assert_eq!(scarce.available(), Ok(1));
    assert_eq!(scarce.can_allocate(), Ok(false));
    assert!(StageComponent::from_assessment(&scarce) == Ok(None));
    let mut invalid = c.clone();
    invalid.active = 4;
    assert!(
        invalid.available().is_err(),
        "cannot reclaim reservations that do not exist"
    );
    for change in [
        |c: &mut AssessedComponent| c.demand = 0,
        |c: &mut AssessedComponent| c.active = -1,
        |c: &mut AssessedComponent| c.active = 5,
        |c: &mut AssessedComponent| c.before.on_hand = -1,
        |c: &mut AssessedComponent| c.before.quarantine = i64::MAX,
    ] {
        let mut bad = c.clone();
        change(&mut bad);
        assert!(bad.can_allocate().is_err());
    }
}
#[test]
fn late_stock_source_refs_states_fresh_members_and_all_four_stages_are_closed() {
    let changes: [fn(&mut LateFootprint); 18] = [
        |f| f.schema_version = 2,
        |f| f.original.clear(),
        |f| f.original[0].pairs.clear(),
        |f| f.original[0].pairs[0].reserve_sequence = 0,
        |f| f.original[0].pairs[0].terminal_sequence = Some(20),
        |f| f.original[0].pairs[0].component_code = "SEAT-OTHER".into(),
        |f| f.original[0].state = OriginalState::Released,
        |f| f.original[0].state = OriginalState::Consumed,
        |f| f.fresh.clear(),
        |f| f.fresh[0].hold_id = f.original[0].member.hold_id.clone(),
        |f| f.fresh[0].components[0].quantity = 1,
        |f| f.fresh[0].sku = "OTHER-COVERS".into(),
        |f| f.fresh[0].revision = 2,
        |f| f.components[0].active = 3,
        |f| f.components[0].expired.reserved = 1,
        |f| f.components[0].reserved.reserved = 3,
        |f| f.components[0].after.on_hand = 7,
        |f| f.components[0].after.quarantine = 0,
    ];
    for change in changes {
        let mut bad = late();
        change(&mut bad);
        assert!(bad.validate().is_err());
    }
    let expired = original(1, "SET-COVERS", OriginalState::Expired, 2, false);
    assert_eq!(expired.validate(), Ok(()));
    let mut missing = expired.clone();
    missing.pairs[0].terminal_sequence = None;
    assert!(missing.validate().is_err());
    let mut reversed = expired;
    reversed.pairs[0].terminal_sequence = Some(9);
    assert!(reversed.validate().is_err());
    let value = serde_json::to_value(late()).expect("closed serializable footprint");
    let mut extra = value.clone();
    extra["caller_approved"] = true.into();
    assert!(serde_json::from_value::<LateFootprint>(extra).is_err());
    let empty = serde_json::from_value::<LateFootprint>(
        serde_json::json!({"schema_version":1,"original":[],"fresh":[],"components":[]}),
    )
    .expect("typed empty input still requires semantic validation");
    assert!(empty.validate().is_err());
}
#[test]
fn late_stock_receipt_actor_identity_clock_and_exact_sixty_second_window_are_server_owned() {
    let receipt = LateAllocation {
        id: "5".repeat(32),
        order_id: "6".repeat(32),
        attempt_id: "7".repeat(32),
        recognition_id: "8".repeat(32),
        original_group_id: "9".repeat(32),
        fresh_group_id: "a".repeat(32),
        actor: "late-payment-allocation".into(),
        allocated_at: 2000,
        expires_at: 2060,
        footprint: late(),
    };
    assert_eq!(receipt.validate(), Ok(()));
    let changes: [fn(&mut LateAllocation); 10] = [
        |r| r.id = "bad".into(),
        |r| r.order_id = "bad".into(),
        |r| r.attempt_id = "bad".into(),
        |r| r.recognition_id = "bad".into(),
        |r| r.original_group_id = "bad".into(),
        |r| r.fresh_group_id = r.original_group_id.clone(),
        |r| r.actor = "customer".into(),
        |r| r.allocated_at = -1,
        |r| r.expires_at = 2000,
        |r| r.expires_at = 2061,
    ];
    for change in changes {
        let mut bad = receipt.clone();
        change(&mut bad);
        assert!(bad.validate().is_err());
    }
    assert_eq!(check_time(2000, 1100, 1500), Ok(()));
    for at in [1099, 1499, i64::MAX] {
        assert!(check_time(at, 1100, 1500).is_err());
    }
    assert_eq!(check_window(2000, 2060), Ok(()));
    assert_eq!(check_window(2059, 2060), Ok(()));
    assert_eq!(check_window(2060, 2060), Err(PolicyError::WindowExpired));
}
#[test]
fn late_stock_review_reasons_are_closed_priority_stops_and_never_allocation_claims() {
    let mut footprint = ReviewFootprint {
        schema_version: 1,
        original: source(),
        components: assessment(),
    };
    assert_eq!(footprint.validate(), Ok(()));
    assert_eq!(ReviewReason::classify(&footprint), Ok(None));
    footprint.original[0] = original(1, "SET-COVERS", OriginalState::Released, 2, false);
    footprint.components[0].active = 2;
    footprint.components[0].before.reserved = 2;
    assert_eq!(
        ReviewReason::classify(&footprint),
        Ok(Some(ReviewReason::ReleaseIntentUnresolved))
    );
    let release = ReviewCase {
        id: "1".repeat(32),
        order_id: "2".repeat(32),
        attempt_id: "3".repeat(32),
        recognition_id: "4".repeat(32),
        group_id: "5".repeat(32),
        actor: "late-stock-review".into(),
        reason: ReviewReason::ReleaseIntentUnresolved,
        assessed_at: 2000,
        footprint: footprint.clone(),
    };
    assert_eq!(release.validate(), Ok(()));
    let changes: [fn(&mut ReviewCase); 9] = [
        |r| r.id = "bad".into(),
        |r| r.order_id = "bad".into(),
        |r| r.attempt_id = "bad".into(),
        |r| r.recognition_id = "bad".into(),
        |r| r.group_id = "bad".into(),
        |r| r.actor = "finance-operator".into(),
        |r| r.reason = ReviewReason::ComponentScarcity,
        |r| r.assessed_at = -1,
        |r| r.assessed_at = i64::MAX,
    ];
    for change in changes {
        let mut bad = release.clone();
        change(&mut bad);
        assert!(bad.validate().is_err());
    }
    footprint.original[1] = original(2, "SET-FILLED", OriginalState::Consumed, 2, true);
    for component in &mut footprint.components {
        component.active = 0;
        component.before.reserved = 0;
    }
    assert_eq!(
        ReviewReason::classify(&footprint),
        Ok(Some(ReviewReason::PriorConsumptionWithoutAllocation)),
        "consumption remains the stronger stop even with release present"
    );
    for member in &mut footprint.original {
        member.state = OriginalState::Expired;
    }
    footprint.components[1].before.on_hand = 2;
    assert_eq!(
        ReviewReason::classify(&footprint),
        Ok(Some(ReviewReason::ComponentScarcity))
    );
    footprint.components[1].before.on_hand = 4;
    assert_eq!(ReviewReason::classify(&footprint), Ok(None));
    let mut invalid = footprint.clone();
    invalid.components.pop();
    assert!(invalid.validate().is_err());
    let mut invalid = footprint.clone();
    invalid.schema_version = 2;
    assert!(invalid.validate().is_err());
    let mut invalid = footprint;
    invalid.components[0].active = 1;
    assert!(invalid.validate().is_err());
    assert_eq!(OriginalState::Active.code(), "active");
    assert_eq!(OriginalState::Expired.code(), "expired");
    assert_eq!(OriginalState::Released.code(), "released");
    assert_eq!(OriginalState::Consumed.code(), "consumed");
    assert_eq!(
        ReviewReason::PriorConsumptionWithoutAllocation.code(),
        "prior_consumption_without_allocation"
    );
    assert_eq!(
        ReviewReason::ReleaseIntentUnresolved.code(),
        "release_intent_unresolved"
    );
    assert_eq!(ReviewReason::ComponentScarcity.code(), "component_scarcity");
    assert!(serde_json::from_value::<ReviewReason>(serde_json::json!("retry_authorized")).is_err());
}
#[test]
fn late_stock_twenty_by_sixty_four_pairs_have_no_two_kind_limit_and_excess_members_or_lines_fail() {
    let mut original = vec![];
    let mut fresh = vec![];
    let mut components = vec![];
    for m in 0..20u128 {
        let mut lines = vec![];
        let mut pairs = vec![];
        for n in 0..64 {
            let code = format!("COVER-{m:02}-{n:02}");
            lines.push(Quantity {
                component_code: code.clone(),
                quantity: 1,
            });
            pairs.push(PairReference {
                component_code: code.clone(),
                reserve_sequence: (m as i64) * 64 + n + 1,
                reserve_digest: "a".repeat(64),
                terminal_sequence: None,
                terminal_digest: None,
            });
            components.push(
                StageComponent::from_assessment(&AssessedComponent {
                    component_code: code,
                    demand: 1,
                    active: 1,
                    before: Balance {
                        on_hand: 2,
                        quarantine: 0,
                        reserved: 1,
                    },
                })
                .expect("valid stock")
                .expect("one physical unit fits"),
            );
        }
        let member = Member {
            hold_id: format!("{:032x}", m + 1),
            sku: format!("SET-{m:02}"),
            revision: 1,
            units: 1,
            components: lines,
        };
        let mut replacement = member.clone();
        replacement.hold_id = format!("{:032x}", m + 100);
        original.push(OriginalMember {
            member,
            state: OriginalState::Active,
            pairs,
        });
        fresh.push(replacement);
    }
    let footprint = LateFootprint {
        schema_version: 1,
        original,
        fresh,
        components,
    };
    assert_eq!(footprint.validate(), Ok(()));
    assert_eq!(
        footprint
            .original
            .iter()
            .map(|m| m.pairs.len())
            .sum::<usize>(),
        1280
    );
    assert_eq!(footprint.components.len(), 1280);
    let mut bad = footprint.clone();
    let mut extra = bad.original[0].clone();
    extra.member.hold_id = "f".repeat(32);
    extra.member.sku = "EXTRA-SET".into();
    bad.original.push(extra);
    assert!(bad.validate().is_err());
    let mut bad = footprint.clone();
    bad.original[0].member.components.push(Quantity {
        component_code: "EXTRA-COVER".into(),
        quantity: 1,
    });
    assert!(bad.validate().is_err());
    let mut bad = footprint.clone();
    bad.fresh[0].components[0].quantity = 10_000_001;
    assert!(bad.validate().is_err());
    let mut bad = footprint.clone();
    bad.original[0].pairs[0].reserve_digest = "bad".into();
    assert!(bad.validate().is_err());
    let mut bad = footprint;
    bad.original[0].pairs[0].terminal_digest = Some("b".repeat(64));
    assert!(bad.validate().is_err());
}

#[test]
fn late_stock_original_reference_digests_and_clock_metadata_are_independently_bounded() {
    let mut member = original(2, "SET-FILLED", OriginalState::Expired, 2, true);
    member.pairs[0].reserve_digest = "0123456789abcdef".repeat(4);
    member.pairs[0].terminal_digest = Some("0".repeat(64));
    assert_eq!(member.validate(), Ok(()));
    for digest in ["g".repeat(64), "F".repeat(64), "0".repeat(63)] {
        let mut bad = member.clone();
        bad.pairs[0].reserve_digest = digest.clone();
        assert!(bad.validate().is_err());
        let mut bad = member.clone();
        bad.pairs[0].terminal_digest = Some(digest);
        assert!(bad.validate().is_err());
    }
    let mut duplicate = member.clone();
    duplicate.pairs[1] = duplicate.pairs[0].clone();
    assert!(duplicate.validate().is_err());
    let mut absent = member;
    absent.pairs[0].terminal_digest = None;
    assert!(absent.validate().is_err());
    for (created, recognized) in [(-1, 1500), (1500, 1499), (1100, 9_007_199_254_740_992)] {
        assert_eq!(
            check_time(2000, created, recognized),
            Err(PolicyError::InvalidSnapshot)
        );
    }
    for expiry in [i64::MIN, 59, 9_007_199_254_740_992] {
        assert_eq!(
            check_window(2000, expiry),
            Err(PolicyError::InvalidSnapshot)
        );
    }
    assert_eq!(check_window(1999, 2060), Err(PolicyError::InvalidClock));
}

#[test]
fn late_stock_matching_source_assessments_still_require_full_fresh_membership_and_only_active_or_expired()
 {
    let value = late();
    let mut bad = value.clone();
    bad.fresh.pop();
    assert!(bad.validate().is_err());
    let mut bad = value.clone();
    bad.fresh[0].units = 2;
    assert!(bad.validate().is_err());
    let mut bad = value.clone();
    bad.components[0].component_code = "OTHER-COVER".into();
    assert!(bad.validate().is_err());
    let mut bad = value.clone();
    bad.components[0].demand = 3;
    assert!(bad.validate().is_err());
    for state in [OriginalState::Released, OriginalState::Consumed] {
        let mut bad = value.clone();
        bad.original[0] = original(1, "SET-COVERS", state, 2, false);
        bad.components[0].active = 2;
        bad.components[0].before.reserved = 2;
        bad.components[0].expired.reserved = 0;
        bad.components[0].reserved.reserved = 4;
        bad.components[0].after.reserved = 0;
        let review = ReviewFootprint {
            schema_version: 1,
            original: bad.original.clone(),
            components: vec![
                AssessedComponent {
                    component_code: "SEAT-COVER".into(),
                    demand: 4,
                    active: 2,
                    before: Balance {
                        on_hand: 10,
                        quarantine: 1,
                        reserved: 2,
                    },
                },
                assessment()[1].clone(),
            ],
        };
        assert_eq!(
            review.validate(),
            Ok(()),
            "complete terminal source is valid review evidence"
        );
        assert!(
            bad.validate().is_err(),
            "coherent release/consume source still grants no replacement admission"
        );
    }
}
