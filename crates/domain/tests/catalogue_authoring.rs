use commerce_domain::{ComponentKind, catalogue_authoring::*};
fn line(code: &str, quantity: u32) -> BomLineDraft {
    BomLineDraft {
        component_code: code.into(),
        quantity,
    }
}
fn variant() -> CatalogueCommand {
    CatalogueCommand::Variant {
        sku: "SET-A".into(),
        family_code: "FAMILY-A".into(),
        size_code: "fixture size".into(),
        textile_code: "fixture textile".into(),
        fill: Fill::WithFoam,
        lines: vec![line("FOAM", 2), line("COVER", 2)],
    }
}
#[test]
fn company_codes_are_bounded_and_cannot_promote_the_fixture_namespace() {
    for value in ["A", "0", "SET-A", "COMPONENT-2", &"X".repeat(65)] {
        assert!(merchant_code(value), "{value}");
    }
    for value in [
        "",
        "DEMO",
        "DEMO-ONE",
        "lower",
        "UP_PER",
        "/",
        "-LEFT",
        "RIGHT-",
        "é",
        &"X".repeat(66),
    ] {
        assert!(!merchant_code(value), "{value}");
    }
    assert_eq!(ComponentKind::Cover.code(), "cover");
    assert_eq!(ComponentKind::Foam.code(), "foam");
    assert_eq!(Fill::CoversOnly.code(), "covers-only");
    assert_eq!(Fill::WithFoam.code(), "with-foam");
}
#[test]
fn commands_have_explicit_bounds_and_order_independent_bom_intent() {
    for command in [
        CatalogueCommand::Family {
            code: "FAMILY-A".into(),
        },
        CatalogueCommand::Component {
            code: "COVER".into(),
            component_kind: ComponentKind::Cover,
        },
    ] {
        assert_eq!(command.normalized().expect("valid definition"), command);
        assert_eq!(command.bom_revision(), None);
    }
    let normalized = variant().normalized().expect("variant");
    assert_eq!(normalized.bom_revision(), Some(1));
    let CatalogueCommand::Variant { lines, .. } = &normalized else {
        panic!("variant");
    };
    assert_eq!(
        lines
            .iter()
            .map(|line| line.component_code.as_str())
            .collect::<Vec<_>>(),
        ["COVER", "FOAM"]
    );
    assert_eq!(
        normalized.normalized().expect("stable normalization"),
        normalized
    );
    for case in 0..12 {
        let mut command = variant();
        let CatalogueCommand::Variant {
            sku,
            family_code,
            size_code,
            textile_code,
            lines,
            ..
        } = &mut command
        else {
            panic!("variant");
        };
        match case {
            0 => sku.clear(),
            1 => *family_code = "DEMO-FAMILY".into(),
            2 => *size_code = " ".into(),
            3 => *size_code = "x".repeat(81),
            4 => *size_code = "null\0size".into(),
            5 => *textile_code = " ".into(),
            6 => *textile_code = "x".repeat(81),
            7 => *textile_code = "null\0textile".into(),
            8 => lines.clear(),
            9 => *lines = (0..65).map(|n| line(&format!("COMP-{n}"), 1)).collect(),
            10 => *lines = vec![line("COVER", 1), line("COVER", 2)],
            11 => lines[0].component_code = "DEMO-FOAM".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            command.normalized(),
            Err(InvalidDefinition),
            "variant boundary {case}"
        );
    }
    for quantity in [0, 10001] {
        let command = CatalogueCommand::Bom {
            sku: "SET-A".into(),
            expected_revision: 1,
            lines: vec![line("COVER", quantity)],
        };
        assert_eq!(command.normalized(), Err(InvalidDefinition));
    }
    for revision in [0, -1, i32::MAX] {
        let command = CatalogueCommand::Bom {
            sku: "SET-A".into(),
            expected_revision: revision,
            lines: vec![line("COVER", 1)],
        };
        assert_eq!(command.normalized(), Err(InvalidDefinition));
    }
    let invalid = CatalogueCommand::Bom {
        sku: "DEMO-SET".into(),
        expected_revision: 1,
        lines: vec![line("COVER", 1)],
    };
    assert_eq!(invalid.normalized(), Err(InvalidDefinition));
    let empty = CatalogueCommand::Bom {
        sku: "SET-A".into(),
        expected_revision: 1,
        lines: vec![],
    };
    assert_eq!(empty.normalized(), Err(InvalidDefinition));
    let bom = CatalogueCommand::Bom {
        sku: "SET-A".into(),
        expected_revision: 1,
        lines: vec![line("FOAM", 10000), line("COVER", 1)],
    };
    assert_eq!(
        bom.normalized().expect("bounded BOM").bom_revision(),
        Some(2)
    );
    let last = CatalogueCommand::Bom {
        sku: "SET-A".into(),
        expected_revision: i32::MAX - 1,
        lines: (0..64).map(|n| line(&format!("COMP-{n}"), 1)).collect(),
    };
    assert_eq!(
        last.normalized().expect("last ordinal").bom_revision(),
        Some(i32::MAX)
    );
    let mut unicode = variant();
    let CatalogueCommand::Variant {
        size_code,
        textile_code,
        ..
    } = &mut unicode
    else {
        panic!("variant");
    };
    *size_code = "é".repeat(80);
    *textile_code = "ع".repeat(80);
    assert!(unicode.validate().is_ok());
}
#[test]
fn filled_and_covers_only_configs_require_actual_cover_and_optional_foam_kinds() {
    assert!(!matches_fill(Fill::CoversOnly, &[]));
    assert!(!matches_fill(Fill::WithFoam, &[ComponentKind::Foam]));
    assert!(matches_fill(Fill::CoversOnly, &[ComponentKind::Cover]));
    assert!(!matches_fill(Fill::WithFoam, &[ComponentKind::Cover]));
    assert!(!matches_fill(
        Fill::CoversOnly,
        &[ComponentKind::Cover, ComponentKind::Foam]
    ));
    assert!(matches_fill(
        Fill::WithFoam,
        &[ComponentKind::Cover, ComponentKind::Foam]
    ));
}
