//! Canonical decimal revisions for private test transports.
pub(crate) fn revision(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 19
        || (bytes.len() > 1 && bytes[0] == b'0')
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    value.parse().ok()
}
