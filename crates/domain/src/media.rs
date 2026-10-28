//! Permission attestations only: byte ingestion and product publication are separate.
use crate::specifications::bounded_text;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaKind {
    OriginalPhoto,
    GeneratedInspiration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageType {
    Jpeg,
    Png,
    Webp,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaAssetDraft {
    pub sha256: [u8; 32],
    pub image_type: ImageType,
    pub width_px: u32,
    pub height_px: u32,
    pub kind: MediaKind,
    pub fixture: bool,
    pub source_ref: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaUse {
    ProductPhotography,
    Inspiration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RightsTerm {
    Perpetual,
    Until { expires_at: i64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MediaRightsChange {
    Grant {
        use_scope: MediaUse,
        valid_from: i64,
        term: RightsTerm,
        permission_ref: String,
    },
    Revoke,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaValidationError;
pub fn valid_command_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
impl MediaAssetDraft {
    pub fn validate(&self) -> Result<(), MediaValidationError> {
        if !(1..=16384).contains(&self.width_px)
            || !(1..=16384).contains(&self.height_px)
            || u64::from(self.width_px) * u64::from(self.height_px) > 67108864
            || !bounded_text(&self.source_ref, 512)
        {
            return Err(MediaValidationError);
        }
        Ok(())
    }
    /// A private Piwas storage key, never a caller-supplied URL or filename.
    pub fn original_object_key(&self) -> String {
        let digest: String = self
            .sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let extension = match self.image_type {
            ImageType::Jpeg => "jpeg",
            ImageType::Png => "png",
            ImageType::Webp => "webp",
        };
        format!("private/catalogue/{digest}.{extension}")
    }
}
impl MediaRightsChange {
    pub fn validate(&self, asset: &MediaAssetDraft) -> Result<(), MediaValidationError> {
        if let Self::Grant {
            use_scope,
            valid_from,
            term,
            permission_ref,
        } = self
        {
            if *valid_from < 0 || !bounded_text(permission_ref, 512) {
                return Err(MediaValidationError);
            }
            if let RightsTerm::Until { expires_at } = term
                && *expires_at <= *valid_from
            {
                return Err(MediaValidationError);
            }
            if *use_scope == MediaUse::ProductPhotography
                && (asset.fixture || asset.kind != MediaKind::OriginalPhoto)
            {
                return Err(MediaValidationError);
            }
        }
        Ok(())
    }
    /// Valid rights alone do not prove decoded image content, SKU suitability or publication.
    pub fn product_rights_active(&self, asset: &MediaAssetDraft, now: i64) -> bool {
        if asset.validate().is_err() || self.validate(asset).is_err() {
            return false;
        }
        if let Self::Grant {
            use_scope,
            valid_from,
            term,
            ..
        } = self
        {
            if *use_scope != MediaUse::ProductPhotography || now < *valid_from {
                return false;
            }
            if let RightsTerm::Until { expires_at } = term
                && now >= *expires_at
            {
                return false;
            }
            return true;
        }
        false
    }
}
