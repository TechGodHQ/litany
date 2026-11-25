use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

const CLI: &str = env!("CARGO_BIN_EXE_litany-cli");
const MCP: &str = env!("CARGO_BIN_EXE_litany-mcp");
const NAME: &str = "Read C:\\Inbox \"Today\"\nLater 100%_Done terminal\"";

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, i64) {
    let dir = tempfile::tempdir().expect("isolated fixture directory");
    let database = dir.path().join("synthetic.db");
    let created = cli(
        &database,
        &dir,
        [
            "task-create",
            "--name",
            NAME,
            "--due",
            "2026-11-24",
            "--anchor",
            "2026-11-24",
            "--created-at",
            "2026-11-24T12:00:00",
        ],
    );
    assert_eq!(created["name"], NAME);
    let id = created["id"].as_i64().expect("task id");
    let archived = cli(&database, &dir, ["task-archive", &id.to_string()]);
    assert_eq!(archived["archived"], true);
    assert_eq!(archived["name"], NAME);
    (dir, database, id)
}

fn cli<const N: usize>(
    database: &std::path::Path,
    dir: &tempfile::TempDir,
    args: [&str; N],
) -> Value {
    cli_args(database, dir, args)
}

fn cli_args<I, S>(database: &std::path::Path, dir: &tempfile::TempDir, args: I) -> Value
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new(CLI)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .env("LITANY_DB", database)
        .current_dir(dir.path())
        .output()
        .expect("run actual Litany CLI process");
    assert!(
        output.status.success(),
        "CLI failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI returned valid JSON")
}

fn cli_list(
    database: &std::path::Path,
    dir: &tempfile::TempDir,
    query: Option<&str>,
    recurring: Option<bool>,
) -> Value {
    cli_list_filtered(database, dir, query, recurring, None, None)
}

fn cli_list_filtered(
    database: &std::path::Path,
    dir: &tempfile::TempDir,
    query: Option<&str>,
    recurring: Option<bool>,
    due_before: Option<&str>,
    satisfied: Option<&str>,
) -> Value {
    let mut args = vec![
        "task-list".to_owned(),
        "--archived".to_owned(),
        "true".to_owned(),
    ];
    if let Some(query) = query {
        args.push("--q".into());
        args.push(query.to_owned());
    }
    if let Some(recurring) = recurring {
        args.push("--recurring".into());
        args.push(recurring.to_string());
    }
    if let Some(due_before) = due_before {
        args.push("--due-before".into());
        args.push(due_before.to_owned());
    }
    if let Some(satisfied) = satisfied {
        args.push("--satisfied".into());
        args.push(satisfied.to_owned());
    }
    cli_args(database, dir, args)
}

fn mcp_list(
    database: &std::path::Path,
    dir: &tempfile::TempDir,
    query: Option<Value>,
    recurring: Option<bool>,
) -> Value {
    mcp_list_filtered(database, dir, query, recurring, None, None)
}

fn mcp_list_filtered(
    database: &std::path::Path,
    dir: &tempfile::TempDir,
    query: Option<Value>,
    recurring: Option<bool>,
    due_before: Option<&str>,
    satisfied: Option<&str>,
) -> Value {
    let mut arguments = serde_json::Map::new();
    arguments.insert("archived".into(), json!("true"));
    if let Some(query) = query {
        arguments.insert("q".into(), query);
    }
    if let Some(recurring) = recurring {
        arguments.insert("recurring".into(), json!(recurring));
    }
    if let Some(due_before) = due_before {
        arguments.insert("due_before".into(), json!(due_before));
    }
    if let Some(satisfied) = satisfied {
        arguments.insert("satisfied".into(), json!(satisfied));
    }
    let requests = [
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "litany-literal-query-test", "version": "1"}
            }
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "task_list", "arguments": arguments}
        }),
    ];
    let mut wire = String::new();
    for request in requests {
        wire.push_str(&serde_json::to_string(&request).unwrap());
        wire.push('\n');
    }
    let mut child = Command::new(MCP)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .env("LITANY_DB", database)
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start actual Litany MCP process");
    child
        .stdin
        .take()
        .expect("MCP stdin")
        .write_all(wire.as_bytes())
        .expect("send MCP initialize and tool call");
    let output = child
        .wait_with_output()
        .expect("wait for actual MCP process");
    assert!(
        output.status.success(),
        "MCP failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response = String::from_utf8(output.stdout)
        .expect("MCP stdout is UTF-8")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("MCP response is JSON"))
        .find(|response| response["id"] == 2)
        .expect("MCP task_list response");
    assert!(response.get("error").is_none(), "MCP error: {response}");
    assert_ne!(
        response["result"]["isError"], true,
        "MCP tool error: {response}"
    );
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP task_list text result"),
    )
    .expect("MCP task_list returned JSON")
}

fn ids(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .expect("task_list returned an array")
        .iter()
        .map(|task| task["id"].as_i64().expect("numeric task id"))
        .collect()
}

#[test]
fn cli_and_mcp_preserve_literal_queries_and_existing_filter_semantics() {
    let (dir, database, id) = fixture();
    let before_history = cli(&database, &dir, ["task-history", &id.to_string()]);
    assert_eq!(before_history, json!([]));
    let mut failures = Vec::new();

    let cases = [
        ("backslash", "C:\\Inbox"),
        ("embedded quotes", "\"Today\""),
        ("terminal quote", "terminal\""),
        ("newline", "\nLater"),
        ("mixed case", "rEaD c:"),
        ("percent and underscore", "100%_dOnE"),
    ];
    for (label, query) in cases {
        let cli_ids = ids(&cli_list(&database, &dir, Some(query), None));
        if cli_ids != [id] {
            failures.push(format!("CLI {label}: expected [{id}], got {cli_ids:?}"));
        }
        let mcp_ids = ids(&mcp_list(&database, &dir, Some(json!(query)), None));
        if mcp_ids != [id] {
            failures.push(format!("MCP {label}: expected [{id}], got {mcp_ids:?}"));
        }
    }

    let cli_empty = cli_list(&database, &dir, Some(""), None);
    let cli_omitted = cli_list(&database, &dir, None, None);
    if ids(&cli_empty) != [id] || cli_empty != cli_omitted {
        failures.push("CLI empty/omitted query control changed".into());
    }

    let mcp_empty = mcp_list(&database, &dir, Some(json!("")), None);
    let mcp_omitted = mcp_list(&database, &dir, None, None);
    let mcp_null = mcp_list(&database, &dir, Some(Value::Null), None);
    if ids(&mcp_empty) != [id] || mcp_empty != mcp_omitted || mcp_null != mcp_omitted {
        failures.push("MCP empty/null/omitted query control changed".into());
    }

    if !ids(&cli_list(&database, &dir, Some("no-match"), None)).is_empty() {
        failures.push("CLI unmatched query returned a task".into());
    }
    if !ids(&mcp_list(&database, &dir, Some(json!("no-match")), None)).is_empty() {
        failures.push("MCP unmatched query returned a task".into());
    }

    if ids(&cli_list(&database, &dir, Some("rEaD c:"), Some(false))) != [id]
        || ids(&mcp_list(
            &database,
            &dir,
            Some(json!("rEaD c:")),
            Some(false),
        )) != [id]
        || !ids(&cli_list(&database, &dir, None, Some(true))).is_empty()
        || !ids(&mcp_list(&database, &dir, None, Some(true))).is_empty()
    {
        failures.push("CLI/MCP recurring boolean filter or composed query changed".into());
    }

    if ids(&cli_list_filtered(
        &database,
        &dir,
        Some("rEaD c:"),
        Some(false),
        Some("2026-11-25"),
        Some("false"),
    )) != [id]
        || ids(&mcp_list_filtered(
            &database,
            &dir,
            Some(json!("rEaD c:")),
            Some(false),
            Some("2026-11-25"),
            Some("false"),
        )) != [id]
        || !ids(&cli_list_filtered(
            &database,
            &dir,
            None,
            None,
            None,
            Some("true"),
        ))
        .is_empty()
        || !ids(&mcp_list_filtered(
            &database,
            &dir,
            None,
            None,
            None,
            Some("true"),
        ))
        .is_empty()
    {
        failures.push("CLI/MCP due_before or satisfied filter composition changed".into());
    }

    let after_history = cli(&database, &dir, ["task-history", &id.to_string()]);
    assert_eq!(
        after_history, before_history,
        "list adapter calls do not write occurrences"
    );
    assert!(
        failures.is_empty(),
        "literal-query regressions:\n{}",
        failures.join("\n")
    );
}
