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
    let manifest = PathBuf::from(manifest);
    let worker = manifest.join("../worker/src/callbacks.rs").canonicalize()?;
    let worker = worker
        .to_str()
        .ok_or_else(|| io::Error::other("Worker source path must be UTF-8"))?;
    let declaration = format!(
        "#[allow(dead_code)]\n#[path = {worker:?}]\npub(crate) mod callback_worker_fixture;\n"
    );
    fs::write(
        PathBuf::from(&output).join("callback_worker_fixture.rs"),
        declaration,
    )?;
    let reversal_worker = manifest
        .join("../worker/src/reversal_pages.rs")
        .canonicalize()?;
    let reversal_worker = reversal_worker
        .to_str()
        .ok_or_else(|| io::Error::other("Worker source path must be UTF-8"))?;
    let declaration = format!(
        "#[allow(dead_code)]\n#[path = {reversal_worker:?}]\npub(crate) mod reversal_worker_fixture;\n"
    );
    fs::write(
        PathBuf::from(&output).join("reversal_worker_fixture.rs"),
        declaration,
    )?;
    let item_worker = manifest
        .join("../worker/src/reversal_items.rs")
        .canonicalize()?;
    let item_worker = item_worker
        .to_str()
        .ok_or_else(|| io::Error::other("Worker source path must be UTF-8"))?;
    let declaration = format!(
        "#[allow(dead_code)]\n#[path = {item_worker:?}]\npub(crate) mod reversal_item_worker_fixture;\n"
    );
    fs::write(
        PathBuf::from(output).join("reversal_item_worker_fixture.rs"),
        declaration,
    )?;
    println!("cargo:rerun-if-changed=../worker/src/reversal_items.rs");
    println!("cargo:rerun-if-changed=../worker/src/reversal_pages.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../worker/src/callbacks.rs");
    Ok(())
}
