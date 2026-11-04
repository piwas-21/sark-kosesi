//! Real approved frozen BOMs retain every pair and each physical code.
use super::*;
use commerce_domain::ComponentKind;
use fixture::bounds::{accepted_lines, component, market};
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_twenty_members_by_sixty_four_lines_has_all_2560_original_fresh_pairs_and_5120_movements(
    pool: PgPool,
) {
    seed(&pool).await;
    let mut components = vec![];
    for kind in [ComponentKind::Cover, ComponentKind::Foam] {
        for n in 0..32 {
            let label = if kind == ComponentKind::Cover {
                "COVER"
            } else {
                "FOAM"
            };
            let code = format!("BOUND-{label}-{n:02}");
            component(&pool, &code, kind, 40).await;
            components.push((code, 1));
        }
    }
    let skus: Vec<_> = (0..20).map(|i| format!("LATE-SET-{i:02}")).collect();
    for sku in &skus {
        fixture::additional_variant(&pool, sku, true, &components).await;
    }
    market(&pool, &skus).await;
    let f = accepted_lines(&pool, &skus).await;
    assert_eq!(f.order.quote.total.gross_minor, 248110);
    let immutable = original_snapshot(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let Outcome::Late(late) = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("largest complete late source")
        .outcome
    else {
        panic!("maximum intact source must allocate")
    };
    assert_eq!(late.footprint.original.len(), 20);
    assert_eq!(late.footprint.fresh.len(), 20);
    assert_eq!(
        late.footprint
            .original
            .iter()
            .map(|m| m.pairs.len())
            .sum::<usize>(),
        1280
    );
    assert_eq!(
        late.footprint
            .fresh
            .iter()
            .map(|m| m.components.len())
            .sum::<usize>(),
        1280
    );
    assert_eq!(late.footprint.components.len(), 64);
    assert!(
        late.footprint
            .original
            .iter()
            .all(|m| m.member.components.len() == 64)
    );
    assert!(
        late.footprint
            .fresh
            .iter()
            .all(|m| m.components.len() == 64)
    );
    let rows:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'BOUND-%' ORDER BY component_code").fetch_all(&pool).await.expect("64 complete physical pools");
    assert_eq!(rows.len(), 64);
    assert!(rows.iter().all(|r| (r.1, r.2, r.3) == (20, 0, 0)));
    assert_eq!(
        allocation_counts(&pool).await,
        (1, 1, 0, 0, 2, 40, 1280, 1280, 2560)
    );
    assert_eq!(original_snapshot(&pool).await, immutable);
    let encoded = serde_json::to_vec(&late).expect("bounded closed maximum envelope");
    assert!(encoded.len() < 2_097_152);
    replay_without_ports(&pool, &f, Outcome::Late(late)).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_covers_only_three_physical_codes_remain_distinct_and_never_allocate_foam(
    pool: PgPool,
) {
    seed(&pool).await;
    let components: Vec<(String, u32)> = vec![
        ("BOUND-COVER-A".into(), 1),
        ("BOUND-COVER-B".into(), 2),
        ("BOUND-COVER-C".into(), 3),
    ];
    for (code, _) in &components {
        component(&pool, code, ComponentKind::Cover, 10).await;
    }
    let skus = vec!["LATE-COVERS".to_owned()];
    fixture::additional_variant(&pool, &skus[0], false, &components).await;
    market(&pool, &skus).await;
    let f = accepted_lines(&pool, &skus).await;
    expire_original(&pool, &f).await;
    let Outcome::Late(late) = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("distinct covers")
        .outcome
    else {
        panic!("intact covers allocate")
    };
    assert_eq!(late.footprint.components.len(), 3);
    let rows:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'BOUND-%' ORDER BY component_code").fetch_all(&pool).await.expect("three physical codes");
    assert_eq!(
        rows,
        vec![
            ("BOUND-COVER-A".into(), 9, 0, 0),
            ("BOUND-COVER-B".into(), 8, 0, 0),
            ("BOUND-COVER-C".into(), 7, 0, 0)
        ]
    );
    let foam: (i64, i64, i64) = sqlx::query_as(
        "SELECT on_hand,quarantine,reserved FROM stock_balances WHERE component_code='SEAT-FOAM'",
    )
    .fetch_one(&pool)
    .await
    .expect("unrelated foam");
    assert_eq!(foam, (4, 1, 0));
    let foam_moves:i64=sqlx::query_scalar("SELECT count(*) FROM stock_movements m JOIN physical_components c ON c.code=m.component_code WHERE c.kind='foam' AND m.action IN ('reserve','expire-hold','consume-hold')").fetch_one(&pool).await.expect("no foam movement");
    assert_eq!(foam_moves, 0);
    replay_without_ports(&pool, &f, Outcome::Late(late)).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_largest_twenty_by_sixty_four_distinct_union_uses_full_codes_quantities_and_two_mib_envelope(
    pool: PgPool,
) {
    seed(&pool).await;
    let mut skus = vec![];
    for member in 0..20 {
        let prefix = format!("LATE-UNION-SET-{member:02}-");
        let sku = format!("{prefix}{}", "X".repeat(65 - prefix.len()));
        assert_eq!(sku.len(), 65);
        let mut components = vec![];
        for line in 0..64 {
            let kind = if line < 32 {
                ComponentKind::Cover
            } else {
                ComponentKind::Foam
            };
            let label = if kind == ComponentKind::Cover {
                "COVER"
            } else {
                "FOAM"
            };
            let prefix = format!("UNION-{label}-{member:02}-{line:02}-");
            let code = format!("{prefix}{}", "X".repeat(65 - prefix.len()));
            assert_eq!(code.len(), 65);
            component(&pool, &code, kind, 20000).await;
            components.push((code, 10000));
        }
        fixture::additional_variant(&pool, &sku, true, &components).await;
        skus.push(sku);
    }
    market(&pool, &skus).await;
    let f = accepted_lines(&pool, &skus).await;
    assert_eq!(f.order.quote.total.gross_minor, 248110);
    let before:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'UNION-%' ORDER BY component_code").fetch_all(&pool).await.expect("1280 genuine independent component pools");
    assert_eq!(before.len(), 1280);
    assert!(before.iter().all(|r| (r.1, r.2, r.3) == (20000, 0, 10000)));
    assert_eq!(
        allocation_counts(&pool).await,
        (0, 0, 0, 0, 1, 20, 0, 0, 1280)
    );
    let source = original_snapshot(&pool).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let Outcome::Late(late) = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("actual1280union admission within existing lock and statement budgets")
        .outcome
    else {
        panic!("largest physically backed source must allocate")
    };
    assert_eq!(late.footprint.original.len(), 20);
    assert_eq!(late.footprint.fresh.len(), 20);
    assert_eq!(late.footprint.components.len(), 1280);
    assert!(
        late.footprint
            .components
            .iter()
            .all(|c| c.component_code.len() == 65
                && c.demand == 10000
                && c.active == 10000
                && c.before.on_hand == 20000
                && c.before.quarantine == 0
                && c.before.reserved == 10000
                && c.expired.reserved == 0
                && c.reserved.reserved == 10000
                && c.after.on_hand == 10000
                && c.after.reserved == 0)
    );
    assert_eq!(
        late.footprint
            .original
            .iter()
            .map(|m| m.pairs.len())
            .sum::<usize>(),
        1280
    );
    assert_eq!(
        late.footprint
            .fresh
            .iter()
            .map(|m| m.components.len())
            .sum::<usize>(),
        1280
    );
    let encoded = serde_json::to_vec(&late).expect("actual largest closed receipt");
    let bytes: i32 =
        sqlx::query_scalar("SELECT octet_length(footprint::text) FROM late_stock_allocations")
            .fetch_one(&pool)
            .await
            .expect("actual PostgreSQL envelope size");
    assert!(encoded.len() < 2_097_152);
    assert!(bytes < 2_097_152);
    eprintln!(
        "largest late allocation: compact receipt {} bytes; PostgreSQL footprint {} bytes; 1280 distinct65-byte physical codes; quantity10000; 5120 canonical movements",
        encoded.len(),
        bytes
    );
    let after:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code LIKE 'UNION-%' ORDER BY component_code").fetch_all(&pool).await.expect("full actual union final balances");
    assert_eq!(after.len(), 1280);
    assert!(after.iter().all(|r| (r.1, r.2, r.3) == (10000, 0, 0)));
    assert_eq!(
        allocation_counts(&pool).await,
        (1, 1, 0, 0, 2, 40, 1280, 1280, 2560)
    );
    assert_eq!(original_snapshot(&pool).await, source);
    let untouched:Vec<(String,i64,i64,i64)>=sqlx::query_as("SELECT component_code,on_hand,quarantine,reserved FROM stock_balances WHERE component_code NOT LIKE 'UNION-%' ORDER BY component_code").fetch_all(&pool).await.expect("unrelated physical stock");
    assert_eq!(
        untouched,
        vec![
            ("SEAT-COVER".into(), 10, 1, 0),
            ("SEAT-FOAM".into(), 4, 1, 0)
        ]
    );
    replay_without_ports(&pool, &f, Outcome::Late(late)).await;
}
