//! Versioned, closed, bounded frames. The transport rejects extra request bytes and
//! completes the response at EOF.
use crate::{
    Kind, MAX_ORIGINAL_BYTES, MAX_RENDITION_BYTES, Operation, Prepared, Rejection, Rendition,
    Response,
};

pub const HEADER_LEN: usize = 20;
// Header, prepared dimensions/count, three rendition headers and bounded byte payloads.
pub const MAX_RESPONSE_BYTES: usize = 20 + 3 * (20 + MAX_RENDITION_BYTES);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireError;

fn kind_byte(kind: Kind) -> u8 {
    match kind {
        Kind::Jpeg => 1,
        Kind::Png => 2,
        Kind::Webp => 3,
    }
}
fn kind(byte: u8) -> Result<Kind, WireError> {
    match byte {
        1 => Ok(Kind::Jpeg),
        2 => Ok(Kind::Png),
        3 => Ok(Kind::Webp),
        _ => Err(WireError),
    }
}
fn rejection_byte(value: Rejection) -> u8 {
    match value {
        Rejection::MetadataMismatch => 1,
        Rejection::InvalidImage => 2,
        Rejection::UnsupportedImage => 3,
        Rejection::ResourceLimit => 4,
        Rejection::InvalidStoredData => 5,
        Rejection::Unavailable => 6,
    }
}
fn rejection(byte: u8) -> Result<Rejection, WireError> {
    match byte {
        1 => Ok(Rejection::MetadataMismatch),
        2 => Ok(Rejection::InvalidImage),
        3 => Ok(Rejection::UnsupportedImage),
        4 => Ok(Rejection::ResourceLimit),
        5 => Ok(Rejection::InvalidStoredData),
        6 => Ok(Rejection::Unavailable),
        _ => Err(WireError),
    }
}
fn request_fields(
    operation: Operation,
    len: usize,
) -> Result<(u8, Kind, u32, u32, u32), WireError> {
    let (tag, kind, width, height, max_len, max_edge) = match operation {
        Operation::Prepare {
            kind,
            width,
            height,
        } => (1, kind, width, height, MAX_ORIGINAL_BYTES, 16384),
        Operation::Validate {
            kind,
            width,
            height,
        } => (2, kind, width, height, MAX_RENDITION_BYTES, 960),
    };
    if len == 0
        || len > max_len
        || width == 0
        || height == 0
        || width > max_edge
        || height > max_edge
        || (tag == 2 && kind == Kind::Webp)
    {
        return Err(WireError);
    }
    Ok((
        tag,
        kind,
        width,
        height,
        u32::try_from(len).map_err(|_| WireError)?,
    ))
}
pub fn request_header(operation: Operation, len: usize) -> Result<[u8; HEADER_LEN], WireError> {
    let (tag, kind, width, height, len) = request_fields(operation, len)?;
    let mut out = [0; HEADER_LEN];
    out[..4].copy_from_slice(b"IC01");
    out[4] = tag;
    out[5] = kind_byte(kind);
    out[8..12].copy_from_slice(&width.to_be_bytes());
    out[12..16].copy_from_slice(&height.to_be_bytes());
    out[16..20].copy_from_slice(&len.to_be_bytes());
    Ok(out)
}
pub fn parse_request_header(header: &[u8; HEADER_LEN]) -> Result<(Operation, usize), WireError> {
    if &header[..4] != b"IC01" || header[6..8] != [0, 0] {
        return Err(WireError);
    }
    let kind = kind(header[5])?;
    let width = u32::from_be_bytes(header[8..12].try_into().map_err(|_| WireError)?);
    let height = u32::from_be_bytes(header[12..16].try_into().map_err(|_| WireError)?);
    let len = usize::try_from(u32::from_be_bytes(
        header[16..20].try_into().map_err(|_| WireError)?,
    ))
    .map_err(|_| WireError)?;
    let operation = match header[4] {
        1 => Operation::Prepare {
            kind,
            width,
            height,
        },
        2 => Operation::Validate {
            kind,
            width,
            height,
        },
        _ => return Err(WireError),
    };
    request_fields(operation, len)?;
    Ok((operation, len))
}
fn check_prepared(prepared: &Prepared) -> Result<(), WireError> {
    if prepared.width <= 0
        || prepared.height <= 0
        || prepared.width > 16384
        || prepared.height > 16384
    {
        return Err(WireError);
    }
    let maximum = prepared.width.max(prepared.height);
    let count = if maximum <= 320 {
        1
    } else if maximum <= 640 {
        2
    } else {
        3
    };
    if prepared.renditions.len() != count {
        return Err(WireError);
    }
    for (r, edge) in prepared.renditions.iter().zip([320, 640, 960]) {
        if r.edge != edge
            || r.width <= 0
            || r.height <= 0
            || r.width > prepared.width
            || r.height > prepared.height
            || r.width.max(r.height) != edge.min(maximum)
            || !matches!(r.kind, Kind::Jpeg | Kind::Png)
            || r.bytes.is_empty()
            || r.bytes.len() > MAX_RENDITION_BYTES
        {
            return Err(WireError);
        }
    }
    Ok(())
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
pub fn encode_response(response: &Response) -> Result<Vec<u8>, WireError> {
    let mut out = b"ICR1".to_vec();
    match response {
        Response::Valid => out.extend_from_slice(&[0, 0, 0, 0]),
        Response::Rejected(reason) => out.extend_from_slice(&[1, rejection_byte(*reason), 0, 0]),
        Response::Prepared(prepared) => {
            check_prepared(prepared)?;
            out.extend_from_slice(&[2, 0, 0, 0]);
            put_u32(
                &mut out,
                u32::try_from(prepared.width).map_err(|_| WireError)?,
            );
            put_u32(
                &mut out,
                u32::try_from(prepared.height).map_err(|_| WireError)?,
            );
            put_u32(
                &mut out,
                u32::try_from(prepared.renditions.len()).map_err(|_| WireError)?,
            );
            for r in &prepared.renditions {
                put_u32(&mut out, u32::try_from(r.edge).map_err(|_| WireError)?);
                put_u32(&mut out, u32::try_from(r.width).map_err(|_| WireError)?);
                put_u32(&mut out, u32::try_from(r.height).map_err(|_| WireError)?);
                out.extend_from_slice(&[kind_byte(r.kind), 0, 0, 0]);
                put_u32(
                    &mut out,
                    u32::try_from(r.bytes.len()).map_err(|_| WireError)?,
                );
                out.extend_from_slice(&r.bytes);
            }
        }
    }
    if out.len() > MAX_RESPONSE_BYTES {
        return Err(WireError);
    }
    Ok(out)
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let end = self.offset.checked_add(len).ok_or(WireError)?;
        let value = self.bytes.get(self.offset..end).ok_or(WireError)?;
        self.offset = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| WireError)?,
        ))
    }
    fn i32(&mut self) -> Result<i32, WireError> {
        i32::try_from(self.u32()?).map_err(|_| WireError)
    }
}
pub fn parse_response(bytes: &[u8]) -> Result<Response, WireError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(WireError);
    }
    let mut reader = Reader { bytes, offset: 0 };
    if reader.take(4)? != b"ICR1" {
        return Err(WireError);
    }
    let status = reader.take(4)?;
    let response = match status {
        [0, 0, 0, 0] => Response::Valid,
        [1, reason, 0, 0] => Response::Rejected(rejection(*reason)?),
        [2, 0, 0, 0] => {
            let width = reader.i32()?;
            let height = reader.i32()?;
            let count = reader.u32()?;
            if !(1..=3).contains(&count) {
                return Err(WireError);
            }
            let mut renditions = Vec::new();
            for _ in 0..count {
                let edge = reader.i32()?;
                let width = reader.i32()?;
                let height = reader.i32()?;
                let format = reader.take(4)?;
                if format[1..] != [0, 0, 0] {
                    return Err(WireError);
                }
                let kind = kind(format[0])?;
                let len = usize::try_from(reader.u32()?).map_err(|_| WireError)?;
                if len == 0 || len > MAX_RENDITION_BYTES {
                    return Err(WireError);
                }
                let bytes = reader.take(len)?.to_vec();
                renditions.push(Rendition {
                    edge,
                    width,
                    height,
                    kind,
                    bytes,
                });
            }
            let prepared = Prepared {
                width,
                height,
                renditions,
            };
            check_prepared(&prepared)?;
            Response::Prepared(prepared)
        }
        _ => return Err(WireError),
    };
    if reader.offset != bytes.len() {
        return Err(WireError);
    }
    Ok(response)
}
impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid image decoder frame")
    }
}
impl std::error::Error for WireError {}
