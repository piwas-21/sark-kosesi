// Synthetic isolated candidates for served-HTML acceptance. Never actual merchant evidence.
#[allow(dead_code, unused_imports)]
mod accepted {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}
use accepted::*;
pub async fn seed(pool: &PgPool) {
    let _case = media_seed(pool).await;
    // Browser requests use the actual system clock. Perpetual TEST permission
    // avoids fabricating an expired-clock override in the application runtime.
    rights(
        pool,
        1,
        &MediaRightsChange::Grant {
            use_scope: MediaUse::ProductPhotography,
            valid_from: 1100,
            term: RightsTerm::Perpetual,
            permission_ref: "isolated synthetic permission; no actual photo acceptance".into(),
        },
    )
    .await;
    definition(
        pool,
        &CatalogueCommand::Variant {
            sku: "SET-COVERS".into(),
            family_code: "ROOM".into(),
            size_code: "isolated size".into(),
            textile_code: "isolated textile".into(),
            fill: Fill::CoversOnly,
            lines: vec![BomLineDraft {
                component_code: "SEAT-COVER".into(),
                quantity: 2,
            }],
        },
        "covers-variant",
    )
    .await;
    let mut spec = draft(false);
    spec.components[0].component_code = "SEAT-COVER".into();
    record_specification(
        pool,
        &author(),
        &FixedClock(1100),
        SpecificationRequest {
            sku: "SET-COVERS",
            bom_revision: 1,
            expected_revision: 0,
            effect_key: "covers-spec",
            draft: &spec,
            reason: "synthetic browser data",
            confirmed: true,
        },
    )
    .await
    .expect("covers spec");
    record_price_revision(
        pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "SET-COVERS",
            expected_revision: 0,
            effect_key: "covers-price",
            price: commerce_domain::Money::eur(9876).expect("synthetic price"),
            evidence_ref: "synthetic price",
            reason: "synthetic browser data",
            confirmed: true,
        },
    )
    .await
    .expect("covers price");
    for sku in ["SET-COVERS", "SET-FILLED"] {
        let mut selection = selected();
        selection.image.rights_revision = 2;
        record_factual_review(
            pool,
            &reviewer(),
            &FixedClock(1100),
            FactReviewRequest {
                sku,
                target: selection.facts,
                expected_review_revision: 0,
                effect_key: &format!("facts-{sku}"),
                decision: FactDecision::Approved,
                evidence_ref: "synthetic workflow exercise",
                qualification_ref: "test attestation, not qualified human acceptance",
                reason: "synthetic",
                confirmed: true,
            },
        )
        .await
        .expect("facts review");
        record_image_review(
            pool,
            &reviewer(),
            &FixedClock(1100),
            ImageReviewRequest {
                sku,
                target: &selection.image,
                expected_revision: 0,
                effect_key: &format!("image-{sku}"),
                decision: ImageDecision::Approved,
                evidence_ref: "synthetic image workflow exercise",
                qualification_ref: "test attestation, not photo suitability",
                reason: "synthetic",
                confirmed: true,
            },
        )
        .await
        .expect("image review");
        for locale in [Locale::Nl, Locale::Ar] {
            let title = if locale == Locale::Nl {
                format!("Testconfiguratie {sku}")
            } else {
                format!("عينة اختبار {sku}")
            };
            let copy = if locale == Locale::Nl {
                TranslationDraft {
                    title,
                    description:
                        "Geïsoleerde testgegevens. Dit is geen echt product en niet te koop.".into(),
                    materials: "Synthetische materiaalspecificatie voor een test.".into(),
                    care: "Testtekst voor onderhoud; geen gebruiksinstructie.".into(),
                    safety: "Testtekst; geen veiligheidsadvies.".into(),
                    assembly: "Testtekst voor montage; geen gebruiksinstructie.".into(),
                }
            } else {
                TranslationDraft {
                    title,
                    description: "بيانات اختبار معزولة. هذا ليس منتجًا حقيقيًا ولا يُباع.".into(),
                    materials: "مواصفات مواد اصطناعية للاختبار.".into(),
                    care: "نص اختبار للعناية، وليس تعليمات للاستخدام.".into(),
                    safety: "نص اختبار، وليس نصيحة سلامة.".into(),
                    assembly: "نص اختبار للتركيب، وليس تعليمات للاستخدام.".into(),
                }
            };
            let source_revision = if locale == Locale::Nl { None } else { Some(1) };
            record_translation(
                pool,
                &author(),
                &FixedClock(1100),
                TranslationRequest {
                    sku,
                    locale,
                    specification_revision: 1,
                    source_revision,
                    expected_revision: 0,
                    effect_key: &format!("copy-{}-{sku}", locale.internal()),
                    draft: &copy,
                    reason: "synthetic",
                    confirmed: true,
                },
            )
            .await
            .expect("copy draft");
            record_review(
                pool,
                &reviewer(),
                &FixedClock(1100),
                TranslationReviewRequest {
                    sku,
                    locale,
                    translation_revision: 1,
                    expected_review_revision: 0,
                    effect_key: &format!("copy-review-{}-{sku}", locale.internal()),
                    decision: TranslationDecision::Approved,
                    evidence_ref: "synthetic workflow exercise",
                    qualification_ref: "test attestation, not native-reader acceptance",
                    reason: "synthetic",
                    confirmed: true,
                },
            )
            .await
            .expect("copy review");
            selection.source_review_revision = source_revision;
            selection.family_slug = if locale == Locale::Nl {
                "isolated-room"
            } else {
                "isolated-ar-room"
            }
            .into();
            record_publication(
                pool,
                &author(),
                &FixedClock(1100),
                PublicationRequest {
                    sku,
                    locale,
                    expected_revision: 0,
                    effect_key: &format!("publication-{}-{sku}", locale.internal()),
                    choice: &PublicationChoice::Activate {
                        selection: selection.clone(),
                    },
                    evidence_ref: "synthetic non-selling browser exercise",
                    reason: "synthetic",
                    confirmed: true,
                },
            )
            .await
            .expect("activation");
        }
    }
    sqlx::query("INSERT INTO stock_balances(component_code,on_hand) VALUES('SEAT-COVER',2)")
        .execute(pool)
        .await
        .expect("isolated physical cover stock");
}
