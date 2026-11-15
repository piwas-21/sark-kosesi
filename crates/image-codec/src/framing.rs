//! No-allocation WebP framing before the decoder can follow declared chunk ranges.
use super::Rejection;

pub(crate) fn webp(bytes: &[u8]) -> Result<(), Rejection> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(Rejection::InvalidImage);
    }
    let declared = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if usize::try_from(declared)
        .ok()
        .and_then(|n| n.checked_add(8))
        != Some(bytes.len())
    {
        return Err(Rejection::ResourceLimit);
    }
    let mut offset = 12_usize;
    let mut canvas = None;
    let mut native = None;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(Rejection::ResourceLimit)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(Rejection::InvalidImage)?;
        let length = usize::try_from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ]))
        .map_err(|_| Rejection::ResourceLimit)?;
        let data_end = header_end
            .checked_add(length)
            .ok_or(Rejection::ResourceLimit)?;
        let next = data_end
            .checked_add(length % 2)
            .ok_or(Rejection::ResourceLimit)?;
        if next > bytes.len() {
            return Err(Rejection::ResourceLimit);
        }
        let data = &bytes[header_end..data_end];
        match &header[..4] {
            // The decoder inspects frame subchunks even with a clear animation flag.
            b"ANIM" | b"ANMF" => return Err(Rejection::UnsupportedImage),
            b"VP8X" => {
                if data.len() != 10 || canvas.is_some() || offset != 12 {
                    return Err(Rejection::InvalidImage);
                }
                if data[0] & 2 != 0 {
                    return Err(Rejection::UnsupportedImage);
                }
                canvas = Some((
                    u32::from_le_bytes([data[4], data[5], data[6], 0]) + 1,
                    u32::from_le_bytes([data[7], data[8], data[9], 0]) + 1,
                ));
            }
            b"VP8 " | b"VP8L" => {
                if native.is_some() {
                    return Err(Rejection::InvalidImage);
                }
                native = Some(native_dimensions(&header[..4], data)?);
            }
            _ => {}
        }
        offset = next;
    }
    let native = native.ok_or(Rejection::InvalidImage)?;
    // Extended canvas dimensions must not hide a larger bitstream allocation.
    if canvas.is_some_and(|size| size != native) {
        return Err(Rejection::InvalidImage);
    }
    Ok(())
}
fn native_dimensions(kind: &[u8], data: &[u8]) -> Result<(u32, u32), Rejection> {
    if kind == b"VP8L" {
        if data.len() < 5 || data[0] != 0x2f {
            return Err(Rejection::InvalidImage);
        }
        let header = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        Ok(((header & 0x3fff) + 1, ((header >> 14) & 0x3fff) + 1))
    } else {
        if data.len() < 10 || data[0] & 1 != 0 || &data[3..6] != b"\x9d\x01\x2a" {
            return Err(Rejection::InvalidImage);
        }
        Ok((
            u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff),
            u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff),
        ))
    }
}
#[cfg(test)]
#[path = "framing_tests.rs"]
mod tests;
