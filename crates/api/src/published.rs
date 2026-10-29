use crate::published_types::{PublishedCatalogue, PublishedProduct};
use crate::{ApiError, AppState, error};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use commerce_application::Locale;
use commerce_infrastructure::published_catalogue::{self as source, PublishedError};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    after: Option<String>,
}
fn failure(e: PublishedError) -> Response {
    match e {
        PublishedError::InvalidRequest => {
            error(StatusCode::BAD_REQUEST, "invalid_catalogue_request")
        }
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "published_catalogue_unavailable",
        ),
    }
}
#[utoipa::path(get,path="/api/published/{locale}",params(("locale"=String,Path),("after"=Option<String>,Query)),responses((status=200,body=PublishedCatalogue),(status=400,body=ApiError),(status=401,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[])))]
pub async fn page(
    State(state): State<AppState>,
    Path(language): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "unsupported_locale");
    };
    let Ok(Query(query)) = query else {
        return error(StatusCode::BAD_REQUEST, "invalid_catalogue_request");
    };
    match source::published_page(
        &state.pool,
        state.clock.as_ref(),
        locale,
        query.after.as_deref(),
    )
    .await
    {
        Ok(page) => Json(PublishedCatalogue {
            items: page.items.into_iter().map(Into::into).collect(),
            locale: locale.public().into(),
            next_after: page.next_after,
            production_enabled: false,
        })
        .into_response(),
        Err(e) => failure(e),
    }
}
#[utoipa::path(get,path="/api/published/{locale}/products/{sku}",params(("locale"=String,Path),("sku"=String,Path)),responses((status=200,body=PublishedProduct),(status=400,body=ApiError),(status=401,body=ApiError),(status=404,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[])))]
pub async fn product(
    State(state): State<AppState>,
    Path((language, sku)): Path<(String, String)>,
) -> Response {
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "unsupported_locale");
    };
    match source::published_product(&state.pool, state.clock.as_ref(), locale, &sku).await {
        Ok(Some(p)) => Json(PublishedProduct::from(p)).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "published_product_not_found"),
        Err(e) => failure(e),
    }
}
#[utoipa::path(get,path="/api/published/{locale}/images/{sku}/{revision}/{edge}",params(("locale"=String,Path),("sku"=String,Path),("revision"=i32,Path),("edge"=i32,Path)),responses((status=200,description="Verified sanitized JPEG or PNG derivative",content((crate::published_types::ImageBytes="image/jpeg"),(crate::published_types::ImageBytes="image/png"))),(status=400,body=ApiError),(status=401,body=ApiError),(status=404,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[])))]
pub async fn image(
    State(state): State<AppState>,
    path: Result<Path<(String, String, i32, i32)>, PathRejection>,
) -> Response {
    let Ok(Path((language, sku, revision, edge))) = path else {
        return error(StatusCode::BAD_REQUEST, "invalid_catalogue_request");
    };
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "unsupported_locale");
    };
    match source::published_image(
        &state.pool,
        state.clock.as_ref(),
        locale,
        &sku,
        revision,
        edge,
    )
    .await
    {
        Ok(Some(r)) => image_response(r),
        Ok(None) => error(StatusCode::NOT_FOUND, "published_image_not_found"),
        Err(e) => failure(e),
    }
}

fn image_response(r: commerce_infrastructure::image_ingestion::PrivateRendition) -> Response {
    let Some(content_type) = image_content_type(&r.image_type) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "published_catalogue_unavailable",
        );
    };
    let mut response = r.bytes.into_response();
    response.headers_mut().insert("content-type", content_type);
    response
}

fn image_content_type(kind: &str) -> Option<HeaderValue> {
    match kind {
        "jpeg" => Some(HeaderValue::from_static("image/jpeg")),
        "png" => Some(HeaderValue::from_static("image/png")),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_verified_raster_types_receive_image_content_types() {
        assert_eq!(image_content_type("jpeg").expect("JPEG"), "image/jpeg");
        assert_eq!(image_content_type("png").expect("PNG"), "image/png");
        for (kind, expected) in [
            ("jpeg", StatusCode::OK),
            ("png", StatusCode::OK),
            ("svg", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let r = commerce_infrastructure::image_ingestion::PrivateRendition {
                image_type: kind.into(),
                width: 1,
                height: 1,
                bytes: vec![1],
            };
            assert_eq!(image_response(r).status(), expected);
        }
        for invalid in ["svg", "text/html", "image/jpeg", ""] {
            assert!(image_content_type(invalid).is_none());
        }
    }
}
