// Synthetic acceptance setup. Never real supplier, native-review or launch evidence.
// Isolated synthetic acceptance data, never real product or launch evidence.
pub(super) use commerce_application::{
    Locale,
    request_context::{Clock, ContextError},
};
pub(super) use commerce_domain::{
    ComponentKind,
    catalogue_authoring::{BomLineDraft, CatalogueCommand, Fill},
    factual_review::{FactDecision, FactTarget},
    image_review::{ImageDecision, ImageTarget},
    media::{ImageType, MediaAssetDraft, MediaKind, MediaRightsChange, MediaUse, RightsTerm},
    publication::{ApprovedSelection, PublicationChoice},
    translations::{TranslationDecision, TranslationDraft},
};
pub(super) use commerce_infrastructure::{
    MIGRATOR,
    catalogue_authoring::{DefinitionRequest, record_definition},
    catalogue_prices::{PriceRevisionRequest, record_price_revision},
    factual_review::{FactReviewRequest, record_factual_review},
    image_ingestion::{IngestImageRequest, ingest_image},
    image_review::{ImageReviewRequest, record_image_review},
    media::{MediaRightsRequest, RegisterMediaRequest, record_media_rights, register_media},
    publication::*,
    specifications::{SpecificationRequest, record_specification},
    staff_access::{SessionToken, StaffError},
    translations::{
        TranslationRequest, TranslationReviewRequest, record_review, record_translation,
    },
};
pub(super) use sha2::{Digest, Sha256};
pub(super) use sqlx::{Executor, PgPool};
pub(super) use std::{
    io::Cursor,
    sync::atomic::{AtomicUsize, Ordering},
};
// Invented isolated measurement/material data. Never supplier or merchant evidence.
pub(super) use commerce_domain::specifications::*;
pub(super) fn draft(filled: bool) -> SpecificationDraft {
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

pub(super) async fn definition(pool: &PgPool, command: &CatalogueCommand, key: &str) {
    record_definition(
        pool,
        &author(),
        &FixedClock(1100),
        DefinitionRequest {
            command,
            effect_key: key,
            source_ref: "isolated synthetic source",
            reason: "isolated graph",
            confirmed: true,
        },
    )
    .await
    .expect("definition");
}
pub(super) async fn seed(pool: &PgPool) {
    pool.execute(include_str!("bom.sql"))
        .await
        .expect("legacy fixtures");
    pool.execute("INSERT INTO staff_accounts VALUES('author',true),('reviewer',true),('other',true),('warehouse',true); INSERT INTO staff_capabilities VALUES('author','catalogue.manage'),('reviewer','catalogue.review'),('other','catalogue.review'),('warehouse','warehouse.manage')").await.expect("isolated grants");
    for (number, id) in [
        (1, "author"),
        (2, "reviewer"),
        (3, "other"),
        (4, "warehouse"),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1100)").bind(SessionToken::from_bytes([number;32]).digest().as_slice()).bind(id).execute(pool).await.expect("isolated sessions");
    }
    definition(
        pool,
        &CatalogueCommand::Family {
            code: "ROOM".into(),
        },
        "family",
    )
    .await;
    for (code, kind) in [
        ("SEAT-COVER", ComponentKind::Cover),
        ("SEAT-FOAM", ComponentKind::Foam),
    ] {
        definition(
            pool,
            &CatalogueCommand::Component {
                code: code.into(),
                component_kind: kind,
            },
            code,
        )
        .await;
    }
    definition(
        pool,
        &CatalogueCommand::Variant {
            sku: "SET-FILLED".into(),
            family_code: "ROOM".into(),
            size_code: "isolated size".into(),
            textile_code: "isolated textile".into(),
            fill: Fill::WithFoam,
            lines: lines(2),
        },
        "variant",
    )
    .await;
    specification(pool, 0, 1, "spec-1").await;
    price(pool, 0, "price-1").await;
}
pub(super) fn lines(quantity: u32) -> Vec<BomLineDraft> {
    vec![
        BomLineDraft {
            component_code: "SEAT-COVER".into(),
            quantity,
        },
        BomLineDraft {
            component_code: "SEAT-FOAM".into(),
            quantity,
        },
    ]
}
pub(super) async fn specification(pool: &PgPool, expected: i32, bom_revision: i32, key: &str) {
    let mut value = draft(true);
    value.components[0].component_code = "SEAT-COVER".into();
    value.components[1].component_code = "SEAT-FOAM".into();
    record_specification(
        pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku: "SET-FILLED",
            bom_revision,
            expected_revision: expected,
            effect_key: key,
            draft: &value,
            reason: "isolated specification",
            confirmed: true,
        },
    )
    .await
    .expect("specification");
}
pub(super) async fn price(pool: &PgPool, expected: i32, key: &str) {
    record_price_revision(
        pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "SET-FILLED",
            expected_revision: expected,
            effect_key: key,
            price: commerce_domain::Money::eur(12345).expect("fixture price"),
            evidence_ref: "isolated price evidence",
            reason: "isolated price",
            confirmed: true,
        },
    )
    .await
    .expect("price");
}

static SEEDS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
// Retain this guard for the whole case: byte inspection shares the decoder budget.
pub(super) async fn media_seed(pool: &PgPool) -> tokio::sync::MutexGuard<'static, ()> {
    let case = SEEDS.lock().await;
    seed(pool).await;
    let mut bytes = Vec::new();
    image::DynamicImage::new_rgb8(32, 24)
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .expect("synthetic bytes");
    let metadata = MediaAssetDraft {
        sha256: Sha256::digest(&bytes).into(),
        image_type: ImageType::Jpeg,
        width_px: 32,
        height_px: 24,
        kind: MediaKind::OriginalPhoto,
        fixture: false,
        source_ref: "isolated synthetic candidate".into(),
    };
    register_media(
        pool,
        &author(),
        &FixedClock(1100),
        RegisterMediaRequest {
            asset_id: "photo",
            effect_key: "photo",
            draft: &metadata,
            reason: "synthetic registry",
            confirmed: true,
        },
    )
    .await
    .expect("register");
    ingest_image(
        pool,
        &author(),
        &FixedClock(1100),
        IngestImageRequest {
            asset_id: "photo",
            effect_key: "ingest",
            bytes: &bytes,
            reason: "synthetic ingestion",
            confirmed: true,
        },
    )
    .await
    .expect("ingest");
    rights(
        pool,
        0,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Until { expires_at: 2000 },
            permission_ref: "synthetic permission".into(),
        },
    )
    .await;
    case
}
pub(super) async fn rights(pool: &PgPool, expected: i32, change: &MediaRightsChange) {
    record_media_rights(
        pool,
        &author(),
        &FixedClock(1100),
        MediaRightsRequest {
            asset_id: "photo",
            expected_revision: expected,
            effect_key: &format!("rights-{expected}"),
            change,
            reason: "synthetic rights",
            confirmed: true,
        },
    )
    .await
    .expect("rights");
}

pub(super) struct FixedClock(pub(super) u64);
impl Clock for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}
pub(super) struct StepClock {
    pub(super) calls: AtomicUsize,
    pub(super) after: usize,
    pub(super) value: Result<u64, ContextError>,
}
impl Clock for StepClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.after {
            Ok(1100)
        } else {
            self.value
        }
    }
}
pub(super) fn author() -> SessionToken {
    SessionToken::from_bytes([1; 32])
}
pub(super) fn reviewer() -> SessionToken {
    SessionToken::from_bytes([2; 32])
}
pub(super) fn selected() -> ApprovedSelection {
    ApprovedSelection {
        facts: FactTarget {
            bom_revision: 1,
            specification_revision: 1,
            price_revision: 1,
        },
        factual_review_revision: 1,
        image: ImageTarget {
            asset_id: "photo".into(),
            bom_revision: 1,
            specification_revision: 1,
            rights_revision: 1,
        },
        image_review_revision: 1,
        translation_revision: 1,
        translation_review_revision: 1,
        source_review_revision: None,
        family_slug: "isolated-room".into(),
    }
}
pub(super) fn choice() -> PublicationChoice {
    PublicationChoice::Activate {
        selection: selected(),
    }
}
pub(super) fn request<'a>(key: &'a str, choice: &'a PublicationChoice) -> PublicationRequest<'a> {
    PublicationRequest {
        sku: "SET-FILLED",
        locale: Locale::Nl,
        expected_revision: 0,
        effect_key: key,
        choice,
        evidence_ref: "isolated explicit activation",
        reason: "isolated publication",
        confirmed: true,
    }
}
pub(super) async fn facts_review(
    pool: &PgPool,
    target: FactTarget,
    expected: i32,
    decision: FactDecision,
    key: &str,
) {
    record_factual_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        FactReviewRequest {
            sku: "SET-FILLED",
            target,
            expected_review_revision: expected,
            effect_key: key,
            decision,
            evidence_ref: "isolated facts",
            qualification_ref: "isolated qualification",
            reason: "isolated review",
            confirmed: true,
        },
    )
    .await
    .expect("facts decision");
}
pub(super) async fn image_review(
    pool: &PgPool,
    target: &ImageTarget,
    expected: i32,
    decision: ImageDecision,
    key: &str,
) {
    record_image_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        ImageReviewRequest {
            sku: "SET-FILLED",
            target,
            expected_revision: expected,
            effect_key: key,
            decision,
            evidence_ref: "isolated visual comparison",
            qualification_ref: "isolated qualification",
            reason: "isolated review",
            confirmed: true,
        },
    )
    .await
    .expect("image decision");
}
pub(super) fn copy() -> TranslationDraft {
    TranslationDraft {
        title: "Isolated acceptance sample".into(),
        description: "Synthetic copy; never real supplier facts.".into(),
        materials: "Synthetic composition description".into(),
        care: "Synthetic care".into(),
        safety: "Synthetic safety".into(),
        assembly: "Synthetic assembly".into(),
    }
}
pub(super) async fn copy_draft(
    pool: &PgPool,
    locale: Locale,
    spec: i32,
    source: Option<i32>,
    expected: i32,
    key: &str,
) {
    record_translation(
        pool,
        &author(),
        &FixedClock(1100),
        TranslationRequest {
            sku: "SET-FILLED",
            locale,
            specification_revision: spec,
            source_revision: source,
            expected_revision: expected,
            effect_key: key,
            draft: &copy(),
            reason: "isolated copy",
            confirmed: true,
        },
    )
    .await
    .expect("copy draft");
}
pub(super) async fn copy_review(
    pool: &PgPool,
    locale: Locale,
    translation: i32,
    expected: i32,
    decision: TranslationDecision,
    key: &str,
) {
    record_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        TranslationReviewRequest {
            sku: "SET-FILLED",
            locale,
            translation_revision: translation,
            expected_review_revision: expected,
            effect_key: key,
            decision,
            evidence_ref: "isolated native-copy comparison",
            qualification_ref: "isolated qualification",
            reason: "isolated review",
            confirmed: true,
        },
    )
    .await
    .expect("copy review");
}
pub(super) async fn approve(pool: &PgPool) {
    facts_review(
        pool,
        selected().facts,
        0,
        FactDecision::Approved,
        "facts-approved",
    )
    .await;
    image_review(
        pool,
        &selected().image,
        0,
        ImageDecision::Approved,
        "image-approved",
    )
    .await;
    copy_draft(pool, Locale::Nl, 1, None, 0, "copy-nl").await;
    copy_review(
        pool,
        Locale::Nl,
        1,
        0,
        TranslationDecision::Approved,
        "copy-nl-approved",
    )
    .await;
}
