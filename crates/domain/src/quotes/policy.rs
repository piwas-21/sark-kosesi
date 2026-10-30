use super::*;
use crate::specifications::bounded_text;

/// Trusted server input. No installed policy, request decoder or ready-country default.
/// Exact postcodes and publication revisions prevent broad territorial/product fallback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketPolicy {
    pub id: String,
    pub revision: i32,
    pub destination: Destination,
    pub valid_from: i64,
    pub expires_at: i64,
    pub products: Vec<ProductRule>,
    pub shipping: ShippingRule,
    pub evidence_ref: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductRule {
    pub sku: String,
    pub publication_revision: i32,
    pub max_quantity: u32,
    pub vat_basis_points: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShippingRule {
    #[serde(deserialize_with = "decode_money")]
    pub gross: Money,
    pub vat_basis_points: u16,
    pub max_parcels: u32,
    pub max_total_weight_g: u64,
    /// Each measured parcel edge plus its tolerance must fit this bound.
    pub max_edge_mm: u32,
}
impl MarketPolicy {
    pub fn validate(&self) -> Result<(), QuoteError> {
        let mut seen = BTreeSet::new();
        if !merchant_code(&self.id)
            || self.revision <= 0
            || !valid_country(&self.destination.country)
            || !valid_postcode(&self.destination.postcode)
            || self.valid_from < 0
            || self.expires_at <= self.valid_from
            || !bounded_text(&self.evidence_ref, 500)
            || self.evidence_ref.chars().any(char::is_control)
            || self.products.is_empty()
            || self.products.len() > MAX_LINES
            || !self.products.iter().all(|p| {
                merchant_code(&p.sku)
                    && p.publication_revision > 0
                    && (1..=MAX_QUANTITY).contains(&p.max_quantity)
                    && valid_rate(p.vat_basis_points)
                    && seen.insert(&p.sku)
            })
            || !bounded_money(self.shipping.gross)
            || !valid_rate(self.shipping.vat_basis_points)
            || !(1..=1000).contains(&self.shipping.max_parcels)
            || self.shipping.max_total_weight_g == 0
            || self.shipping.max_edge_mm == 0
        {
            return Err(QuoteError::InvalidPolicy);
        }
        Ok(())
    }
    pub fn permits(&self, destination: &Destination, now: i64) -> Result<(), QuoteError> {
        self.validate()?;
        if self.destination != *destination || now < self.valid_from || now >= self.expires_at {
            return Err(QuoteError::MarketUnavailable);
        }
        Ok(())
    }
    pub fn product(
        &self,
        sku: &str,
        revision: i32,
        quantity: u32,
    ) -> Result<&ProductRule, QuoteError> {
        self.products
            .iter()
            .find(|p| {
                p.sku == sku
                    && p.publication_revision == revision
                    && (1..=p.max_quantity).contains(&quantity)
            })
            .ok_or(QuoteError::ProductUnavailable)
    }
}

fn decode_money<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Money, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StoredMoney {
        minor_units: i64,
        #[serde(rename = "currency")]
        _currency: crate::Currency,
    }
    let value = StoredMoney::deserialize(decoder)?;
    Money::eur(value.minor_units).map_err(|_| serde::de::Error::custom("Invalid stored money"))
}
