use commerce_domain::{guest_baskets::BasketUpdate, quotes::BasketLine};
use commerce_infrastructure::guest_baskets::{BasketReceipt, BasketView};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GuestIssueRequest {}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GuestLine {
    pub sku: String,
    pub quantity: u32,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GuestBasketUpdate {
    pub expected_revision: i32,
    pub effect_key: String,
    pub lines: Vec<GuestLine>,
}
impl From<GuestBasketUpdate> for BasketUpdate {
    fn from(value: GuestBasketUpdate) -> Self {
        Self {
            expected_revision: value.expected_revision,
            effect_key: value.effect_key,
            lines: value
                .lines
                .into_iter()
                .map(|line| BasketLine {
                    sku: line.sku,
                    quantity: line.quantity,
                })
                .collect(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct GuestBasket {
    pub revision: i32,
    pub lines: Vec<GuestLine>,
    pub csrf_token: String,
}
impl From<BasketView> for GuestBasket {
    fn from(value: BasketView) -> Self {
        Self {
            revision: value.revision,
            csrf_token: value.csrf_token().into(),
            lines: value
                .lines
                .into_iter()
                .map(|line| GuestLine {
                    sku: line.sku,
                    quantity: line.quantity,
                })
                .collect(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct GuestBasketReceipt {
    pub revision: i32,
    pub replayed: bool,
}
impl From<BasketReceipt> for GuestBasketReceipt {
    fn from(value: BasketReceipt) -> Self {
        Self {
            revision: value.revision,
            replayed: value.replayed,
        }
    }
}

/// Enforce the object shapes promised by OpenAPI; serde structs also accept sequences.
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ()> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.get("lines").is_some_and(|lines| {
        !lines
            .as_array()
            .is_some_and(|lines| lines.iter().all(serde_json::Value::is_object))
    }) {
        return Err(());
    }
    serde_json::from_value(value).map_err(|_| ())
}
