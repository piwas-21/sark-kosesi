//! Synthetic seller/contact fixtures; no legal, tax or postal acceptance.
use commerce_domain::checkout_orders::*;
#[allow(dead_code)]
#[path = "../../../../tests/fixtures/checkout_order_snapshot.rs"]
pub(crate) mod snapshot_fixture;
pub use snapshot_fixture::{address, seller};
pub fn intent(quote_id: &str) -> OrderIntent {
    OrderIntent {
        quote_id: quote_id.into(),
        expected_basket_revision: 1,
        buyer: BuyerContact {
            name: "Synthetic Buyer".into(),
            email: "buyer@example.test".into(),
        },
        shipping_address: address(),
        billing_address: address(),
        accepted_terms_version: seller().terms_version,
        confirmed: true,
    }
}
