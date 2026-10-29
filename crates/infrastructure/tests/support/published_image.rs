use super::*;
#[test]
fn decoded_output_must_match_the_selected_dimensions_and_digest() {
    let bytes = PrivateRendition {
        image_type: "jpeg".into(),
        width: 32,
        height: 24,
        bytes: vec![1, 2, 3],
    };
    let expected = PublishedRendition {
        edge: 320,
        width: 32,
        height: 24,
        sha256: Sha256::digest([1, 2, 3]).to_vec(),
    };
    check_identity(&bytes, &expected).expect("exact bound identity");
    for field in 0..3 {
        let mut changed = expected.clone();
        match field {
            0 => changed.width = 31,
            1 => changed.height = 23,
            _ => changed.sha256 = Sha256::digest([4, 5, 6]).to_vec(),
        };
        assert!(matches!(
            check_identity(&bytes, &changed),
            Err(PublishedError::InvalidStoredData)
        ));
    }
}

#[test]
fn changed_activation_or_selected_image_withholds_previously_verified_bytes() {
    let bytes = || PrivateRendition {
        image_type: "jpeg".into(),
        width: 32,
        height: 24,
        bytes: vec![1, 2, 3],
    };
    let selected = PublishedImage {
        asset_id: "PHOTO".into(),
        renditions: vec![],
    };
    assert!(finish(bytes(), 1, &selected, 1, &selected).is_some());
    assert!(finish(bytes(), 2, &selected, 1, &selected).is_none());
    let replacement = PublishedImage {
        asset_id: "REPLACEMENT".into(),
        renditions: vec![],
    };
    assert!(finish(bytes(), 1, &replacement, 1, &selected).is_none());
}
