include!("../../../tests/fixtures/media.rs");
#[test]
fn bounded_metadata_and_derived_private_keys_reject_untrusted_paths() {
    let original = photo();
    assert_eq!(original.validate(), Ok(()));
    for (image_type, extension) in [
        (ImageType::Jpeg, "jpeg"),
        (ImageType::Png, "png"),
        (ImageType::Webp, "webp"),
    ] {
        let mut value = original.clone();
        value.image_type = image_type;
        assert_eq!(
            value.original_object_key(),
            format!("private/catalogue/{}.{}", "ab".repeat(32), extension)
        );
    }
    for case in 0..7 {
        let mut invalid = original.clone();
        match case {
            0 => invalid.width_px = 0,
            1 => invalid.width_px = 16385,
            2 => invalid.height_px = 0,
            3 => invalid.height_px = 16385,
            4 => {
                invalid.width_px = 16384;
                invalid.height_px = 16384;
            }
            5 => invalid.source_ref = " ".into(),
            6 => invalid.source_ref = "s".repeat(513),
            _ => unreachable!(),
        }
        assert!(invalid.validate().is_err(), "case {case}");
    }
    let mut boundary = original;
    boundary.width_px = 8192;
    boundary.height_px = 8192;
    assert_eq!(boundary.validate(), Ok(()));
    for bad in ["", "a/b", "..", "name.jpeg", "é"] {
        assert!(!valid_command_id(bad));
    }
    assert!(!valid_command_id(&"x".repeat(129)));
    assert!(valid_command_id(&"x".repeat(128)));
    assert!(valid_command_id("A-01"));
}
#[test]
fn licence_scope_terms_expiry_and_revocation_are_explicit() {
    let asset = photo();
    let rights = grant();
    assert_eq!(rights.validate(&asset), Ok(()));
    for (now, active) in [(1099, false), (1100, true), (1299, true), (1300, false)] {
        assert_eq!(rights.product_rights_active(&asset, now), active);
    }
    for case in 0..8 {
        let mut asset = asset.clone();
        let mut rights = rights.clone();
        let MediaRightsChange::Grant {
            use_scope,
            valid_from,
            term,
            permission_ref,
        } = &mut rights
        else {
            panic!("fixture")
        };
        match case {
            0 => *valid_from = -1,
            1 => *permission_ref = "".into(),
            2 => *permission_ref = "x".repeat(513),
            3 => *term = RightsTerm::Until { expires_at: 1100 },
            4 => asset.fixture = true,
            5 => asset.kind = MediaKind::GeneratedInspiration,
            6 => *use_scope = MediaUse::Inspiration,
            7 => asset.width_px = 0,
            _ => unreachable!(),
        }
        assert!(!rights.product_rights_active(&asset, 1200), "case {case}");
    }
    let perpetual = MediaRightsChange::Grant {
        use_scope: MediaUse::ProductPhotography,
        valid_from: 0,
        term: RightsTerm::Perpetual,
        permission_ref: "fixture perpetual permission".into(),
    };
    assert!(perpetual.product_rights_active(&asset, i64::MAX));
    assert_eq!(MediaRightsChange::Revoke.validate(&asset), Ok(()));
    assert!(!MediaRightsChange::Revoke.product_rights_active(&asset, 1200));
}
