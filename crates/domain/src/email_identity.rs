//! Narrow ASCII mailbox identity, not evidence that a mailbox exists.

/// Preserves the local part exactly and folds only the ASCII DNS domain.
pub fn normalize_email(value: &str) -> Option<String> {
    if value.len() > 254 || !value.is_ascii() {
        return None;
    }
    let (local, domain) = value.split_once('@')?;
    if local.is_empty() || local.len() > 64 || domain.is_empty() || domain.len() > 253 {
        return None;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return None;
    }
    if !local
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~.".contains(&byte))
    {
        return None;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2
        || labels.last().is_none_or(|label| {
            label.len() < 2 || !label.bytes().any(|byte| byte.is_ascii_alphabetic())
        })
    {
        return None;
    }
    if !labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return None;
    }
    Some(format!("{local}@{}", domain.to_ascii_lowercase()))
}
