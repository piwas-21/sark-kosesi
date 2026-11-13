//! Explicit test-only customer favourites projection; no public account route.
use crate::{ApiError, AppState, error, test_login::LoginConfig, test_session::customer_cookie};
use axum::{
    Extension, Json,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use commerce_application::Locale;
use commerce_infrastructure::customer_favourites::{self, FavouriteError};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct TestFavouriteItem {
    sku: String,
    publication_revision: i32,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct TestFavouriteList {
    /// Decimal representation avoids losing a PostgreSQL bigint in JavaScript.
    #[schema(pattern = "^(0|[1-9][0-9]*)$", max_length = 19)]
    revision: String,
    items: Vec<TestFavouriteItem>,
}

#[utoipa::path(get, operation_id="test_customer_favourites", path="/api/test/customer/favourites/{locale}", params(("locale"=String,Path),("Origin"=String,Header)), responses((status=200,body=TestFavouriteList,description="Current private favourites for the cookie owner"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"customer_test"=[])))]
pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    Path(language): Path<String>,
    request: Request,
) -> Response {
    if request.method() != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    }
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "favourites_origin_required");
    }
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    }
    let Some(token) = customer_cookie(request.headers()) else {
        return error(StatusCode::UNAUTHORIZED, "favourites_denied");
    };
    if to_bytes(request.into_body(), 0).await.is_err() {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    }
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "unsupported_locale");
    };
    match customer_favourites::list(&state.pool, &token, state.clock.as_ref(), locale).await {
        Ok(list) => Json(TestFavouriteList {
            revision: list.revision.to_string(),
            items: list
                .items
                .into_iter()
                .map(|item| TestFavouriteItem {
                    sku: item.sku,
                    publication_revision: item.publication_revision,
                })
                .collect(),
        })
        .into_response(),
        Err(FavouriteError::Denied) => error(StatusCode::UNAUTHORIZED, "favourites_denied"),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "favourites_unavailable"),
    }
}
