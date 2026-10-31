//! Invented snapshot; no actual seller/tax/legal/postal or delivery evidence.
use commerce_domain::{checkout_orders::*, checkout_quotes::*, specifications::DimensionsMm};
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
pub fn snapshot() -> OrderSnapshot {
    let quote:QuoteSnapshot=serde_json::from_value(serde_json::json!({"schema_version":1,"currency":"EUR","binding":{"quote_id":"11111111111111111111111111111111","basket_id":"22222222222222222222222222222222","basket_revision":1,"locale":"nl","country":"NL","postcode":"1234 AB","policy_id":"NL-POLICY","policy_revision":1,"policy_review_revision":1,"created_at":1100,"expires_at":1400},"lines":[{"sku":"SET-FILLED","publication_revision":1,"bom_revision":1,"quantity":3,"unit_gross_minor":100,"vat_basis_points":2100,"amounts":{"gross_minor":300,"net_minor":248,"vat_minor":52}}],"shipping":{"gross_minor":900,"net_minor":744,"vat_minor":156},"shipping_vat_basis_points":2100,"total":{"gross_minor":1200,"net_minor":992,"vat_minor":208}})).expect("independent literal prices");
    OrderSnapshot {
        schema_version: 1,
        identity: OrderIdentity {
            id: "3".repeat(32),
            basket_id: "2".repeat(32),
            quote_id: "1".repeat(32),
            stock_group_id: "4".repeat(32),
            created_at: 1200,
            hold_expires_at: 2100,
        },
        quote,
        buyer: BuyerContact {
            name: "Synthetic Buyer".into(),
            email: "buyer@example.test".into(),
        },
        shipping_address: address(),
        billing_address: address(),
        seller: seller(),
        market: commerce_domain::quotes::MarketPolicy {
            id: "NL-POLICY".into(),
            revision: 1,
            destination: commerce_domain::quotes::Destination {
                country: "NL".into(),
                postcode: "1234 AB".into(),
            },
            valid_from: 1000,
            expires_at: 2000,
            products: vec![commerce_domain::quotes::ProductRule {
                sku: "SET-FILLED".into(),
                publication_revision: 1,
                max_quantity: 10,
                vat_basis_points: 2100,
            }],
            shipping: commerce_domain::quotes::ShippingRule {
                gross: commerce_domain::Money::eur(900).expect("fixture money"),
                vat_basis_points: 2100,
                max_parcels: 10,
                max_total_weight_g: 30000,
                max_edge_mm: 505,
            },
            evidence_ref: "synthetic market policy".into(),
        },
        products: vec![ProductSnapshot {
            sku: "SET-FILLED".into(),
            publication_revision: 1,
            bom_revision: 1,
            title: "Synthetic test set".into(),
            family_code: "FAMILY-A".into(),
            size_code: "SIZE-A".into(),
            textile_code: "TEXTILE-A".into(),
            fill: "with-foam".into(),
            footprint: DimensionsMm {
                width_mm: 1000,
                depth_mm: 500,
                height_mm: 500,
                tolerance_mm: 5,
            },
        }],
    }
}
