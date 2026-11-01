//! Typed boundary failures keep static classifications and discard private data.
use super::*;
use crate::{
    checkout_orders::OrderStoreError, holds::HoldWriteError, order_stock::SettlementError,
    payment_attempts::PaymentStoreError,
};
use commerce_domain::{holds::HoldError, inventory::StockError};
#[test]
fn late_stock_boundary_errors_are_literal_and_private_database_diagnostics_are_discarded() {
    for (input, expected) in [
        (
            PolicyError::InvalidSnapshot,
            LateStockError::InvalidStoredData,
        ),
        (PolicyError::InvalidClock, LateStockError::ClockUnavailable),
        (PolicyError::WindowExpired, LateStockError::StateChanged),
    ] {
        assert_eq!(LateStockError::from(input), expected);
    }
    for (input, expected) in [
        (
            SettlementError::InvalidRequest,
            LateStockError::InvalidRequest,
        ),
        (SettlementError::NotFound, LateStockError::NotFound),
        (SettlementError::Unavailable, LateStockError::Unavailable),
        (
            SettlementError::RecoveryRequired,
            LateStockError::NotApplicable,
        ),
        (
            SettlementError::ClockUnavailable,
            LateStockError::ClockUnavailable,
        ),
        (
            SettlementError::EntropyUnavailable,
            LateStockError::EntropyUnavailable,
        ),
        (SettlementError::Database, LateStockError::Database),
        (
            SettlementError::InvalidStoredData,
            LateStockError::InvalidStoredData,
        ),
    ] {
        assert_eq!(LateStockError::from(input), expected);
    }
    for (input, expected) in [
        (
            PaymentStoreError::InvalidRequest,
            LateStockError::InvalidRequest,
        ),
        (PaymentStoreError::NotFound, LateStockError::NotFound),
        (PaymentStoreError::Unavailable, LateStockError::Unavailable),
        (
            PaymentStoreError::ClockUnavailable,
            LateStockError::ClockUnavailable,
        ),
        (PaymentStoreError::Database, LateStockError::Database),
        (
            PaymentStoreError::Boundary(OrderStoreError::Database),
            LateStockError::Database,
        ),
        (
            PaymentStoreError::Boundary(OrderStoreError::InvalidRequest),
            LateStockError::InvalidStoredData,
        ),
        (
            PaymentStoreError::Conflict,
            LateStockError::InvalidStoredData,
        ),
        (
            PaymentStoreError::InvalidStoredData,
            LateStockError::InvalidStoredData,
        ),
    ] {
        assert_eq!(LateStockError::from(input), expected);
    }
    for (input, expected) in [
        (
            HoldWriteError::InvalidRequest,
            LateStockError::InvalidStoredData,
        ),
        (
            HoldWriteError::ClockUnavailable,
            LateStockError::ClockUnavailable,
        ),
        (
            HoldWriteError::MissingBom,
            LateStockError::InvalidStoredData,
        ),
        (HoldWriteError::NotFound, LateStockError::InvalidStoredData),
        (HoldWriteError::Ownership, LateStockError::InvalidStoredData),
        (
            HoldWriteError::IdempotencyConflict,
            LateStockError::InvalidStoredData,
        ),
        (
            HoldWriteError::Stock(StockError::InvalidBalance),
            LateStockError::InvalidStoredData,
        ),
        (
            HoldWriteError::Policy(HoldError::InvalidClock),
            LateStockError::ClockUnavailable,
        ),
        (
            HoldWriteError::Policy(HoldError::Expired),
            LateStockError::StateChanged,
        ),
        (
            HoldWriteError::Policy(HoldError::InvalidTransition),
            LateStockError::InvalidStoredData,
        ),
    ] {
        assert_eq!(LateStockError::from(input), expected);
    }
    let private = sqlx::Error::Protocol("late-fixture-private-provider-and-sql-marker".into());
    assert!(
        format!("{private:?}").contains("late-fixture-private-provider-and-sql-marker"),
        "positive leak-detector calibration"
    );
    let public = LateStockError::from(HoldWriteError::Database(private));
    assert_eq!(public, LateStockError::Database);
    assert!(!format!("{public:?}").contains("late-fixture-private-provider-and-sql-marker"));
    assert_eq!(
        LateStockError::from(sqlx::Error::Protocol(
            "late-fixture-private-provider-and-sql-marker".into()
        )),
        LateStockError::Database
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_closed_database_returns_static_error_for_recovery_and_history_without_effects(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = all_effects(&pool).await;
    let options = pool.connect_options().as_ref().clone();
    pool.close().await;
    assert_eq!(
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(LateStockError::Database)
    );
    assert_eq!(
        read_recorded(&pool, &f.order.identity.id).await.err(),
        Some(LateStockError::Database)
    );
    let probe = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("same isolated database independent verification connection");
    assert_eq!(all_effects(&probe).await, before);
    probe.close().await;
}
