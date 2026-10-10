//! SR-011, R-SEC-04, CLAUDE.md rule 5: caelum-vehicle has no write API.
//!
//! The Rust type system cannot prove a method's absence, so this test documents and checks it
//! two ways, alongside the CI write guard (tools/ci/src/writes.ts, which scans `src/` with zero
//! exceptions):
//! 1. no source line in `src/` contains a send, write, connect or subscribe call;
//! 2. the receiver's public API is exactly the receive-side list below; adding any method fails
//!    this test until it is reviewed here.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use std::fs;
use std::path::{Path, PathBuf};

/// Mirrors WRITE_RULES and VEHICLE_ONLY_RULES of the CI guard (substrings, built with concat! so
/// this file does not itself look like a call site).
const FORBIDDEN: &[&str] = &[
    concat!(".", "write_all("),
    concat!(".", "write_vectored("),
    concat!("io::", "Write"),
    concat!("Async", "Write"),
    concat!("Write", "Type::"),
    concat!(".", "write_descriptor("),
    concat!(".", "write_ext("),
    concat!(".", "write_io("),
    concat!(".", "send_feature_report("),
    concat!(".", "send_output_report("),
    concat!(".", "send_to("),
    concat!(".", "send("),
    concat!(".", "connect("),
    concat!(".", "try_send"),
    concat!(".", "poll_send"),
    concat!(".", "set_broadcast("),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn code(line: &str) -> &str {
    line.split_once("//").map_or(line, |(c, _)| c)
}

/// SR-011: no write, send, connect or subscribe call anywhere in `src/`.
#[test]
fn sr_011_no_send_write_or_connect_call_in_source() {
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() >= 6, "found {files:?}");
    let mut findings = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read file");
        for (i, line) in text.lines().enumerate() {
            let c = code(line);
            for bad in FORBIDDEN {
                if c.contains(bad) {
                    findings.push(format!("{}:{}: {bad}", file.display(), i + 1));
                }
            }
            // `.write(` with an argument, and `.subscribe(` with an argument (BLE CCCD write).
            for call in [concat!(".", "write("), concat!(".", "subscribe(")] {
                if let Some(at) = c.find(call) {
                    let rest = c[at + call.len()..].trim_start();
                    if !rest.starts_with(')') {
                        findings.push(format!("{}:{}: {call}", file.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(findings.is_empty(), "write-capable calls: {findings:#?}");
}

/// SR-011: `VehicleReceiver`'s public API is receive-only. Reviewed list; extend only with a
/// method that cannot send.
#[test]
fn sr_011_receiver_public_api_is_receive_only() {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/receiver.rs"))
        .expect("read receiver");
    let mut methods: Vec<&str> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l
                .strip_prefix("pub async fn ")
                .or_else(|| l.strip_prefix("pub fn "))?;
            rest.split(['(', '<']).next()
        })
        .collect();
    methods.sort_unstable();
    assert_eq!(
        methods,
        [
            "bind",
            "config",
            "correlation",
            "local_addr",
            "no_station_time",
            "recv",
            "stats"
        ]
    );
    // The socket never leaves the receiver.
    assert!(!text.contains("pub socket"));
    assert!(!text.contains("-> &UdpSocket"));
    assert!(!text.contains("-> UdpSocket"));
}
