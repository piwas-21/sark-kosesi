use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub struct IntentFile(PathBuf);
impl IntentFile {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "synthetic-recovery-intent-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("private synthetic intent file");
        Self(path)
    }
    pub fn write(&self, value: &Value) {
        fs::write(&self.0, serde_json::to_vec(value).expect("fixture JSON"))
            .expect("write isolated intent");
    }
    pub fn path(&self) -> &str {
        self.0.to_str().expect("test path")
    }
}
impl Drop for IntentFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
