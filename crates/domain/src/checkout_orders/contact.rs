use super::*;
use email_address::{EmailAddress, Options};
/// Syntax and bounds only; neither a delivery promise nor proof of mailbox ownership.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Address {
    pub recipient: String,
    pub line1: String,
    pub line2: Option<String>,
    pub city: String,
    pub region: Option<String>,
    pub country: String,
    pub postcode: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuyerContact {
    pub name: String,
    pub email: String,
}
impl Address {
    pub fn validate(&self) -> Result<(), OrderError> {
        if !text(&self.recipient, 160)
            || !text(&self.line1, 200)
            || !text(&self.city, 160)
            || self.line2.as_ref().is_some_and(|v| !text(v, 200))
            || self.region.as_ref().is_some_and(|v| !text(v, 160))
            || self.country.len() != 2
            || self.postcode.len() > 16
        {
            return Err(OrderError::InvalidRequest);
        }
        crate::quotes::Destination {
            country: self.country.clone(),
            postcode: self.postcode.clone(),
        }
        .validate()
        .map_err(|_| OrderError::InvalidRequest)
    }
}
impl BuyerContact {
    pub fn validate(&self) -> Result<(), OrderError> {
        if !text(&self.name, 160) || !email(&self.email) {
            return Err(OrderError::InvalidRequest);
        }
        Ok(())
    }
}
pub(super) fn email(value: &str) -> bool {
    value.len() <= 254
        && text(value, 254)
        && EmailAddress::parse_with_options(
            value,
            Options::default()
                .without_display_text()
                .without_domain_literal()
                .with_required_tld(),
        )
        .is_ok()
}
