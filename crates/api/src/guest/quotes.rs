//! Private indicative quote routes; no checkout or provider execution.
use super::*;
use axum::extract::Path;
use commerce_application::Locale;
use commerce_domain::quotes::Destination;
use commerce_infrastructure::checkout_quotes::{self, GuestQuoteRequest, QuoteStoreError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QuoteRequestBody {
    pub expected_basket_revision: i32,
    pub effect_key: String,
    pub country: String,
    pub postcode: String,
}
#[derive(Serialize, ToSchema)]
pub struct QuoteAmounts {
    pub gross_minor: i64,
    pub net_minor: i64,
    pub vat_minor: i64,
}
impl From<commerce_domain::checkout_quotes::CapturedAmount> for QuoteAmounts {
    fn from(a: commerce_domain::checkout_quotes::CapturedAmount) -> Self {
        Self {
            gross_minor: a.gross_minor,
            net_minor: a.net_minor,
            vat_minor: a.vat_minor,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct QuotedLine {
    pub sku: String,
    pub quantity: u32,
    pub unit_gross_minor: i64,
    pub vat_basis_points: u16,
    pub amounts: QuoteAmounts,
}
#[derive(Serialize, ToSchema)]
pub struct QuoteView {
    pub id: String,
    pub basket_revision: i32,
    pub locale: String,
    pub expires_at: i64,
    pub currency: String,
    pub lines: Vec<QuotedLine>,
    pub shipping: QuoteAmounts,
    pub shipping_vat_basis_points: u16,
    pub total: QuoteAmounts,
    pub indicative: bool,
}
impl TryFrom<commerce_domain::checkout_quotes::QuoteSnapshot> for QuoteView {
    type Error = QuoteStoreError;
    fn try_from(s: commerce_domain::checkout_quotes::QuoteSnapshot) -> Result<Self, Self::Error> {
        let locale =
            Locale::from_internal(&s.binding.locale).ok_or(QuoteStoreError::InvalidStoredData)?;
        Ok(Self {
            id: s.binding.quote_id,
            basket_revision: s.binding.basket_revision,
            locale: locale.public().into(),
            expires_at: s.binding.expires_at,
            currency: s.currency,
            lines: s
                .lines
                .into_iter()
                .map(|l| QuotedLine {
                    sku: l.sku,
                    quantity: l.quantity,
                    unit_gross_minor: l.unit_gross_minor,
                    vat_basis_points: l.vat_basis_points,
                    amounts: l.amounts.into(),
                })
                .collect(),
            shipping: s.shipping.into(),
            shipping_vat_basis_points: s.shipping_vat_basis_points,
            total: s.total.into(),
            indicative: true,
        })
    }
}
#[derive(Serialize, ToSchema)]
pub struct QuoteResponse {
    pub quote: QuoteView,
    pub replayed: bool,
}
fn failed(value: QuoteStoreError) -> Response {
    use commerce_domain::quotes::QuoteError;
    use commerce_infrastructure::{market_policies::MarketError, quotes::QuoteFailure};
    let (status, code) = match value {
        QuoteStoreError::Guest(value) => return failure(value),
        QuoteStoreError::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid_quote_request"),
        QuoteStoreError::NotFound => (StatusCode::NOT_FOUND, "quote_not_found"),
        QuoteStoreError::Conflict => (StatusCode::CONFLICT, "quote_intent_conflict"),
        QuoteStoreError::Expired => (StatusCode::GONE, "quote_expired"),
        QuoteStoreError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "quote_rate_limited"),
        QuoteStoreError::Market(MarketError::NotFound)
        | QuoteStoreError::Market(MarketError::Quote(QuoteFailure::Policy(
            QuoteError::MarketUnavailable
            | QuoteError::ProductUnavailable
            | QuoteError::InsufficientStock
            | QuoteError::ShippingUnavailable,
        )))
        | QuoteStoreError::Evaluation(QuoteFailure::Policy(
            QuoteError::MarketUnavailable
            | QuoteError::ProductUnavailable
            | QuoteError::InsufficientStock
            | QuoteError::ShippingUnavailable,
        )) => (StatusCode::CONFLICT, "quote_unavailable"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "quote_service_unavailable"),
    };
    let mut response = error(status, code);
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert("retry-after", axum::http::HeaderValue::from_static("60"));
    }
    response
}
#[utoipa::path(post,path="/api/guest/{locale}/quotes",params(("locale"=String,Path),("Origin"=String,Header),("x-csrf-token"=String,Header)),request_body=QuoteRequestBody,responses((status=200,body=QuoteResponse),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=409,body=ApiError),(status=410,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=429,body=ApiError),(status=503,body=ApiError)),security(("preview"=[],"guest"=[])))]
pub(crate) async fn issue(
    Extension(state): Extension<GuestState>,
    Path(language): Path<String>,
    session: Session,
    request: Request,
) -> Response {
    let csrf = match mutation(&state, &request, true) {
        Ok(csrf) => csrf,
        Err(value) => return value.into_response(),
    };
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "unsupported_locale");
    };
    let key = match GuestCredential::from_session(&session) {
        Ok(key) => key,
        Err(value) => return failure(value),
    };
    let body = match body::<QuoteRequestBody>(request).await {
        Ok(body) => body,
        Err(value) => return value.into_response(),
    };
    let input = GuestQuoteRequest {
        expected_basket_revision: body.expected_basket_revision,
        effect_key: body.effect_key,
        destination: Destination {
            country: body.country,
            postcode: body.postcode,
        },
    };
    match checkout_quotes::issue_quote(&state.store, &key, &csrf, locale, &input).await {
        Ok(receipt) => match QuoteView::try_from(receipt.snapshot) {
            Ok(quote) => Json(QuoteResponse {
                quote,
                replayed: receipt.replayed,
            })
            .into_response(),
            Err(value) => failed(value),
        },
        Err(value) => failed(value),
    }
}
#[utoipa::path(get,path="/api/guest/quotes/{id}",params(("id"=String,Path)),responses((status=200,body=QuoteView),(status=400,body=ApiError),(status=401,body=ApiError),(status=404,body=ApiError),(status=410,body=ApiError),(status=503,body=ApiError)),security(("preview"=[],"guest"=[])))]
pub(crate) async fn read(
    Extension(state): Extension<GuestState>,
    Path(id): Path<String>,
    session: Session,
) -> Response {
    let key = match GuestCredential::from_session(&session) {
        Ok(key) => key,
        Err(value) => return failure(value),
    };
    match checkout_quotes::owned_quote(&state.store, &key, &id).await {
        Ok(snapshot) => match QuoteView::try_from(snapshot) {
            Ok(quote) => Json(quote).into_response(),
            Err(value) => failed(value),
        },
        Err(value) => failed(value),
    }
}

#[cfg(test)]
#[path = "../../tests/support/quote_view.rs"]
mod tests;
