//! Synthetic seller/contact fixtures; no legal, tax or postal acceptance.
use commerce_domain::checkout_orders::*;
pub fn address() -> Address {
    Address {
        recipient: "Synthetic Recipient".into(),
        line1: "Synthetic Street 22".into(),
        line2: None,
        city: "Fixture City".into(),
        region: None,
        country: "NL".into(),
        postcode: "1234 AB".into(),
    }
}
pub fn seller() -> SellerPolicy {
    SellerPolicy {
        id: "SYNTHETIC-SELLER".into(),
        revision: 1,
        locale: "nl".into(),
        legal_name: "Piwas synthetic test seller".into(),
        registration_id: "synthetic-registration-not-real".into(),
        tax_registration: SellerTaxRegistration::VatRegistered {
            vat_id: "synthetic-tax-id-not-real".into(),
        },
        address: address(),
        support_email: "support@example.test".into(),
        terms_version: "synthetic-terms-v1".into(),
        terms_sha256: "a".repeat(64),
        privacy_version: "synthetic-privacy-v1".into(),
        privacy_sha256: "b".repeat(64),
        valid_from: 1000,
        expires_at: 2000,
        evidence_ref: "synthetic seller/legal/tax evidence only".into(),
    }
}

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
