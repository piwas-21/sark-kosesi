//! Prepared adapter tuples must be bounded before any building child is claimed.
use super::*;
use commerce_domain::{BomComponent, ComponentKind};
fn plan(lines: usize) -> Plan {
    Plan {
        line: BasketHoldLine {
            id: "1".repeat(32),
            sku: "SET-COVERS".into(),
            revision: 1,
            units: 1000,
        },
        bom: BillOfMaterials {
            sku: "SET-COVERS".into(),
            revision: 1,
            components: (0..lines)
                .map(|i| BomComponent {
                    code: format!("COVER-{i:02}"),
                    kind: ComponentKind::Cover,
                    quantity: 10000,
                })
                .collect(),
        },
    }
}
#[test]
fn fresh_bom_adapter_contract_rejects_empty_or_sixty_five_lines_and_accepts_all_sixty_four() {
    assert!(matches!(
        validate_plans(&[plan(0)]),
        Err(HoldWriteError::InvalidRequest)
    ));
    assert!(validate_plans(&[plan(1)]).is_ok());
    assert!(matches!(
        validate_plans(&[plan(65)]),
        Err(HoldWriteError::InvalidRequest)
    ));
    assert!(validate_plans(&[plan(64)]).is_ok());
}
#[test]
fn prepared_members_preserve_literal_maximum_per_pair_demand_and_all_distinct_codes() {
    let fresh = FreshPreparation {
        id: "2".repeat(32),
        owner: "3".repeat(32),
        created: 2000,
        expires: 2060,
        plans: vec![plan(64)],
    };
    let members = fresh.members().expect("bounded prepared member projection");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].hold_id, "1".repeat(32));
    assert_eq!(members[0].sku, "SET-COVERS");
    assert_eq!(members[0].revision, 1);
    assert_eq!(members[0].units, 1000);
    assert_eq!(members[0].components.len(), 64);
    assert!(
        members[0]
            .components
            .iter()
            .all(|line| line.quantity == 10_000_000)
    );
    assert_eq!(members[0].components[0].component_code, "COVER-00");
    assert_eq!(members[0].components[63].component_code, "COVER-63");
}
