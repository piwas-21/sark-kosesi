//! Application policies shared by the API and database projection.
#![forbid(unsafe_code)]

pub mod request_context;
pub mod staff_access;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    Nl,
    De,
    Fr,
    En,
    Ar,
    Tr,
    Kmr,
}

impl Locale {
    pub fn from_public(value: &str) -> Option<Self> {
        match value {
            "nl" => Some(Self::Nl),
            "de" => Some(Self::De),
            "fr" => Some(Self::Fr),
            "en" => Some(Self::En),
            "ar" => Some(Self::Ar),
            "tr" => Some(Self::Tr),
            "ku" => Some(Self::Kmr),
            _ => None,
        }
    }
    pub fn internal(self) -> &'static str {
        match self {
            Self::Nl => "nl",
            Self::De => "de",
            Self::Fr => "fr",
            Self::En => "en",
            Self::Ar => "ar",
            Self::Tr => "tr",
            Self::Kmr => "kmr",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Locale;
    #[test]
    fn public_and_internal_languages_have_explicit_mapping() {
        for (public, internal) in [
            ("nl", "nl"),
            ("de", "de"),
            ("fr", "fr"),
            ("en", "en"),
            ("ar", "ar"),
            ("tr", "tr"),
            ("ku", "kmr"),
        ] {
            assert_eq!(
                Locale::from_public(public).map(Locale::internal),
                Some(internal)
            );
        }
        for unsupported in ["kmr", "ckb", "", "EN", "../../", "ku-Arab"] {
            assert_eq!(Locale::from_public(unsupported), None);
        }
    }
}
