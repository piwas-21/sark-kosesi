//! Failures after real stage writes cannot leak receipt, claim, group or stock effects.
use super::*;
use commerce_application::request_context::{ContextError, Identifiers};
use std::sync::atomic::AtomicU64;
struct StopIds {
    calls: AtomicU64,
    fail: u64,
}
impl Identifiers for StopIds {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail {
            Err(ContextError::EntropyUnavailable)
        } else {
            Ok(u128::from(0xde000000 + call).to_be_bytes())
        }
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_entropy_failure_at_group_each_child_receipt_or_final_event_rolls_back_all_effects(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    for fail in 1..=5 {
        let ids = StopIds {
            calls: AtomicU64::new(0),
            fail,
        };
        assert_eq!(
            recover(&pool, f.time.as_ref(), &ids, &f.order.identity.id)
                .await
                .err(),
            Some(LateStockError::EntropyUnavailable)
        );
        assert_eq!(
            all_effects(&pool).await,
            before,
            "stage ID failure must roll back all physical and canonical work"
        );
    }
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("healthy final event ID retry")
            .replayed
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_real_failure_after_expiry_reservation_and_first_consumed_member_rolls_back_every_phase(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let before = all_effects(&pool).await;
    for phase in ["expired", "active", "consumed"] {
        let body = match phase {
            "expired" => {
                "IF NEW.state='expired' AND NEW.sku='SET-FILLED' THEN IF (SELECT count(*) FROM stock_movements WHERE action='expire-hold')<>3 THEN RAISE EXCEPTION 'Fixture did not observe full original expiry'; END IF; PERFORM pg_advisory_xact_lock(938006); RAISE EXCEPTION 'Injected after actual original expiry'; END IF;"
            }
            "active" => {
                "IF NEW.state='active' AND NEW.sku='SET-FILLED' THEN IF (SELECT count(*) FROM stock_movements WHERE action='reserve')<>6 OR (SELECT count(*) FROM stock_holds WHERE state='expired')<>2 THEN RAISE EXCEPTION 'Fixture did not observe complete fresh reserves'; END IF; PERFORM pg_advisory_xact_lock(938006); RAISE EXCEPTION 'Injected after actual fresh reservation'; END IF;"
            }
            "consumed" => {
                "IF NEW.state='consumed' AND NEW.sku='SET-FILLED' THEN IF (SELECT count(*) FROM stock_movements WHERE action='consume-hold')<>3 OR (SELECT count(*) FROM stock_holds WHERE state='consumed')<>1 THEN RAISE EXCEPTION 'Fixture did not observe actual first member'; END IF; PERFORM pg_advisory_xact_lock(938006); RAISE EXCEPTION 'Injected after actual first consumed member'; END IF;"
            }
            _ => unreachable!("closed phases"),
        };
        let sql = format!(
            "CREATE FUNCTION fixture_late_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN {body} RETURN NEW; END $$; CREATE TRIGGER zz_fixture_late_failure BEFORE UPDATE ON stock_holds FOR EACH ROW EXECUTE FUNCTION fixture_late_failure()"
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&pool)
            .await
            .expect("test-owned calibrated stage failure instrument");
        let mut gate = pool.begin().await.expect("stage instrument gate");
        gate.execute("SELECT pg_advisory_xact_lock(938006)")
            .await
            .expect("hold after instrument actually saw stage writes");
        let p = pool.clone();
        let t = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        let task = tokio::spawn(async move { recover(&p, t.as_ref(), ids.as_ref(), &order).await });
        wait_for(&pool, "UPDATE stock_holds SET state%", 1).await;
        assert_eq!(
            all_effects(&pool).await,
            before,
            "all actual stage writes remain transaction-private"
        );
        gate.commit().await.expect("release calibrated failure");
        assert_eq!(
            task.await.expect("stage task").err(),
            Some(LateStockError::Database)
        );
        assert_eq!(all_effects(&pool).await, before);
        pool.execute("DROP TRIGGER zz_fixture_late_failure ON stock_holds; DROP FUNCTION fixture_late_failure()").await.expect("remove only fixture instrument");
        let candidate = super::super::native::candidate(&pool, &f).await;
        super::super::native::healthy_probe(&pool, &candidate).await;
    }
    assert!(
        !recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("same healthy operation after all failure probes")
            .replayed
    );
}
