use super::*;
#[test]
fn static_error_classifications_are_redacted_and_quota_has_retry_after() {
    for (value, status, code) in [
        (
            OrderStoreError::InvalidRequest,
            StatusCode::BAD_REQUEST,
            "invalid_order_request",
        ),
        (
            OrderStoreError::Denied,
            StatusCode::UNAUTHORIZED,
            "guest_session_required",
        ),
        (
            OrderStoreError::Csrf,
            StatusCode::FORBIDDEN,
            "guest_csrf_required",
        ),
        (
            OrderStoreError::Conflict,
            StatusCode::CONFLICT,
            "order_unavailable",
        ),
        (
            OrderStoreError::QuoteChanged,
            StatusCode::CONFLICT,
            "order_unavailable",
        ),
        (
            OrderStoreError::Unavailable,
            StatusCode::CONFLICT,
            "order_unavailable",
        ),
        (
            OrderStoreError::QuoteNotFound,
            StatusCode::NOT_FOUND,
            "order_source_not_found",
        ),
        (
            OrderStoreError::NotFound,
            StatusCode::NOT_FOUND,
            "order_source_not_found",
        ),
        (
            OrderStoreError::QuoteExpired,
            StatusCode::GONE,
            "order_quote_expired",
        ),
        (
            OrderStoreError::RateLimited,
            StatusCode::TOO_MANY_REQUESTS,
            "order_rate_limited",
        ),
        (
            OrderStoreError::Database,
            StatusCode::SERVICE_UNAVAILABLE,
            "order_service_unavailable",
        ),
        (
            OrderStoreError::InvalidStoredData,
            StatusCode::SERVICE_UNAVAILABLE,
            "order_service_unavailable",
        ),
        (
            OrderStoreError::ClockUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "order_service_unavailable",
        ),
        (
            OrderStoreError::EntropyUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "order_service_unavailable",
        ),
    ] {
        let response = failed(value);
        assert_eq!(response.status(), status, "{code}");
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(response.headers()["retry-after"], "60");
        } else {
            assert!(!response.headers().contains_key("retry-after"));
        }
    }
}
