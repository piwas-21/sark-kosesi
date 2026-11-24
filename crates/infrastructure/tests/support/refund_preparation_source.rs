//! Authorized source reads over the exact current paid order proof.
use super::*;
use crate::{
    MIGRATOR, mollie::tests::order_stock_cases::fixture as orders, payment_attempts::recognition,
};
use sqlx::Executor;
use std::{sync::atomic::Ordering, time::Duration};

async fn staff(pool: &PgPool, id: &str, byte: u8, grant: &str) -> SessionToken {
    sqlx::query("INSERT INTO staff_accounts VALUES($1,true)")
        .bind(id)
        .execute(pool)
        .await
        .expect("isolated finance account");
    sqlx::query("INSERT INTO staff_capabilities VALUES($1,$2)")
        .bind(id)
        .bind(grant)
        .execute(pool)
        .await
        .expect("specific grant");
    let token = SessionToken::from_bytes([byte; 32]);
    sqlx::query("INSERT INTO staff_sessions VALUES($1,$2,1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .bind(id)
        .execute(pool)
        .await
        .expect("synthetic recent MFA session");
    token
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn preparation_source_tracks_only_the_exact_received_prefix(pool: PgPool) {
    let fixture = Box::pin(orders::accepted(&pool, false, false)).await;
    let order = &fixture.order.identity.id;
    let attempt = &fixture.frozen.snapshot().binding.id;
    let (reserver, proposer) = (
        staff(&pool, "source-reserver", 75, "refund.prepare").await,
        staff(&pool, "source-proposer", 76, "refund.execution.prepare").await,
    );
    let clock = fixture.time.as_ref();
    let read = |token, phase| {
        Box::pin(inspect_preparation_source(
            &pool, token, clock, order, phase,
        ))
    };
    for (token, phase) in [
        (&reserver, PreparationPhase::Reservation),
        (&proposer, PreparationPhase::Proposal),
    ] {
        assert_eq!(read(token, phase).await.expect("unreceived order"), None);
    }
    assert_eq!(
        read(&reserver, PreparationPhase::Proposal).await,
        Err(RefundError::Denied)
    );
    assert_eq!(
        read(&proposer, PreparationPhase::Reservation).await,
        Err(RefundError::Denied)
    );

    for (at, generation, effect) in [(1510, 1, "first-source"), (1520, 2, "next-source")] {
        fixture.time.0.store(at, Ordering::SeqCst);
        Box::pin(orders::observe(
            &pool,
            &fixture,
            "tr_REFUNDSOURCE",
            "paid",
            effect,
        ))
        .await;
        assert_eq!(
            read(&proposer, PreparationPhase::Proposal)
                .await
                .expect("no current recognition"),
            None
        );
        let recorded = Box::pin(recognition::recognize(
            &pool,
            clock,
            fixture.ids.as_ref(),
            attempt,
        ))
        .await
        .expect("actual complete received prefix")
        .recognition;
        let before = orders::footprint(&pool).await;
        for (token, phase, through) in [
            (&reserver, PreparationPhase::Reservation, None),
            (&proposer, PreparationPhase::Proposal, Some(generation)),
        ] {
            assert_eq!(
                read(token, phase).await.expect("exact current proof"),
                Some(PreparationSource {
                    order_id: order.clone(),
                    recognition_id: recorded.id.clone(),
                    through_observation: through,
                })
            );
        }
        assert_eq!(orders::footprint(&pool).await, before);
        fixture.time.0.store(at - 1, Ordering::SeqCst);
        assert_eq!(
            read(&proposer, PreparationPhase::Proposal).await,
            Err(RefundError::ClockUnavailable)
        );
    }
    fixture.time.0.store(1520, Ordering::SeqCst);
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='payment.recognized.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("disposable damaged recognition source");
    assert_eq!(
        read(&proposer, PreparationPhase::Proposal).await,
        Err(RefundError::InvalidStoredData)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn preparation_source_fences_staff_freshness_after_waiting_for_attempt(pool: PgPool) {
    let fixture = Box::pin(orders::accepted(&pool, false, true)).await;
    let order = fixture.order.identity.id.clone();
    let attempt = &fixture.frozen.snapshot().binding.id;
    let token = staff(
        &pool,
        "waiting-source-proposer",
        77,
        "refund.execution.prepare",
    )
    .await;
    let before = orders::footprint(&pool).await;
    let mut blocker = pool.begin().await.expect("isolated attempt blocker");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("blocker process ID");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("claim exact attempt");
    let reader = {
        let pool = pool.clone();
        let clock = fixture.time.clone();
        let order = order.clone();
        tokio::spawn(async move {
            Box::pin(inspect_preparation_source(
                &pool,
                &token,
                clock.as_ref(),
                &order,
                PreparationPhase::Proposal,
            ))
            .await
        })
    };
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&pool).await.expect("actual row wait");
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("source reader waits on the original attempt");
    fixture.time.0.store(2301, Ordering::SeqCst);
    blocker.rollback().await.expect("release exact attempt");
    assert_eq!(
        reader.await.expect("source reader task"),
        Err(RefundError::Denied)
    );
    assert_eq!(orders::footprint(&pool).await, before);
    fixture.time.0.store(1500, Ordering::SeqCst);
    let token = SessionToken::from_bytes([77; 32]);
    assert!(
        Box::pin(inspect_preparation_source(
            &pool,
            &token,
            fixture.time.as_ref(),
            &order,
            PreparationPhase::Proposal
        ))
        .await
        .expect("fresh authorized control")
        .is_some()
    );
}
