//! Error boundaries must distinguish an ordinary denial from unavailable custody
//! without carrying the underlying storage error into the public classification.
use super::*;

#[test]
fn session_guest_and_order_errors_keep_the_custody_boundary() {
    assert_eq!(LinkError::from(SessionError::Denied), LinkError::Denied);
    assert_eq!(
        LinkError::from(SessionError::Unavailable),
        LinkError::Unavailable
    );
    for error in [
        GuestError::Denied,
        GuestError::Csrf,
        GuestError::InvalidRequest,
    ] {
        assert_eq!(LinkError::from(error), LinkError::Denied);
    }
    for error in [
        GuestError::ClockUnavailable,
        GuestError::InvalidStoredData,
        GuestError::Database(sqlx::Error::RowNotFound),
    ] {
        assert_eq!(LinkError::from(error), LinkError::Unavailable);
    }
    for error in [
        checkout_orders::OrderStoreError::NotFound,
        checkout_orders::OrderStoreError::Denied,
        checkout_orders::OrderStoreError::InvalidRequest,
    ] {
        assert_eq!(order_error(error), LinkError::Denied);
    }
    for error in [
        checkout_orders::OrderStoreError::Csrf,
        checkout_orders::OrderStoreError::InvalidStoredData,
        checkout_orders::OrderStoreError::Database,
    ] {
        assert_eq!(order_error(error), LinkError::Unavailable);
    }
}
