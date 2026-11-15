//! Single-use codec execution. Limits are installed before any request bytes are read.
use commerce_image_codec::{process, wire};
use rustix::process::{Resource, Rlimit, setrlimit};
use std::io::{self, Read, Write};

pub(crate) const MAX_INPUT: usize = 20 * 1024 * 1024;
// Initial isolation ceilings, not measured production capacity.
const ADDRESS_SPACE: u64 = 768 * 1024 * 1024;
const CPU_SECONDS: u64 = 10;

pub(crate) fn limits(address_space: u64, cpu_seconds: u64) -> io::Result<()> {
    for (resource, value) in [
        (Resource::As, address_space),
        (Resource::Cpu, cpu_seconds),
        (Resource::Core, 0),
        (Resource::Nofile, 16),
    ] {
        let cap = Some(value);
        setrlimit(
            resource,
            Rlimit {
                current: cap,
                maximum: cap,
            },
        )
        .map_err(io::Error::other)?;
    }
    Ok(())
}

pub(crate) fn run() -> io::Result<()> {
    limits(ADDRESS_SPACE, CPU_SECONDS)?;
    crate::sandbox::install()?;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut header = [0; wire::HEADER_LEN];
    input.read_exact(&mut header)?;
    let (operation, length) = wire::parse_request_header(&header).map_err(io::Error::other)?;
    if length == 0 || length > MAX_INPUT {
        return Err(io::Error::other("input length"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    let mut extra = [0; 1];
    if input.read(&mut extra)? != 0 {
        return Err(io::Error::other("trailing input"));
    }
    let response = process(operation, &bytes);
    let encoded = wire::encode_response(&response).map_err(io::Error::other)?;
    if encoded.len() > wire::MAX_RESPONSE_BYTES {
        return Err(io::Error::other("output length"));
    }
    io::stdout().lock().write_all(&encoded)
}
