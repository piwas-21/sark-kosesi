// Invented isolated measurement/material data. Never supplier or merchant evidence.
use commerce_domain::specifications::*;
fn draft(filled: bool) -> SpecificationDraft {
    let dimensions = DimensionsMm {
        width_mm: 500,
        depth_mm: 500,
        height_mm: 100,
        tolerance_mm: 5,
    };
    let mut components = vec![ComponentSpecification {
        component_code: "DEMO-COVER".into(),
        dimensions: dimensions.clone(),
        material: MaterialSpecification::Cover {
            fibres: vec![
                FibrePart {
                    name: "fixture fibre A".into(),
                    basis_points: 6000,
                },
                FibrePart {
                    name: "fixture fibre B".into(),
                    basis_points: 4000,
                },
            ],
            care_evidence_ref: "fixture-care".into(),
        },
        source_ref: "fixture-supplier-cover".into(),
        traceability_ref: "fixture-cover-trace".into(),
    }];
    if filled {
        components.push(ComponentSpecification {
            component_code: "DEMO-FOAM".into(),
            dimensions: dimensions.clone(),
            material: MaterialSpecification::Foam {
                density_g_per_m3: 35000,
                firmness_evidence_ref: "fixture-foam-firmness".into(),
            },
            source_ref: "fixture-supplier-foam".into(),
            traceability_ref: "fixture-foam-trace".into(),
        });
    }
    SpecificationDraft {
        footprint: DimensionsMm {
            width_mm: 1000,
            depth_mm: 500,
            height_mm: 100,
            tolerance_mm: 5,
        },
        assembly_evidence_ref: "fixture-assembly-measurement".into(),
        components,
        parcels: vec![ParcelMeasurements {
            dimensions,
            gross_weight_g: 3000,
        }],
        packaging_evidence_ref: "fixture-parcel-measurement".into(),
    }
}
