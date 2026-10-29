use super::*;
fn asset() -> MediaAssetDraft {
    MediaAssetDraft {
        sha256: [1; 32],
        image_type: commerce_domain::media::ImageType::Jpeg,
        width_px: 32,
        height_px: 24,
        kind: commerce_domain::media::MediaKind::OriginalPhoto,
        fixture: false,
        source_ref: "ref".into(),
    }
}
fn sources() -> Sources {
    Sources {
        identity_key: "identity".into(),
        bom_key: "bom".into(),
        specification_actor: "spec".into(),
        registration_actor: "register".into(),
        ingestion_actor: "ingest".into(),
        rights_actor: "rights".into(),
        change: r#"{"action":"revoke"}"#.into(),
        sha256: vec![1; 32],
        intact: true,
    }
}
#[test]
fn stored_sources_require_hash_integrity_valid_actors_and_closed_valid_permissions() {
    assert_eq!(
        sources().rights(&asset()).expect("valid"),
        MediaRightsChange::Revoke
    );
    for field in 0..8 {
        let mut row = sources();
        match field {0=>row.intact=false,1=>row.sha256=vec![2;32],2=>row.specification_actor="bad/id".into(),3=>row.registration_actor="bad/id".into(),4=>row.ingestion_actor="bad/id".into(),5=>row.rights_actor="bad/id".into(),6=>row.change="{}".into(),_=>row.change=r#"{"action":"grant","use_scope":"product-photography","valid_from":-1,"term":{"kind":"perpetual"},"permission_ref":"ref"}"#.into()}
        assert!(
            matches!(
                row.rights(&asset()),
                Err(ImageReviewError::InvalidStoredData)
            ),
            "field {field}"
        );
    }
}
