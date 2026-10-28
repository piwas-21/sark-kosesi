use commerce_domain::{BillOfMaterials, BomComponent, ComponentKind};
include!("../../../tests/fixtures/specification.rs");
fn bom(filled: bool) -> BillOfMaterials {
    let mut components = vec![BomComponent {
        code: "DEMO-COVER".into(),
        kind: ComponentKind::Cover,
        quantity: 2,
    }];
    if filled {
        components.push(BomComponent {
            code: "DEMO-FOAM".into(),
            kind: ComponentKind::Foam,
            quantity: 2,
        });
    }
    BillOfMaterials {
        sku: "DEMO-FIXTURE".into(),
        revision: 1,
        components,
    }
}
#[test]
fn exact_composition_measurement_units_and_bom_membership_are_bounded() {
    for filled in [false, true] {
        assert_eq!(draft(filled).validate(&bom(filled)), Ok(()));
    }
    let mut boundary = draft(false);
    boundary.footprint = DimensionsMm {
        width_mm: 10000,
        depth_mm: 10000,
        height_mm: 10000,
        tolerance_mm: 500,
    };
    boundary.parcels[0].gross_weight_g = 1000000;
    assert_eq!(boundary.validate(&bom(false)), Ok(()));
    for case in 0..16 {
        let mut invalid = draft(true);
        match case {
            0 => invalid.footprint.width_mm = 0,
            1 => invalid.footprint.depth_mm = 10001,
            2 => invalid.footprint.height_mm = 5,
            3 => {
                invalid.footprint = DimensionsMm {
                    width_mm: 1000,
                    depth_mm: 1000,
                    height_mm: 1000,
                    tolerance_mm: 501,
                }
            }
            4 => invalid.components.clear(),
            5 => invalid.components = vec![invalid.components[0].clone(); 65],
            6 => {
                invalid.components.pop();
            }
            7 => invalid.components[1].component_code = "DEMO-OTHER".into(),
            8 => invalid.components[1].component_code = "DEMO-COVER".into(),
            9 => invalid.components[0].dimensions.width_mm = 0,
            10 => invalid.components[1].material = invalid.components[0].material.clone(),
            11 => invalid.parcels.clear(),
            12 => invalid.parcels = vec![invalid.parcels[0].clone(); 65],
            13 => invalid.parcels[0].gross_weight_g = 0,
            14 => invalid.parcels[0].gross_weight_g = 1000001,
            15 => invalid.parcels[0].dimensions.depth_mm = 0,
            _ => unreachable!(),
        }
        assert!(invalid.validate(&bom(true)).is_err(), "invalid case {case}");
    }
}
#[test]
fn material_fractions_density_and_all_evidence_references_fail_closed() {
    for case in 0..18 {
        let mut invalid = draft(true);
        match case {
            0 => invalid.assembly_evidence_ref = " \t".into(),
            1 => invalid.packaging_evidence_ref = "x".repeat(513),
            2 => invalid.components[0].source_ref = "".into(),
            3 => invalid.components[0].traceability_ref = "x".repeat(513),
            4..=12 => {
                let MaterialSpecification::Cover {
                    fibres,
                    care_evidence_ref,
                } = &mut invalid.components[0].material
                else {
                    panic!("fixture")
                };
                match case {
                    4 => fibres.clear(),
                    5 => *fibres = vec![fibres[0].clone(); 17],
                    6 => fibres[0].name = "".into(),
                    7 => fibres[0].name = "n".repeat(81),
                    8 => fibres[0].basis_points = 0,
                    9 => fibres[0].basis_points = 10001,
                    10 => fibres[1].name = "  FIXTURE FIBRE A  ".into(),
                    11 => fibres[1].basis_points = 3000,
                    12 => *care_evidence_ref = "".into(),
                    _ => unreachable!(),
                }
            }
            13..=16 => {
                let MaterialSpecification::Foam {
                    density_g_per_m3,
                    firmness_evidence_ref,
                } = &mut invalid.components[1].material
                else {
                    panic!("fixture")
                };
                match case {
                    13 => *density_g_per_m3 = 0,
                    14 => *density_g_per_m3 = 1000001,
                    15 => *firmness_evidence_ref = "".into(),
                    16 => *firmness_evidence_ref = "x".repeat(513),
                    _ => unreachable!(),
                }
            }
            17 => invalid.components[1].source_ref = "x".repeat(513),
            _ => unreachable!(),
        }
        assert!(invalid.validate(&bom(true)).is_err(), "invalid case {case}");
    }
    assert!(bounded_text(&"é".repeat(512), 512));
    assert!(!bounded_text(&"é".repeat(513), 512));
}
