use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscountKind {
    Fixed { minor_units: i64 },
    Percentage { basis_points: u16 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibleProduct {
    pub sku: String,
    pub publication_revision: i32,
}
/// Trusted terms use UTC instants, one promotion per quote and no shipping discount.
/// Independent market policy still owns tax, delivery readiness and product limits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promotion {
    pub id: String,
    pub revision: i32,
    pub valid_from: i64,
    pub expires_at: i64,
    pub countries: Vec<String>,
    pub products: Vec<EligibleProduct>,
    pub minimum_eligible_gross_minor: i64,
    pub maximum_discount_minor: i64,
    pub discount: DiscountKind,
    pub evidence_ref: String,
}
impl Promotion {
    pub fn validate(&self) -> Result<(), PromotionError> {
        let mut countries = BTreeSet::new();
        let mut products = BTreeSet::new();
        let amount_valid = match self.discount {
            DiscountKind::Fixed { minor_units } => (1..=MAX_MINOR_UNITS).contains(&minor_units),
            DiscountKind::Percentage { basis_points } => (1..=10000).contains(&basis_points),
        };
        if !merchant_code(&self.id)
            || self.revision <= 0
            || self.valid_from < 0
            || self.expires_at <= self.valid_from
            || !amount_valid
            || !(0..=MAX_MINOR_UNITS).contains(&self.minimum_eligible_gross_minor)
            || !(1..=MAX_MINOR_UNITS).contains(&self.maximum_discount_minor)
            || self.countries.is_empty()
            || self.countries.len() > 27
            || !self
                .countries
                .iter()
                .all(|c| crate::quotes::valid_country(c) && countries.insert(c))
            || self.products.is_empty()
            || self.products.len() > MAX_LINES
            || !self.products.iter().all(|p| {
                merchant_code(&p.sku) && p.publication_revision > 0 && products.insert(&p.sku)
            })
            || self.evidence_ref.trim().is_empty()
            || self.evidence_ref.chars().count() > 500
            || self.evidence_ref.chars().any(char::is_control)
        {
            return Err(PromotionError::InvalidPolicy);
        }
        Ok(())
    }
    pub fn permits(&self, country: &str, now: i64) -> Result<(), PromotionError> {
        self.validate()?;
        if now < self.valid_from
            || now >= self.expires_at
            || !self.countries.iter().any(|c| c == country)
        {
            return Err(PromotionError::Ineligible);
        }
        Ok(())
    }
}
