//! Normalize the maintained middleware's deletion cookies for the __Host- prefix.
use super::Rejection;
use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode, header::SET_COOKIE},
    middleware::Next,
    response::Response,
};
use tower_sessions::cookie::{Cookie, SameSite};
fn as_header(cookie: &Cookie<'_>) -> Result<HeaderValue, Rejection> {
    HeaderValue::from_str(&cookie.to_string())
        .map_err(|_| Rejection(StatusCode::SERVICE_UNAVAILABLE, "guest_unavailable"))
}
pub(crate) async fn secure(request: Request, next: Next) -> Result<Response, Rejection> {
    let mut response = next.run(request).await;
    let mut headers = Vec::new();
    for header in response.headers().get_all(SET_COOKIE) {
        let mut cookie = header
            .to_str()
            .ok()
            .and_then(|value| Cookie::parse(value.to_owned()).ok())
            .ok_or(Rejection(
                StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
            ))?;
        if cookie.name() == "__Host-piwas-guest" {
            cookie.set_secure(true);
            cookie.set_http_only(true);
            cookie.set_same_site(SameSite::Lax);
            cookie.set_path("/");
            cookie.unset_domain();
            headers.push(as_header(&cookie)?);
        } else {
            headers.push(header.clone());
        }
    }
    response.headers_mut().remove(SET_COOKIE);
    for header in headers {
        response.headers_mut().append(SET_COOKIE, header);
    }
    Ok(response)
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware, routing::get};
    use tower::ServiceExt;
    #[tokio::test]
    async fn only_host_session_attributes_are_normalized_and_invalid_headers_fail_closed() {
        for (raw, status) in [
            (
                HeaderValue::from_static("other=unchanged; Path=/"),
                StatusCode::OK,
            ),
            (
                HeaderValue::from_static("invalid-cookie"),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                HeaderValue::from_bytes(&[255]).expect("opaque fixture header"),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ] {
            let supplied = raw.clone();
            let app = Router::new()
                .route(
                    "/",
                    get(move || {
                        let raw = supplied.clone();
                        async move { ([(SET_COOKIE, raw)], "fixture") }
                    }),
                )
                .layer(middleware::from_fn(secure));
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("HTTP");
            assert_eq!(response.status(), status);
            if status == StatusCode::OK {
                assert_eq!(response.headers()[SET_COOKIE], raw);
            }
        }
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    use axum::{Router, body::Body, middleware, routing::get};
    use tower::ServiceExt;
    #[tokio::test]
    async fn insecure_host_tombstones_receive_required_attributes_and_bad_serialization_is_rejected()
     {
        assert!(as_header(&Cookie::new("__Host-piwas-guest", "invalid\nvalue")).is_err());
        let app = Router::new()
            .route(
                "/",
                get(|| async { ([(SET_COOKIE, "__Host-piwas-guest=; Max-Age=0")], "fixture") }),
            )
            .layer(middleware::from_fn(secure));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("HTTP");
        assert_eq!(response.status(), StatusCode::OK);
        let cookie =
            Cookie::parse(response.headers()[SET_COOKIE].to_str().expect("ASCII")).expect("cookie");
        assert_eq!(cookie.name(), "__Host-piwas-guest");
        assert_eq!(cookie.value(), "");
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.domain(), None);
        assert_eq!(
            cookie.max_age(),
            Some(tower_sessions::cookie::time::Duration::ZERO)
        );
    }
}
