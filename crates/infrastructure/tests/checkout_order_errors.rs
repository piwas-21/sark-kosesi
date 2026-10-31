//! Closed diagnostic boundary: failures never retain embedded private dependency data.
use commerce_domain::{checkout_quotes::SnapshotError, quotes::QuoteError};
use commerce_infrastructure::{
    checkout_orders::OrderStoreError as Safe, checkout_quotes::QuoteStoreError,
    guest_baskets::GuestError, holds::HoldWriteError, market_policies::MarketError,
    quotes::QuoteFailure,
};
fn private_error() -> sqlx::Error {
    sqlx::Error::Protocol("buyer@example.test Synthetic Street 22 credential-secret".into())
}
#[test]
fn order_failure_diagnostics_are_closed_and_discard_private_nested_dependency_details() {
    let mut errors = vec![Safe::from(private_error())];
    for input in [
        GuestError::InvalidRequest,
        GuestError::Denied,
        GuestError::Csrf,
        GuestError::Conflict,
        GuestError::RateLimited,
        GuestError::ClockUnavailable,
        GuestError::EntropyUnavailable,
        GuestError::InvalidStoredData,
        GuestError::SessionUnavailable,
        GuestError::Database(private_error()),
    ] {
        errors.push(input.into());
    }
    for input in [
        QuoteStoreError::NotFound,
        QuoteStoreError::Expired,
        QuoteStoreError::ClockUnavailable,
        QuoteStoreError::Guest(GuestError::Database(private_error())),
        QuoteStoreError::Database(private_error()),
        QuoteStoreError::InvalidStoredData,
        QuoteStoreError::Market(MarketError::Database(private_error())),
        QuoteStoreError::Evaluation(QuoteFailure::Database(private_error())),
    ] {
        errors.push(input.into());
    }
    for input in [
        SnapshotError::Expired,
        SnapshotError::InvalidClock,
        SnapshotError::InvalidSnapshot,
    ] {
        errors.push(input.into());
    }
    for input in [
        QuoteFailure::ClockUnavailable,
        QuoteFailure::Database(private_error()),
        QuoteFailure::InvalidStoredData,
        QuoteFailure::Policy(QuoteError::InsufficientStock),
    ] {
        errors.push(input.into());
    }
    for input in [
        MarketError::Database(private_error()),
        MarketError::InvalidStoredData,
        MarketError::NotFound,
    ] {
        errors.push(input.into());
    }
    for input in [
        HoldWriteError::ClockUnavailable,
        HoldWriteError::Database(private_error()),
        HoldWriteError::IdempotencyConflict,
        HoldWriteError::MissingBom,
    ] {
        errors.push(input.into());
    }
    for error in errors {
        let message = format!("{error:?}");
        assert!(
            [
                "InvalidRequest",
                "Denied",
                "Csrf",
                "Conflict",
                "RateLimited",
                "NotFound",
                "QuoteNotFound",
                "QuoteExpired",
                "QuoteChanged",
                "Unavailable",
                "InvalidStoredData",
                "ClockUnavailable",
                "EntropyUnavailable",
                "Database"
            ]
            .contains(&message.as_str())
        );
        for private in [
            "buyer@example.test",
            "Synthetic Street",
            "credential-secret",
            "Protocol",
        ] {
            assert!(!message.contains(private));
        }
    }
    // The instrument is meaningful: its unredacted input does contain the private marker.
    assert!(format!("{:?}", private_error()).contains("buyer@example.test"));
}
