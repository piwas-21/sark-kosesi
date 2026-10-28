// Invented isolated metadata/permission assertions; no real image or licence proof.
use commerce_domain::media::*;
fn photo() -> MediaAssetDraft {
    MediaAssetDraft {
        sha256: [0xab; 32],
        image_type: ImageType::Jpeg,
        width_px: 1200,
        height_px: 800,
        kind: MediaKind::OriginalPhoto,
        fixture: false,
        source_ref: "isolated fixture photo provenance".into(),
    }
}
fn grant() -> MediaRightsChange {
    MediaRightsChange::Grant {
        use_scope: MediaUse::ProductPhotography,
        valid_from: 1100,
        term: RightsTerm::Until { expires_at: 1300 },
        permission_ref: "isolated fixture permission assertion".into(),
    }
}
