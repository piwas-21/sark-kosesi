async fn definition(pool: &PgPool, command: &CatalogueCommand, key: &str) {
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
async fn seed(pool: &PgPool) {
    pool.execute(include_str!("../../../../tests/fixtures/bom.sql"))
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
fn lines(quantity: u32) -> Vec<BomLineDraft> {
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
async fn specification(pool: &PgPool, expected: i32, bom_revision: i32, key: &str) {
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
async fn price(pool: &PgPool, expected: i32, key: &str) {
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
