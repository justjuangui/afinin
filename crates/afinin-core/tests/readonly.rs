// SPDX-License-Identifier: MIT OR Apache-2.0

//! NFR-1 / acceptance criterion: Afinín never calls an AWS API that changes
//! anything. Every AWS SDK operation in the source must be a read (Describe,
//! List, Get, Search, Select…). Non-mutating exceptions, such as the SSO
//! OIDC token exchange, need a `// readonly-allow: <reason>` comment on the
//! line above.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Verbs of AWS operations that create, change or delete something.
const WRITE_VERBS: &[&str] = &[
    "create",
    "delete",
    "put",
    "update",
    "modify",
    "terminate",
    "start",
    "stop",
    "run",
    "attach",
    "detach",
    "associate",
    "disassociate",
    "tag",
    "untag",
    "reboot",
    "restore",
    "copy",
    "import",
    "register",
    "deregister",
    "enable",
    "disable",
    "cancel",
    "reset",
    "replace",
    "revoke",
    "authorize",
    "purchase",
    "accept",
    "reject",
    "send",
    "invoke",
    "publish",
    "execute",
    "batch_write",
    "batch_delete",
    "upload",
    "release",
    "allocate",
    "apply",
    "set",
    "add",
    "remove",
];

/// Every operation name of the AWS SDK crates Afinín depends on, taken from
/// each crate's `src/operation/<name>/` directories via `cargo metadata`.
fn sdk_operations() -> HashSet<String> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata");
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata JSON");
    let mut ops = HashSet::new();
    for pkg in meta["packages"].as_array().unwrap() {
        if !pkg["name"]
            .as_str()
            .unwrap_or_default()
            .starts_with("aws-sdk-")
        {
            continue;
        }
        let dir = Path::new(pkg["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .join("src/operation");
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                ops.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    ops
}

fn is_write(op: &str) -> bool {
    WRITE_VERBS
        .iter()
        .any(|v| op == *v || op.starts_with(&format!("{v}_")))
}

/// Calls of AWS operations with a write verb on a line, e.g. `.delete_volume()`.
fn write_calls(line: &str, ops: &HashSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find('.') {
        rest = &rest[i + 1..];
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        if rest[ident.len()..].starts_with('(') && ops.contains(&ident) && is_write(&ident) {
            found.push(ident);
        }
    }
    found
}

fn scan(dir: &Path, ops: &HashSet<String>, violations: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, ops, violations);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let mut previous = "";
            for (n, line) in text.lines().enumerate() {
                if !previous.contains("readonly-allow:") {
                    for call in write_calls(line, ops) {
                        violations.push(format!("{}:{}: .{call}()", path.display(), n + 1));
                    }
                }
                previous = line;
            }
        }
    }
}

#[test]
fn no_aws_write_operations() {
    let ops = sdk_operations();
    assert!(
        ops.len() > 500,
        "found only {} SDK operations; is the registry readable?",
        ops.len()
    );
    let mut violations = Vec::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    scan(&root.join("src"), &ops, &mut violations);
    scan(
        &root.join("../../apps/desktop/src-tauri/src"),
        &ops,
        &mut violations,
    );
    assert!(
        violations.is_empty(),
        "AWS write operations found (NFR-1):\n{}",
        violations.join("\n")
    );
}

#[test]
fn detector_catches_writes() {
    let ops: HashSet<String> = [
        "delete_volume",
        "describe_volumes",
        "terminate_instances",
        "create_token",
    ]
    .map(String::from)
    .into();
    assert_eq!(
        write_calls("        .delete_volume()", &ops),
        vec!["delete_volume"]
    );
    assert_eq!(
        write_calls("client.terminate_instances().send()", &ops),
        vec!["terminate_instances"]
    );
    assert!(write_calls("client.describe_volumes().send()", &ops).is_empty());
    // Getters and our own methods are not SDK operations.
    assert!(write_calls("v.create_time()", &ops).is_empty());
    assert!(write_calls("db.reset_stale_syncs()", &ops).is_empty());
}
