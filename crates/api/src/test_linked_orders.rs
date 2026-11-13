//! Test-only linked guest-order history. No linking or public account route.
use crate::{ApiError, AppState, error, test_login::LoginConfig, test_session::customer_cookie};
use axum::{
    Extension, Json,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use commerce_domain::checkout_quotes::{CapturedAmount, MAX_QUOTE_UNIX_SECONDS};
use commerce_infrastructure::guest_order_links::{
    self, LinkError, LinkedOrderCursor, LinkedOrderPage, LinkedOrderView,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct TestOrderAmount {
    gross_minor: String,
    net_minor: String,
    vat_minor: String,
}
impl From<CapturedAmount> for TestOrderAmount {
    fn from(value: CapturedAmount) -> Self {
        Self {
            gross_minor: value.gross_minor.to_string(),
            net_minor: value.net_minor.to_string(),
            vat_minor: value.vat_minor.to_string(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct TestOrderLine {
    sku: String,
    title: String,
    quantity: u32,
    amounts: TestOrderAmount,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct TestLinkedOrder {
    order_id: String,
    created_at: String,
    linked_at: String,
    currency: String,
    lines: Vec<TestOrderLine>,
    shipping: TestOrderAmount,
    total: TestOrderAmount,
}
impl From<LinkedOrderView> for TestLinkedOrder {
    fn from(order: LinkedOrderView) -> Self {
        Self {
            order_id: order.order_id,
            created_at: order.created_at.to_string(),
            linked_at: order.linked_at.to_string(),
            currency: order.currency,
            lines: order
                .lines
                .into_iter()
                .map(|line| TestOrderLine {
                    sku: line.sku,
                    title: line.title,
                    quantity: line.quantity,
                    amounts: line.amounts.into(),
                })
                .collect(),
            shipping: order.shipping.into(),
            total: order.total.into(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct TestLinkedOrderPage {
    orders: Vec<TestLinkedOrder>,
    has_more: bool,
}

// A cursor is only a position. Each read independently claims the current owner.
fn cursor(query: Option<&str>) -> Result<Option<LinkedOrderCursor>, ()> {
    let Some(query) = query else { return Ok(None) };
    if query.len() > 100 {
        return Err(());
    }
    let mut time = None;
    let mut order = None;
    for part in query.split('&') {
        let (key, value) = part.split_once('=').ok_or(())?;
        match key {
            "after_linked_at" if time.is_none() => time = Some(value),
            "after_order_id" if order.is_none() => order = Some(value),
            _ => return Err(()),
        }
    }
    let (Some(time), Some(order)) = (time, order) else {
        return Err(());
    };
    if time.is_empty()
        || time.len() > 16
        || (time.len() > 1 && time.starts_with('0'))
        || !time.bytes().all(|b| b.is_ascii_digit())
        || order.len() != 32
        || !order
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(());
    }
    let time = time.parse::<i64>().map_err(|_| ())?;
    if time > MAX_QUOTE_UNIX_SECONDS {
        return Err(());
    }
    Ok(Some(LinkedOrderCursor {
        linked_at: time,
        order_id: order.into(),
    }))
}

fn has_more(page: &LinkedOrderPage) -> Result<bool, ()> {
    let Some(next) = &page.next_cursor else {
        return Ok(false);
    };
    if page
        .orders
        .last()
        .is_some_and(|order| order.linked_at == next.linked_at && order.order_id == next.order_id)
    {
        Ok(true)
    } else {
        Err(())
    }
}

#[utoipa::path(get, operation_id="test_customer_linked_orders", path="/api/test/customer/linked-orders", params(("Origin"=String,Header),("after_linked_at"=Option<String>,Query),("after_order_id"=Option<String>,Query)), responses((status=200,body=TestLinkedOrderPage,description="Owned linked guest orders; use the last item as the next position when has_more"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"customer_test"=[])))]
pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    request: Request,
) -> Response {
    if request.method() != Method::GET {
        return error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
    }
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "linked_orders_origin_required");
    }
    let Ok(after) = cursor(request.uri().query()) else {
        return error(StatusCode::BAD_REQUEST, "invalid_linked_orders_request");
    };
    let Some(token) = customer_cookie(request.headers()) else {
        return error(StatusCode::UNAUTHORIZED, "linked_orders_denied");
    };
    if to_bytes(request.into_body(), 0).await.is_err() {
        return error(StatusCode::BAD_REQUEST, "invalid_linked_orders_request");
    }
    match guest_order_links::list_linked_orders(
        &state.pool,
        &token,
        after.as_ref(),
        8,
        state.clock.as_ref(),
    )
    .await
    {
        Ok(page) => {
            let Ok(has_more) = has_more(&page) else {
                return error(StatusCode::SERVICE_UNAVAILABLE, "linked_orders_unavailable");
            };
            Json(TestLinkedOrderPage {
                orders: page.orders.into_iter().map(Into::into).collect(),
                has_more,
            })
            .into_response()
        }
        Err(LinkError::Denied) => error(StatusCode::UNAUTHORIZED, "linked_orders_denied"),
        Err(LinkError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "linked_orders_unavailable")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CapturedAmount, LinkedOrderCursor, LinkedOrderPage, LinkedOrderView, cursor, has_more,
    };

    #[test]
    fn cursor_is_a_closed_canonical_position() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(matches!(cursor(None), Ok(None)));
        for query in [
            format!("after_linked_at=0&after_order_id={id}"),
            format!("after_order_id={id}&after_linked_at=9007199254740991"),
        ] {
            assert!(matches!(cursor(Some(&query)), Ok(Some(_))));
        }
        for query in [
            "".to_owned(),
            "after_linked_at=1".to_owned(),
            format!("after_linked_at=01&after_order_id={id}"),
            format!("after_linked_at=+1&after_order_id={id}"),
            format!("after_linked_at=9007199254740992&after_order_id={id}"),
            format!("after_linked_at=1&after_order_id={}", id.to_uppercase()),
            format!("after_linked_at=1&after_order_id={id}&after_linked_at=1"),
            format!("after_linked_at=1&after_order_id={id}&extra=1"),
            format!("after_linked_at=%31&after_order_id={id}"),
        ] {
            assert!(cursor(Some(&query)).is_err());
        }
    }

    #[test]
    fn inconsistent_next_position_is_rejected() {
        let mut page = LinkedOrderPage {
            orders: Vec::new(),
            next_cursor: Some(LinkedOrderCursor {
                linked_at: 1,
                order_id: "0123456789abcdef0123456789abcdef".into(),
            }),
        };
        assert!(has_more(&page).is_err());
        let amount = CapturedAmount {
            gross_minor: 0,
            net_minor: 0,
            vat_minor: 0,
        };
        page.orders.push(LinkedOrderView {
            order_id: "0123456789abcdef0123456789abcdef".into(),
            created_at: 0,
            linked_at: 1,
            currency: "EUR".into(),
            lines: Vec::new(),
            shipping: amount.clone(),
            total: amount,
        });
        assert_eq!(has_more(&page), Ok(true));
        page.next_cursor.as_mut().expect("cursor").linked_at = 2;
        assert!(has_more(&page).is_err());
        let next = page.next_cursor.as_mut().expect("cursor");
        next.linked_at = 1;
        next.order_id = "ffffffffffffffffffffffffffffffff".into();
        assert!(has_more(&page).is_err());
        page.next_cursor = None;
        assert_eq!(has_more(&page), Ok(false));
    }
}
