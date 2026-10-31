//! Normalize the maintained middleware's deletion cookies for the __Host- prefix.
use crate::error;
use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode, header::SET_COOKIE},
    middleware::Next,
    response::Response,
};
use tower_sessions::cookie::{Cookie, SameSite};
pub(crate) async fn secure(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let mut headers = Vec::new();
    for header in response.headers().get_all(SET_COOKIE) {
        let parsed = header
            .to_str()
            .ok()
            .and_then(|value| Cookie::parse(value.to_owned()).ok());
        let Some(mut cookie) = parsed else {
            return error(StatusCode::SERVICE_UNAVAILABLE, "guest_unavailable");
        };
        if cookie.name() == "__Host-piwas-guest" {
            cookie.set_secure(true);
            cookie.set_http_only(true);
            cookie.set_same_site(SameSite::Lax);
            cookie.set_path("/");
            cookie.unset_domain();
            let Ok(header) = HeaderValue::from_str(&cookie.to_string()) else {
                return error(StatusCode::SERVICE_UNAVAILABLE, "guest_unavailable");
            };
            headers.push(header);
        } else {
            headers.push(header.clone());
        }
    }
    response.headers_mut().remove(SET_COOKIE);
    for header in headers {
        response.headers_mut().append(SET_COOKIE, header);
    }
    response
}
