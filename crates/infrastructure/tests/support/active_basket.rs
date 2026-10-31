//! Contract checks for claiming an existing complete reservation, with no new stock effects.
use super::*;
use crate::MIGRATOR;
use sqlx::Executor;
use std::sync::atomic::{AtomicU64, Ordering};
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
use super::super::transaction_tests::fixture;
const GROUP: &str = "11111111111111111111111111111111";
const OWNER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn lines() -> Vec<ExistingBasketLine<'static>> {
    vec![
        ExistingBasketLine {
            sku: "SET-COVERS",
            revision: 1,
            units: 1,
        },
        ExistingBasketLine {
            sku: "SET-FILLED",
            revision: 1,
            units: 1,
        },
    ]
}
fn binding<'a>(lines: &'a [ExistingBasketLine<'a>]) -> ExistingBasketBinding<'a> {
    ExistingBasketBinding {
        id: GROUP,
        owner_id: OWNER,
        created_at: 1100,
        expires_at: 1200,
        lines,
    }
}
async fn seed(pool: &PgPool) {
    fixture::seed(pool, 4, 2).await;
    let selected = vec![
        BasketHoldLine {
            id: "00000000000000000000000000000001".into(),
            sku: "SET-COVERS".into(),
            revision: 1,
            units: 1,
        },
        BasketHoldLine {
            id: "00000000000000000000000000000002".into(),
            sku: "SET-FILLED".into(),
            revision: 1,
            units: 1,
        },
    ];
    reserve_basket(
        pool,
        &Time(AtomicU64::new(1100)),
        BasketHoldRequest {
            id: GROUP,
            owner_id: OWNER,
            created_at: 1100,
            expires_at: 1200,
            lines: &selected,
        },
    )
    .await
    .expect("complete existing scarcity");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn complete_existing_hold_requires_exact_owner_manifest_window_and_monotonic_clock(
    pool: PgPool,
) {
    seed(&pool).await;
    let before = fixture::counts(&pool).await;
    for mode in 0..17 {
        let mut lines = lines();
        match mode {
            0 => lines.clear(),
            1 => {
                lines = (0..21)
                    .map(|_| ExistingBasketLine {
                        sku: "SET-COVERS",
                        revision: 1,
                        units: 1,
                    })
                    .collect()
            }
            2 => lines[0].sku = "DEMO-COVERS",
            3 => lines[0].revision = 0,
            4 => lines[0].units = 0,
            5 => lines[0].units = 1001,
            6 => lines[1].sku = "SET-COVERS",
            7 => {
                lines.pop();
            }
            14 => lines[0].sku = "OTHER-COVER",
            15 => lines[0].revision = 2,
            16 => lines[0].units = 2,
            _ => {}
        };
        let mut b = binding(&lines);
        match mode {
            8 => b.id = "bad",
            9 => b.owner_id = "bad",
            10 => b.owner_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            11 => b.created_at = 1099,
            12 => b.expires_at = 1199,
            13 => b.id = "ffffffffffffffffffffffffffffffff",
            _ => {}
        };
        let mut tx = pool.begin().await.expect("claim");
        assert!(
            claim_active(&mut tx, &Time(AtomicU64::new(1100)), b)
                .await
                .is_err(),
            "reject mismatched contract {mode}"
        );
        tx.rollback().await.expect("rollback");
    }
    let lines = lines();
    let mut tx = pool.begin().await.expect("claim");
    let mut proof = claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
        .await
        .expect("exact positive claim on fully reserved stock");
    assert!(proof.check_at(1199).is_ok());
    assert!(matches!(
        proof.check_at(1198),
        Err(HoldWriteError::ClockUnavailable)
    ));
    assert!(proof.check_at(1200).is_err());
    tx.rollback().await.expect("release read claims");
    assert_eq!(fixture::counts(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn incomplete_or_mismatched_stored_group_members_never_grant_existing_hold_authority(
    pool: PgPool,
) {
    seed(&pool).await;
    let before = fixture::counts(&pool).await;
    pool.execute("ALTER TABLE basket_hold_groups DISABLE TRIGGER USER;ALTER TABLE basket_hold_members DISABLE TRIGGER USER;ALTER TABLE stock_holds DISABLE TRIGGER USER;ALTER TABLE stock_holds DROP CONSTRAINT stock_holds_state_check").await.expect("isolated damaged import controls");
    let group_changes = [
        "UPDATE basket_hold_groups SET sealed=false",
        "UPDATE basket_hold_groups SET owner_id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'",
        "UPDATE basket_hold_groups SET created_at=1099",
        "UPDATE basket_hold_groups SET expires_at=1199",
        "UPDATE basket_hold_groups SET members_count=3",
    ];
    let lines = lines();
    for change in group_changes {
        pool.execute(change).await.expect("damage group");
        let mut tx = pool.begin().await.expect("claim");
        assert!(
            claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
                .await
                .is_err()
        );
        tx.rollback().await.expect("rollback");
        pool.execute("UPDATE basket_hold_groups SET sealed=true,owner_id='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',created_at=1100,expires_at=1200,members_count=2").await.expect("restore exact group");
    }
    for change in [
        "UPDATE basket_hold_members SET units=2 WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET owner_id='bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET created_at=1099 WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET expires_at=1199 WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET state='unknown' WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET state='released' WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET state='consumed' WHERE sku='SET-COVERS'",
        "UPDATE stock_holds SET state='expired' WHERE sku='SET-COVERS'",
    ] {
        pool.execute(change).await.expect("damage member");
        let mut tx = pool.begin().await.expect("claim");
        assert!(
            claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
                .await
                .is_err()
        );
        tx.rollback().await.expect("rollback");
        pool.execute("UPDATE basket_hold_members SET units=1;UPDATE stock_holds SET owner_id='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',created_at=1100,expires_at=1200,state='active'").await.expect("restore exact members");
    }
    pool.execute("DELETE FROM basket_hold_members WHERE sku='SET-COVERS'")
        .await
        .expect("missing member");
    let mut tx = pool.begin().await.expect("claim");
    assert!(
        claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
            .await
            .is_err()
    );
    tx.rollback().await.expect("rollback");
    pool.execute("INSERT INTO basket_hold_members(group_id,hold_id,sku,revision,units) VALUES('11111111111111111111111111111111','00000000000000000000000000000001','SET-COVERS',1,1)").await.expect("restore member");
    let mut tx = pool.begin().await.expect("claim");
    assert!(
        claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
            .await
            .is_ok()
    );
    tx.rollback().await.expect("rollback");
    pool.execute("ALTER TABLE stock_hold_lines DISABLE TRIGGER USER;DELETE FROM stock_hold_lines WHERE hold_id='00000000000000000000000000000001'").await.expect("missing held component snapshot");
    let mut tx = pool.begin().await.expect("claim");
    assert!(
        claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
            .await
            .is_err()
    );
    tx.rollback().await.expect("rollback");
    pool.execute("INSERT INTO stock_hold_lines(hold_id,component_code,quantity) VALUES('00000000000000000000000000000001','SEAT-COVER',2)").await.expect("restore exact component snapshot");
    let mut tx = pool.begin().await.expect("claim");
    assert!(
        claim_active(&mut tx, &Time(AtomicU64::new(1100)), binding(&lines))
            .await
            .is_ok()
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(fixture::counts(&pool).await, before);
}
