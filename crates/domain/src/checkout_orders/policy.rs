use super::*;
/// Required explicit seller tax identity; data validation is not qualified tax approval.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SellerTaxRegistration {
    VatRegistered { vat_id: String },
    NotVatRegistered { evidence_ref: String },
}
/// Trusted server configuration. Nothing is installed or inferred from customer input.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SellerPolicy {
    pub id: String,
    pub revision: i32,
    pub locale: String,
    pub legal_name: String,
    pub registration_id: String,
    pub tax_registration: SellerTaxRegistration,
    pub address: Address,
    pub support_email: String,
    pub terms_version: String,
    pub terms_sha256: String,
    pub privacy_version: String,
    pub privacy_sha256: String,
    pub valid_from: i64,
    pub expires_at: i64,
    pub evidence_ref: String,
}
impl SellerPolicy {
    pub fn validate(&self) -> Result<(), OrderError> {
        let tax_valid = match &self.tax_registration {
            SellerTaxRegistration::VatRegistered { vat_id } => text(vat_id, 128),
            SellerTaxRegistration::NotVatRegistered { evidence_ref } => text(evidence_ref, 500),
        };
        if !crate::catalogue_authoring::merchant_code(&self.id)
            || self.revision <= 0
            || !crate::checkout_quotes::INTERNAL_LOCALES.contains(&self.locale.as_str())
            || !text(&self.legal_name, 200)
            || !text(&self.registration_id, 128)
            || !tax_valid
            || self.address.country != "NL"
            || !contact::email(&self.support_email)
            || !crate::media::valid_command_id(&self.terms_version)
            || !hex(&self.terms_sha256, 64)
            || !crate::media::valid_command_id(&self.privacy_version)
            || !hex(&self.privacy_sha256, 64)
            || self.valid_from < 0
            || self.expires_at <= self.valid_from
            || self.expires_at > crate::checkout_quotes::MAX_QUOTE_UNIX_SECONDS
            || !text(&self.evidence_ref, 500)
        {
            return Err(OrderError::InvalidRequest);
        }
        self.address.validate()
    }
    pub fn permits(&self, locale: &str, at: i64) -> Result<(), OrderError> {
        self.validate()?;
        if self.locale != locale || at < self.valid_from || at >= self.expires_at {
            return Err(OrderError::PolicyUnavailable);
        }
        Ok(())
    }
}
