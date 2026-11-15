//! Pure, bounded image processing for the isolated decoder.
#![forbid(unsafe_code)]

mod framing;
mod processing;
pub mod wire;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Webp,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Prepare { kind: Kind, width: u32, height: u32 },
    Validate { kind: Kind, width: u32, height: u32 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    MetadataMismatch,
    InvalidImage,
    UnsupportedImage,
    ResourceLimit,
    InvalidStoredData,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendition {
    pub edge: i32,
    pub width: i32,
    pub height: i32,
    pub kind: Kind,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub width: i32,
    pub height: i32,
    pub renditions: Vec<Rendition>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Prepared(Prepared),
    Valid,
    Rejected(Rejection),
}

pub const MAX_ORIGINAL_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_RENDITION_BYTES: usize = 250 * 1024;

/// Process one bounded original or stored derivative. Digest custody belongs to the caller.
pub fn process(operation: Operation, bytes: &[u8]) -> Response {
    let result = match operation {
        Operation::Prepare {
            kind,
            width,
            height,
        } => processing::prepare(kind, width, height, bytes).map(Response::Prepared),
        Operation::Validate {
            kind,
            width,
            height,
        } => processing::validate(kind, width, height, bytes).map(|()| Response::Valid),
    };
    result.unwrap_or_else(Response::Rejected)
}

#[cfg(test)]
mod processing_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_tests;
