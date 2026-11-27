//! End-to-end regression tests for the CLI and MCP adapter boundaries.
//!
//! Both adapters translate generated command/argument parameters into the
//! shared `GeneratedOperationInput`. These tests drive the real binaries so
//! the translation itself — not a re-implementation of it — stays pinned:
//! literal `q` text (quotes, SQL wildcard characters, backslashes) must
//! reach the store exactly as typed, while non-scalar and omitted
//! parameters keep their existing rendering.

use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

/// Task names chosen so SQL wildcard characters, quotes, and backslashes
/// must all match literally. `Routine 100xyDone` is the wildcard decoy: it
/// matches `100%_Done` under LIKE semantics and must never be returned.
const NAMES: [&str; 5] = [
    "Routine 100%_Done",
    "quote \"inside\" name",
    "back\\slash",
    "edge \"quoted\" tail",
    "Routine 100xyDone",
];

fn fixture() -> (TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let database = directory.path().join("litany.db");
    for name in NAMES {
        let output = Command::new(env!("CARGO_BIN_EXE_litany-cli"))
            .args([
                "task-create",
                "--name",
                name,
                "--anchor",
                "2026-11-01",
                "--created-at",
                "2026-11-01T00:00:00",
            ])
            .env("LITANY_DB", &database)
            .output()
            .expect("run litany-cli task-create");
        assert!(
            output.status.success(),
            "creating {name:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // A dated recurring task exercises non-string query parameters.
    let output = Command::new(env!("CARGO_BIN_EXE_litany-cli"))
        .args([
            "task-create",
            "--name",
            "daily grind",
            "--due",
            "2026-11-01",
            "--recurrence",
            "{\"kind\":\"daily\"}",
            "--anchor",
            "2026-11-01",
            "--created-at",
            "2026-11-01T00:00:00",
        ])
        .env("LITANY_DB", &database)
        .output()
        .expect("run litany-cli task-create");
    assert!(
        output.status.success(),
        "creating recurring task failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (directory, database)
}

fn cli(db: &std::path::Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_litany-cli"))
        .args(args)
        .env("LITANY_DB", db)
        .output()
        .expect("run litany-cli");
    assert!(
        output.status.success(),
        "litany-cli {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "litany-cli {args:?} printed invalid JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn names(tasks: &Value) -> Vec<&str> {
    tasks
        .as_array()
        .expect("task list is an array")
        .iter()
        .map(|task| task["name"].as_str().expect("task name is a string"))
        .collect()
}

fn name_set(tasks: &Value) -> BTreeSet<&str> {
    names(tasks).into_iter().collect()
}

#[test]
fn cli_boundary_preserves_literal_queries() {
    let (_directory, database) = fixture();

    for (query, expected) in [
        ("100%_Done", vec!["Routine 100%_Done"]),
        ("100%", vec!["Routine 100%_Done"]),
        ("quote \"inside\" name", vec!["quote \"inside\" name"]),
        ("back\\slash", vec!["back\\slash"]),
        ("\"quoted\"", vec!["edge \"quoted\" tail"]),
        ("rOuTiNe", vec!["Routine 100%_Done", "Routine 100xyDone"]),
        ("no such text", vec![]),
    ] {
        let tasks = cli(&database, &["task-list", "--q", query]);
        assert_eq!(names(&tasks), expected, "literal query {query:?}");
    }

    // Omitted q keeps listing every task, and the empty string stays
    // omitted rather than filtering everything out.
    let all = cli(&database, &["task-list"]);
    assert_eq!(name_set(&all).len(), NAMES.len() + 1);
    assert_eq!(
        name_set(&cli(&database, &["task-list", "--q", ""])),
        name_set(&all)
    );

    // Non-string query parameters keep their JSON-text rendering.
    assert_eq!(
        names(&cli(&database, &["task-list", "--recurring", "true"])),
        vec!["daily grind"]
    );
    assert_eq!(
        names(&cli(&database, &["task-list", "--recurring", "false"])),
        NAMES.to_vec()
    );
}

#[test]
fn mcp_boundary_preserves_literal_queries() {
    let (_directory, database) = fixture();

    let call = |arguments: Value| -> Value {
        let mut child = Command::new(env!("CARGO_BIN_EXE_litany-mcp"))
            .env("LITANY_DB", &database)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn litany-mcp");
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {"name": "task_list", "arguments": arguments},
        });
        {
            let stdin = child.stdin.as_mut().expect("litany-mcp stdin");
            writeln!(stdin, "{request}").expect("write request");
            stdin.flush().expect("flush request");
        }
        drop(child.stdin.take());
        let output = child.wait_with_output().expect("litany-mcp exits");
        assert!(
            output.status.success(),
            "litany-mcp task_list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "invalid JSON-RPC response ({error}): {}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(response["id"], json!(7), "response: {response}");
        assert_ne!(
            response["result"]["isError"],
            json!(true),
            "tool call errored: {response}"
        );
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool result text");
        serde_json::from_str(text).expect("tool result is JSON")
    };

    for (query, expected) in [
        (json!("100%_Done"), vec!["Routine 100%_Done"]),
        (
            json!("quote \"inside\" name"),
            vec!["quote \"inside\" name"],
        ),
        (json!("\"quoted\""), vec!["edge \"quoted\" tail"]),
        (json!("back\\slash"), vec!["back\\slash"]),
    ] {
        let tasks = call(json!({"q": query}));
        assert_eq!(names(&tasks), expected, "literal MCP query {query}");
    }

    // Omitted q lists every task; a JSON bool keeps its rendering.
    assert_eq!(name_set(&call(json!({}))).len(), NAMES.len() + 1);
    assert_eq!(
        names(&call(json!({"recurring": true}))),
        vec!["daily grind"]
    );
}
