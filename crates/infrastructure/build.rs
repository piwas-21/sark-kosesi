//! Canonical test wiring for the actual worker source, without runtime activation.
#![forbid(unsafe_code)]
use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let manifest = env::var_os("CARGO_MANIFEST_DIR")
        .ok_or_else(|| io::Error::other("Cargo manifest directory required"))?;
    let output = env::var_os("OUT_DIR")
        .ok_or_else(|| io::Error::other("Cargo output directory required"))?;
    // This fixed workspace source is compiled by both the worker binary and the
    // authenticated loopback fixture. Canonical identity keeps native coverage
    // attributable to one physical source file; no counters are transformed.
    let worker = PathBuf::from(manifest)
        .join("../worker/src/callbacks.rs")
        .canonicalize()?;
    let worker = worker
        .to_str()
        .ok_or_else(|| io::Error::other("Worker source path must be UTF-8"))?;
    let declaration = format!(
        "#[allow(dead_code)]\n#[path = {worker:?}]\npub(crate) mod callback_worker_fixture;\n"
    );
    fs::write(
        PathBuf::from(output).join("callback_worker_fixture.rs"),
        declaration,
    )?;
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../worker/src/callbacks.rs");
    Ok(())
}
