//! Private merchant definitions; creation never approves facts, prices or stock.
use crate::{ComponentKind, specifications::bounded_text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fill {
    CoversOnly,
    WithFoam,
}
pub fn matches_fill(fill: Fill, kinds: &[ComponentKind]) -> bool {
    kinds.contains(&ComponentKind::Cover)
        && match fill {
            Fill::CoversOnly => !kinds.contains(&ComponentKind::Foam),
            Fill::WithFoam => kinds.contains(&ComponentKind::Foam),
        }
}
impl Fill {
    pub fn code(self) -> &'static str {
        match self {
            Self::CoversOnly => "covers-only",
            Self::WithFoam => "with-foam",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BomLineDraft {
    pub component_code: String,
    pub quantity: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogueCommand {
    Family {
        code: String,
    },
    Component {
        code: String,
        component_kind: ComponentKind,
    },
    Variant {
        sku: String,
        family_code: String,
        size_code: String,
        textile_code: String,
        fill: Fill,
        lines: Vec<BomLineDraft>,
    },
    Bom {
        sku: String,
        expected_revision: i32,
        lines: Vec<BomLineDraft>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidDefinition;
/// Company identifiers are separate from supplier references and legacy DEMO data.
pub fn merchant_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 65
        && value != "DEMO"
        && !value.starts_with("DEMO-")
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}
fn valid_lines(lines: &[BomLineDraft]) -> bool {
    if lines.is_empty() || lines.len() > 64 {
        return false;
    }
    let mut seen = BTreeSet::new();
    lines.iter().all(|line| {
        merchant_code(&line.component_code)
            && (1..=10000).contains(&line.quantity)
            && seen.insert(&line.component_code)
    })
}
impl CatalogueCommand {
    pub fn validate(&self) -> Result<(), InvalidDefinition> {
        let valid = match self {
            Self::Family { code } | Self::Component { code, .. } => merchant_code(code),
            Self::Variant {
                sku,
                family_code,
                size_code,
                textile_code,
                lines,
                ..
            } => {
                merchant_code(sku)
                    && merchant_code(family_code)
                    && bounded_text(size_code, 80)
                    && !size_code.contains('\0')
                    && bounded_text(textile_code, 80)
                    && !textile_code.contains('\0')
                    && valid_lines(lines)
            }
            Self::Bom {
                sku,
                expected_revision,
                lines,
            } => {
                merchant_code(sku)
                    && (1..i32::MAX).contains(expected_revision)
                    && valid_lines(lines)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(InvalidDefinition)
        }
    }
    /// Validate collection/string bounds before cloning or sorting untrusted input.
    pub fn normalized(&self) -> Result<Self, InvalidDefinition> {
        self.validate()?;
        let mut result = self.clone();
        match &mut result {
            Self::Variant { lines, .. } | Self::Bom { lines, .. } => {
                lines.sort_by(|a, b| a.component_code.cmp(&b.component_code))
            }
            Self::Family { .. } | Self::Component { .. } => {}
        }
        Ok(result)
    }
    pub fn bom_revision(&self) -> Option<i32> {
        match self {
            Self::Variant { .. } => Some(1),
            Self::Bom {
                expected_revision, ..
            } => expected_revision.checked_add(1),
            Self::Family { .. } | Self::Component { .. } => None,
        }
    }
}
