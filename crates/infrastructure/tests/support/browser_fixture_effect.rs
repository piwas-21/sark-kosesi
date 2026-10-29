// Test-only owned browser fixture driver. Not an administrative application interface.
#[allow(dead_code, unused_imports)]
mod fixture {
    include!("../../../../tests/fixtures/approved_catalogue.rs");
}
use fixture::*;
#[tokio::main]
async fn main() {
    assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
    let token = std::env::var("PREVIEW_TOKEN").expect("owned browser guard required");
    let pool =
        PgPool::connect(&std::env::var("DATABASE_URL").expect("owned browser database required"))
            .await
            .expect("owned connection");
    let guard: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM browser_fixture_guard")
        .fetch_one(&pool)
        .await
        .expect("browser fixture guard");
    assert_eq!(
        guard,
        Sha256::digest(token).to_vec(),
        "wrong owned database"
    );
    let action = std::env::args().nth(1).expect("explicit test effect");
    match action.as_str() {
        "quarantine" | "release" => {
            use commerce_domain::inventory::WarehouseChange;
            use commerce_infrastructure::stock::{WarehouseRequest, warehouse_movement};
            let change = if action == "quarantine" {
                WarehouseChange::Quarantine(1)
            } else {
                WarehouseChange::ReleaseQuarantine(1)
            };
            warehouse_movement(
                &pool,
                WarehouseRequest {
                    effect_key: &format!("browser-{action}"),
                    component_code: "SEAT-COVER",
                    change,
                    actor: "warehouse",
                    reason: "synthetic served-projection acceptance",
                },
            )
            .await
            .expect("fixture stock effect");
        }
        "price" => {
            price_change(&pool, 1, 11111).await;
        }
        "republish" => {
            activate(&pool, 2, 1).await;
        }
        "withdraw" => {
            for locale in [Locale::Nl, Locale::Ar] {
                record_publication(
                    &pool,
                    &author(),
                    &FixedClock(1100),
                    PublicationRequest {
                        sku: "SET-COVERS",
                        locale,
                        expected_revision: 2,
                        effect_key: &format!("browser-withdraw-{}", locale.internal()),
                        choice: &PublicationChoice::Withdraw {},
                        evidence_ref: "synthetic",
                        reason: "synthetic",
                        confirmed: true,
                    },
                )
                .await
                .expect("fixture withdrawal");
            }
        }
        "restore" => {
            price_change(&pool, 2, 9876).await;
            activate(&pool, 3, 3).await;
        }
        _ => panic!("unknown explicit browser fixture effect"),
    }
    pool.close().await;
}
async fn price_change(pool: &PgPool, expected: i32, minor: i64) {
    record_price_revision(
        pool,
        &author(),
        &FixedClock(1100),
        PriceRevisionRequest {
            sku: "SET-COVERS",
            expected_revision: expected,
            effect_key: &format!("browser-price-{expected}"),
            price: commerce_domain::Money::eur(minor).expect("synthetic integer"),
            evidence_ref: "synthetic",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("fixture draft price");
}
async fn activate(pool: &PgPool, price_revision: i32, expected_publication: i32) {
    let mut selection = selected();
    selection.facts.price_revision = price_revision;
    selection.image.rights_revision = 2;
    record_factual_review(
        pool,
        &reviewer(),
        &FixedClock(1100),
        FactReviewRequest {
            sku: "SET-COVERS",
            target: selection.facts,
            expected_review_revision: 0,
            effect_key: &format!("browser-facts-{price_revision}"),
            decision: FactDecision::Approved,
            evidence_ref: "synthetic",
            qualification_ref: "test-only qualification attestation",
            reason: "synthetic",
            confirmed: true,
        },
    )
    .await
    .expect("fixture factual review");
    for locale in [Locale::Nl, Locale::Ar] {
        selection.source_review_revision = if locale == Locale::Nl { None } else { Some(1) };
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
                sku: "SET-COVERS",
                locale,
                expected_revision: expected_publication,
                effect_key: &format!("browser-publish-{price_revision}-{}", locale.internal()),
                choice: &PublicationChoice::Activate {
                    selection: selection.clone(),
                },
                evidence_ref: "synthetic",
                reason: "synthetic",
                confirmed: true,
            },
        )
        .await
        .expect("fixture explicit activation");
    }
}
