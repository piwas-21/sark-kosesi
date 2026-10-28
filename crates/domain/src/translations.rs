//! Copy remains a draft until an independent, qualified reader records review.
use crate::specifications::bounded_text;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslationDraft {
    pub title: String,
    pub description: String,
    pub materials: String,
    pub care: String,
    pub safety: String,
    pub assembly: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTranslation;
impl TranslationDraft {
    pub fn validate(&self) -> Result<(), InvalidTranslation> {
        for (text, maximum) in [
            (&self.title, 160),
            (&self.description, 4000),
            (&self.materials, 2000),
            (&self.care, 2000),
            (&self.safety, 2000),
            (&self.assembly, 2000),
        ] {
            if !bounded_text(text, maximum) || text.contains('\0') {
                return Err(InvalidTranslation);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationDecision {
    Approved,
    Rejected,
}
impl TranslationDecision {
    pub fn code(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_copy_has_bounded_explicit_fields_in_every_language() {
        let value = TranslationDraft {
            title: "isolated fixture / العربية / Kurmancî".into(),
            description: "fixture description".into(),
            materials: "fixture material, never supplier evidence".into(),
            care: "fixture care\nsecond line".into(),
            safety: "fixture safety".into(),
            assembly: "fixture assembly".into(),
        };
        assert_eq!(value.validate(), Ok(()));
        for field in 0..6 {
            for invalid in [" ".into(), "x".repeat(4001), "hidden\0text".into()] {
                let mut changed = value.clone();
                match field {
                    0 => changed.title = invalid,
                    1 => changed.description = invalid,
                    2 => changed.materials = invalid,
                    3 => changed.care = invalid,
                    4 => changed.safety = invalid,
                    5 => changed.assembly = invalid,
                    _ => unreachable!(),
                }
                assert_eq!(changed.validate(), Err(InvalidTranslation));
            }
        }
        assert_eq!(TranslationDecision::Approved.code(), "approved");
        assert_eq!(TranslationDecision::Rejected.code(), "rejected");
    }
}
