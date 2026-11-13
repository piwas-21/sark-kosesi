//! Describe this binary's embedded SQLx manifest without opening a database.
use commerce_infrastructure::MIGRATOR;
use serde::Serialize;
use sqlx::migrate::MigrationType;
use std::io::{self, Write};

#[derive(Serialize)]
struct Entry {
    version: String,
    kind: &'static str,
    checksum_sha384: String,
}

#[derive(Serialize)]
struct Manifest {
    schema_version: u8,
    ignore_missing: bool,
    migrations: Vec<Entry>,
}

pub(super) fn print() -> Result<(), Box<dyn std::error::Error>> {
    let entries = MIGRATOR
        .iter()
        .map(|migration| Entry {
            version: migration.version.to_string(),
            kind: match migration.migration_type {
                MigrationType::Simple => "simple",
                MigrationType::ReversibleUp => "reversible_up",
                MigrationType::ReversibleDown => "reversible_down",
            },
            checksum_sha384: migration
                .checksum
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        })
        .collect();
    let manifest = Manifest {
        schema_version: 1,
        ignore_missing: MIGRATOR.ignore_missing,
        migrations: entries,
    };
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, &manifest)?;
    writeln!(writer)?;
    Ok(())
}
