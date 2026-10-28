//! No-allocation WebP framing before the decoder can follow declared chunk ranges.
use super::ImageError;

pub(super) fn webp(bytes: &[u8]) -> Result<(), ImageError> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(ImageError::InvalidImage);
    }
    let declared = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if usize::try_from(declared)
        .ok()
        .and_then(|n| n.checked_add(8))
        != Some(bytes.len())
    {
        return Err(ImageError::ResourceLimit);
    }
    let mut offset = 12_usize;
    let mut canvas = None;
    let mut native = None;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(ImageError::ResourceLimit)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(ImageError::InvalidImage)?;
        let length = usize::try_from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ]))
        .map_err(|_| ImageError::ResourceLimit)?;
        let data_end = header_end
            .checked_add(length)
            .ok_or(ImageError::ResourceLimit)?;
        let next = data_end
            .checked_add(length % 2)
            .ok_or(ImageError::ResourceLimit)?;
        if next > bytes.len() {
            return Err(ImageError::ResourceLimit);
        }
        let data = &bytes[header_end..data_end];
        match &header[..4] {
            // The decoder inspects frame subchunks even with a clear animation flag.
            b"ANIM" | b"ANMF" => return Err(ImageError::UnsupportedImage),
            b"VP8X" => {
                if data.len() != 10 || canvas.is_some() || offset != 12 {
                    return Err(ImageError::InvalidImage);
                }
                if data[0] & 2 != 0 {
                    return Err(ImageError::UnsupportedImage);
                }
                canvas = Some((
                    u32::from_le_bytes([data[4], data[5], data[6], 0]) + 1,
                    u32::from_le_bytes([data[7], data[8], data[9], 0]) + 1,
                ));
            }
            b"VP8 " | b"VP8L" => {
                if native.is_some() {
                    return Err(ImageError::InvalidImage);
                }
                native = Some(native_dimensions(&header[..4], data)?);
            }
            _ => {}
        }
        offset = next;
    }
    let native = native.ok_or(ImageError::InvalidImage)?;
    // Extended canvas dimensions must not hide a larger bitstream allocation.
    if canvas.is_some_and(|size| size != native) {
        return Err(ImageError::InvalidImage);
    }
    Ok(())
}
fn native_dimensions(kind: &[u8], data: &[u8]) -> Result<(u32, u32), ImageError> {
    if kind == b"VP8L" {
        if data.len() < 5 || data[0] != 0x2f {
            return Err(ImageError::InvalidImage);
        }
        let header = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        Ok(((header & 0x3fff) + 1, ((header >> 14) & 0x3fff) + 1))
    } else {
        if data.len() < 10 || data[0] & 1 != 0 || &data[3..6] != b"\x9d\x01\x2a" {
            return Err(ImageError::InvalidImage);
        }
        Ok((
            u32::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff),
            u32::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff),
        ))
    }
}

#[cfg(test)]
#[path = "../../tests/support/image_framing.rs"]
mod tests;
