#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! `edda ... | head -1`: once the reader has gone, edda stops quietly.

use std::{env, io::pipe, process::Command};

#[test]
fn edda_stops_quietly_when_the_reader_has_gone() {
    let config = env::temp_dir().join(format!("edda-test-{}", std::process::id()));
    let (reader, writer) = pipe().unwrap();
    drop(reader);
    // Prints this device's key, made in a config directory of its own.
    let run = Command::new(env!("CARGO_BIN_EXE_edda"))
        .args(["auth", "key"])
        .env("EDDA_CONFIG_DIR", &config)
        .stdout(writer)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&config);
    let errors = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{:?}: {errors}", run.status);
    assert!(!errors.contains("panicked"), "{errors}");
}
