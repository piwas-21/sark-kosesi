//! Synthetic demand only; these are not parcel or warehouse facts.
use commerce_domain::{
    BillOfMaterials, BomComponent, ComponentKind,
    checkout_orders::OrderSnapshot,
    checkout_quotes::CapturedAmount,
    order_shipment_components::{ExpectedComponent, ShipmentComponentError, expected_components},
    order_shipments::SkuUnits,
    order_stock::{Member, Quantity},
};

#[path = "../../../tests/fixtures/checkout_order_snapshot.rs"]
mod fixture;

fn units(sku: &str, units: u32) -> SkuUnits {
    SkuUnits {
        sku: sku.into(),
        units,
    }
}
fn component(code: &str, kind: ComponentKind, quantity: u32) -> BomComponent {
    BomComponent {
        code: code.into(),
        kind,
        quantity,
    }
}
fn order() -> OrderSnapshot {
    let mut order = fixture::snapshot();
    let mut line = order.quote.lines[0].clone();
    line.sku = "SET-Z-COVERS".into();
    line.quantity = 2;
    line.vat_basis_points = 900;
    line.amounts = CapturedAmount {
        gross_minor: 200,
        net_minor: 183,
        vat_minor: 17,
    };
    order.quote.lines.push(line);
    order.quote.total = CapturedAmount {
        gross_minor: 1400,
        net_minor: 1175,
        vat_minor: 225,
    };
    let mut product = order.products[0].clone();
    product.sku = "SET-Z-COVERS".into();
    product.fill = "covers-only".into();
    order.products.push(product);
    let mut rule = order.market.products[0].clone();
    rule.sku = "SET-Z-COVERS".into();
    rule.vat_basis_points = 900;
    order.market.products.push(rule);
    assert!(order.validate().is_ok());
    order
}
fn member(sku: &str, id: char, units: u16, lines: &[(&str, i64)]) -> Member {
    Member {
        hold_id: id.to_string().repeat(32),
        sku: sku.into(),
        revision: 1,
        units,
        components: lines
            .iter()
            .map(|(code, quantity)| Quantity {
                component_code: (*code).into(),
                quantity: *quantity,
            })
            .collect(),
    }
}
fn sources() -> (Vec<Member>, Vec<BillOfMaterials>) {
    let members = vec![
        member("SET-FILLED", 'a', 3, &[("COVER", 6), ("FOAM", 3)]),
        member("SET-Z-COVERS", 'b', 2, &[("COVER", 2)]),
    ];
    let boms = vec![
        BillOfMaterials {
            sku: "SET-FILLED".into(),
            revision: 1,
            components: vec![
                component("FOAM", ComponentKind::Foam, 1),
                component("COVER", ComponentKind::Cover, 2),
            ],
        },
        BillOfMaterials {
            sku: "SET-Z-COVERS".into(),
            revision: 1,
            components: vec![component("COVER", ComponentKind::Cover, 1)],
        },
    ];
    (members, boms)
}
fn expected(code: &str, quantity: i64) -> ExpectedComponent {
    ExpectedComponent {
        code: code.into(),
        quantity,
    }
}

#[test]
fn proposed_whole_units_expand_and_shared_components_combine() {
    let (members, boms) = sources();
    let actual = expected_components(
        &order(),
        &[units("SET-Z-COVERS", 0), units("SET-FILLED", 1)],
        &[units("SET-Z-COVERS", 1), units("SET-FILLED", 2)],
        &members,
        &boms,
    )
    .expect("partial plan");
    assert_eq!(actual, vec![expected("COVER", 5), expected("FOAM", 2)]);
    let covers = expected_components(
        &order(),
        &[units("SET-FILLED", 3), units("SET-Z-COVERS", 1)],
        &[units("SET-Z-COVERS", 1)],
        &members,
        &boms,
    )
    .expect("final covers only");
    assert_eq!(covers, vec![expected("COVER", 1)]);
}

#[test]
fn mismatched_original_members_or_bom_never_become_expected_demand() {
    let order = order();
    let (members, boms) = sources();
    let prior = [units("SET-FILLED", 0), units("SET-Z-COVERS", 0)];
    let proposed = [units("SET-FILLED", 1)];
    let check = |members: &[Member], boms: &[BillOfMaterials]| {
        assert_eq!(
            expected_components(&order, &prior, &proposed, members, boms),
            Err(ShipmentComponentError::InvalidFootprint)
        );
    };
    check(&members[..1], &boms);
    check(&members, &boms[..1]);
    let mut changed = members.clone();
    changed[0].revision = 2;
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].units = 2;
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].components[0].quantity = 3;
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].components[0].quantity = 5;
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].components[0].component_code = "OTHER".into();
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].components[1].component_code = "COVER".into();
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].hold_id = changed[1].hold_id.clone();
    check(&changed, &boms);
    let mut changed = members.clone();
    changed[0].sku = "OTHER".into();
    check(&changed, &boms);
    let mut changed = boms.clone();
    changed[0].components.pop();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components.clear();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].sku = "bad sku".into();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].code = "bad code".into();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].revision = 0;
    check(&members, &changed);
    let mut changed = boms.clone();
    for index in 0..64 {
        changed[0].components.push(component(
            &format!("EXTRA-{index}"),
            ComponentKind::Cover,
            1,
        ));
    }
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].revision = 2;
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].quantity = 2;
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].quantity = 0;
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].quantity = 10_001;
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].code = "OTHER".into();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].code = "COVER".into();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].sku = "SET-Z-COVERS".into();
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[0].components[0].kind = ComponentKind::Cover;
    check(&members, &changed);
    let mut changed = boms.clone();
    changed[1].components[0].kind = ComponentKind::Foam;
    check(&members, &changed);
    // Both BOMs satisfy their own fill, but one code cannot be both kinds.
    let mut conflicting_boms = boms.clone();
    let mut conflicting_members = members.clone();
    conflicting_boms[1].components[0].code = "FOAM".into();
    conflicting_members[1].components[0].component_code = "FOAM".into();
    check(&conflicting_members, &conflicting_boms);
}

#[test]
fn incomplete_or_excess_proposal_cannot_create_component_demand() {
    let (members, boms) = sources();
    assert!(matches!(
        expected_components(
            &order(),
            &[units("SET-FILLED", 0)],
            &[units("SET-FILLED", 1)],
            &members,
            &boms
        ),
        Err(ShipmentComponentError::Quantities(_))
    ));
    assert!(matches!(
        expected_components(
            &order(),
            &[units("SET-FILLED", 3), units("SET-Z-COVERS", 0)],
            &[units("SET-FILLED", 1)],
            &members,
            &boms
        ),
        Err(ShipmentComponentError::Quantities(_))
    ));
}

#[test]
fn maximum_twenty_variants_with_sixty_four_distinct_components_each() {
    let mut order = fixture::snapshot();
    let line = order.quote.lines[0].clone();
    let product = order.products[0].clone();
    let rule = order.market.products[0].clone();
    order.quote.lines.clear();
    order.products.clear();
    order.market.products.clear();
    let mut members = Vec::new();
    let mut boms = Vec::new();
    let mut prior = Vec::new();
    let mut proposed = Vec::new();
    for sku_index in 0..20 {
        let sku = format!("SKU-{sku_index:02}");
        let mut next_line = line.clone();
        next_line.sku = sku.clone();
        order.quote.lines.push(next_line);
        let mut next_product = product.clone();
        next_product.sku = sku.clone();
        order.products.push(next_product);
        let mut next_rule = rule.clone();
        next_rule.sku = sku.clone();
        order.market.products.push(next_rule);
        let mut bom_lines = Vec::new();
        let mut member_lines = Vec::new();
        for code_index in 0..64 {
            let code = format!("COMP-{sku_index:02}-{code_index:02}");
            let kind = if code_index == 0 {
                ComponentKind::Foam
            } else {
                ComponentKind::Cover
            };
            bom_lines.push(component(&code, kind, 1));
            member_lines.push(Quantity {
                component_code: code,
                quantity: 3,
            });
        }
        boms.push(BillOfMaterials {
            sku: sku.clone(),
            revision: 1,
            components: bom_lines,
        });
        members.push(Member {
            hold_id: format!("{sku_index:032x}"),
            sku: sku.clone(),
            revision: 1,
            units: 3,
            components: member_lines,
        });
        prior.push(units(&sku, 0));
        proposed.push(units(&sku, 1));
    }
    order.quote.total = CapturedAmount {
        gross_minor: 6900,
        net_minor: 5704,
        vat_minor: 1196,
    };
    assert!(order.validate().is_ok());
    let result =
        expected_components(&order, &prior, &proposed, &members, &boms).expect("bounded maximum");
    assert_eq!(result.len(), 1280);
    assert_eq!(result.first(), Some(&expected("COMP-00-00", 1)));
    assert_eq!(result.last(), Some(&expected("COMP-19-63", 1)));
    assert_eq!(result.iter().map(|item| item.quantity).sum::<i64>(), 1280);
}
