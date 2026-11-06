use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::{http::StatusCode, response::IntoResponse, Router};
use chrono::{NaiveDate, NaiveDateTime};
use litany_core::{streak, AdvanceMode, Recurrence, Task};
use litany_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod generated {
    include!("../../../generated/http.rs");
}
pub mod generated_cli {
    include!("../../../generated/cli.rs");
}
pub const GENERATED_MCP_JSON: &str = include_str!("../../../generated/mcp.json");

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
}
impl AppState {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, OperationError> {
        Ok(Self {
            store: Arc::new(Mutex::new(Store::open(path).map_err(store_error)?)),
        })
    }
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(Mutex::new(Store::open_in_memory().expect("memory store"))),
        }
    }
}

#[derive(Debug)]
pub struct OperationError {
    pub status: StatusCode,
    pub message: String,
}
impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for OperationError {}
fn bad(message: impl Into<String>) -> OperationError {
    OperationError {
        status: StatusCode::BAD_REQUEST,
        message: message.into(),
    }
}
fn missing(message: impl Into<String>) -> OperationError {
    OperationError {
        status: StatusCode::NOT_FOUND,
        message: message.into(),
    }
}
fn store_error(error: litany_store::StoreError) -> OperationError {
    bad(error.to_string())
}
fn date(raw: &str) -> Result<NaiveDate, OperationError> {
    raw.parse()
        .map_err(|_| bad(format!("invalid UTC date: {raw}")))
}
fn timestamp(raw: &str) -> Result<NaiveDateTime, OperationError> {
    raw.parse()
        .map_err(|_| bad(format!("invalid UTC timestamp: {raw}")))
}
fn path_i64(input: &generated::GeneratedOperationInput, key: &str) -> Result<i64, OperationError> {
    input
        .path
        .get(key)
        .ok_or_else(|| bad(format!("missing path parameter: {key}")))?
        .parse()
        .map_err(|_| bad(format!("invalid integer path parameter: {key}")))
}
fn query<'a>(input: &'a generated::GeneratedOperationInput, key: &str) -> Option<&'a str> {
    input.query.get(key).map(String::as_str)
}
fn body<T: for<'a> Deserialize<'a>>(value: Value) -> Result<T, OperationError> {
    serde_json::from_value(value).map_err(|error| bad(format!("invalid request body: {error}")))
}
fn recurrence(value: Option<Value>) -> Result<Option<Recurrence>, OperationError> {
    value
        .map(|value| {
            match value {
                Value::String(raw) => serde_json::from_str(&raw),
                value => serde_json::from_value(value),
            }
            .map_err(|error| bad(format!("invalid recurrence: {error}")))
        })
        .transpose()
}
fn value<T: Serialize>(result: T) -> Result<Value, OperationError> {
    serde_json::to_value(result).map_err(|error| bad(format!("cannot serialize response: {error}")))
}

#[derive(Deserialize)]
struct Create {
    name: String,
    due: Option<String>,
    recurrence: Option<Value>,
    anchor: String,
    collapse: Option<bool>,
    advance: Option<AdvanceMode>,
    created_at: String,
}
#[derive(Deserialize)]
struct Update {
    name: String,
    due: Option<String>,
    recurrence: Option<Value>,
    anchor: String,
    collapse: Option<bool>,
    advance: Option<AdvanceMode>,
}
#[derive(Deserialize)]
struct Complete {
    at: String,
}
#[derive(Serialize)]
struct Streak {
    current: usize,
    best: usize,
    unit: &'static str,
}

pub async fn execute_operation(
    state: &AppState,
    operation: &str,
    input: generated::GeneratedOperationInput,
) -> Result<Value, OperationError> {
    let mut store = state.store.lock().map_err(|_| bad("store lock poisoned"))?;
    match operation {
        "task_create" => {
            let args: Create = body(input.body)?;
            let task = store
                .create_task(&Task {
                    id: 0,
                    name: args.name,
                    due: args.due.as_deref().map(date).transpose()?,
                    recurrence: recurrence(args.recurrence)?,
                    anchor: date(&args.anchor)?,
                    collapse: args.collapse.unwrap_or_default(),
                    advance: args.advance.unwrap_or_default(),
                    created_at: timestamp(&args.created_at)?,
                    archived: false,
                })
                .map_err(store_error)?;
            value(task)
        }
        "task_update" => {
            let id = path_i64(&input, "task_id")?;
            let existing = store
                .get_task(id)
                .map_err(store_error)?
                .ok_or_else(|| missing("task not found"))?;
            let args: Update = body(input.body)?;
            let task = Task {
                id,
                name: args.name,
                due: args.due.as_deref().map(date).transpose()?,
                recurrence: recurrence(args.recurrence)?,
                anchor: date(&args.anchor)?,
                collapse: args.collapse.unwrap_or(existing.collapse),
                advance: args.advance.unwrap_or(existing.advance),
                created_at: existing.created_at,
                archived: existing.archived,
            };
            store.update_task(&task).map_err(store_error)?;
            value(task)
        }
        "task_archive" => {
            let id = path_i64(&input, "task_id")?;
            store.archive_task(id).map_err(store_error)?;
            value(
                store
                    .get_task(id)
                    .map_err(store_error)?
                    .ok_or_else(|| missing("task not found"))?,
            )
        }
        "task_list" => {
            let due_before = query(&input, "due_before").map(date).transpose()?;
            let recurring = query(&input, "recurring")
                .map(|raw| {
                    raw.parse::<bool>()
                        .map_err(|_| bad("recurring must be true or false"))
                })
                .transpose()?;
            let tasks = store
                .list_tasks(due_before)
                .map_err(store_error)?
                .into_iter()
                .filter(|task| recurring.is_none_or(|wanted| task.recurrence.is_some() == wanted))
                .collect::<Vec<_>>();
            value(tasks)
        }
        "tasks_due" => {
            let as_of = date(query(&input, "as_of").ok_or_else(|| bad("missing as_of"))?)?;
            value(store.list_tasks(Some(as_of)).map_err(store_error)?)
        }
        "task_complete" => {
            let id = path_i64(&input, "task_id")?;
            let at = timestamp(&body::<Complete>(input.body)?.at)?;
            let task = store
                .get_task(id)
                .map_err(store_error)?
                .ok_or_else(|| missing("task not found"))?;
            let due = task.due.unwrap_or_else(|| at.date());
            value(store.record_occurrence(id, due, at).map_err(store_error)?)
        }
        "task_history" => value(
            store
                .occurrences_for_task(path_i64(&input, "task_id")?)
                .map_err(store_error)?,
        ),
        "task_streak" => {
            let id = path_i64(&input, "task_id")?;
            let as_of = date(query(&input, "as_of").ok_or_else(|| bad("missing as_of"))?)?;
            let task = store
                .get_task(id)
                .map_err(store_error)?
                .ok_or_else(|| missing("task not found"))?;
            let Some(rule) = task.recurrence.as_ref() else {
                return value(Streak {
                    current: 0,
                    best: 0,
                    unit: "none",
                });
            };
            let history = store.occurrences_for_task(id).map_err(store_error)?;
            let initial_due = history
                .first()
                .map(|occurrence| occurrence.due_date)
                .or(task.due)
                .ok_or_else(|| bad("recurring task lacks due date"))?;
            value(Streak {
                current: streak::current_with_settings(
                    &history,
                    rule,
                    initial_due,
                    task.anchor,
                    as_of,
                    task.collapse,
                    task.advance,
                ),
                best: streak::best_with_settings(
                    &history,
                    rule,
                    initial_due,
                    task.anchor,
                    task.collapse,
                    task.advance,
                ),
                unit: streak_unit(rule),
            })
        }
        _ => Err(bad(format!("unknown operation: {operation}"))),
    }
}
fn streak_unit(rule: &Recurrence) -> &'static str {
    match rule {
        Recurrence::Daily | Recurrence::EveryNDays { .. } | Recurrence::FromLast { .. } => "day",
        Recurrence::Weekly { .. } | Recurrence::EveryNWeeks { .. } => "week",
        Recurrence::Monthly { .. } | Recurrence::EveryNMonths { .. } => "month",
    }
}

pub async fn execute_operation_http(
    state: &AppState,
    operation: &str,
    input: generated::GeneratedOperationInput,
) -> axum::response::Response {
    match execute_operation(state, operation, input).await {
        Ok(output) => axum::Json(output).into_response(),
        Err(error) => (error.status, axum::Json(json!({"error": error.message}))).into_response(),
    }
}
pub fn http_router(state: AppState) -> Router {
    generated::generated_router().with_state(state)
}
const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8941";
fn http_bind_from(value: Option<String>) -> String {
    value.unwrap_or_else(|| DEFAULT_HTTP_BIND.to_owned())
}
fn configured_http_bind() -> String {
    http_bind_from(std::env::var("LITANY_HTTP_BIND").ok())
}

pub async fn run_cli() -> anyhow::Result<()> {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: generated_cli::GeneratedCommand,
    }
    let cli = Cli::parse();
    let command = cli.command;
    let operation = command.operation_name();
    let params = command.parameters_json();
    let locations: Value = serde_json::from_str(GENERATED_MCP_JSON)?;
    let locations = locations["locations"][operation]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("missing generated parameter locations for {operation}"))?;
    let mut path = BTreeMap::new();
    let mut query = BTreeMap::new();
    let mut body = serde_json::Map::new();
    for (key, location) in locations {
        let param = params.get(key).cloned().unwrap_or(Value::Null);
        match location.as_str() {
            Some("path") => {
                path.insert(key.clone(), param.as_str().unwrap_or_default().to_owned());
            }
            Some("query") => {
                if !param.is_null() {
                    query.insert(key.clone(), param.to_string().trim_matches('"').to_owned());
                }
            }
            Some("body") => {
                if !param.is_null() {
                    body.insert(key.clone(), param);
                }
            }
            _ => anyhow::bail!("invalid generated parameter location for {key}"),
        }
    }
    let input = generated::GeneratedOperationInput {
        path,
        query,
        body: Value::Object(body),
    };
    let db = std::env::var("LITANY_DB").unwrap_or_else(|_| "litany.db".into());
    println!(
        "{}",
        execute_operation(&AppState::open(db)?, operation, input).await?
    );
    Ok(())
}
pub async fn run_http() -> anyhow::Result<()> {
    let db = std::env::var("LITANY_DB").unwrap_or_else(|_| "litany.db".into());
    let bind = configured_http_bind();
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, http_router(AppState::open(db)?)).await?;
    Ok(())
}
pub async fn run_mcp() -> anyhow::Result<()> {
    let tools: Value = serde_json::from_str(GENERATED_MCP_JSON)?;
    let locations = tools["locations"].clone();
    let db = std::env::var("LITANY_DB").unwrap_or_else(|_| "litany.db".into());
    let state = AppState::open(db)?;
    hydra_mcp_stdio::serve(
        "litany",
        env!("CARGO_PKG_VERSION"),
        tools,
        move |name, args| {
            let state = state.clone();
            let locations = locations.clone();
            async move {
                let mut path = BTreeMap::new();
                let mut query = BTreeMap::new();
                let mut body = serde_json::Map::new();
                for (key, location) in locations[&name].as_object().cloned().unwrap_or_default() {
                    let value = args.get(&key).cloned().unwrap_or(Value::Null);
                    match location.as_str() {
                        Some("path") => {
                            path.insert(key, value.as_str().unwrap_or_default().to_owned());
                        }
                        Some("query") if value.is_null() => {}
                        Some("query") => {
                            query.insert(key, value.to_string().trim_matches('"').to_owned());
                        }
                        Some("body") => {
                            body.insert(key, value);
                        }
                        _ => return Err(format!("invalid generated parameter location for {key}")),
                    }
                }
                execute_operation(
                    &state,
                    &name,
                    generated::GeneratedOperationInput {
                        path,
                        query,
                        body: Value::Object(body),
                    },
                )
                .await
                .map_err(|error| error.message)
            }
        },
    )
    .await
    .map_err(|error| anyhow::anyhow!("mcp stdio error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_bind_defaults_to_loopback_and_accepts_container_override() {
        assert_eq!(http_bind_from(None), DEFAULT_HTTP_BIND);
        assert_eq!(
            http_bind_from(Some("0.0.0.0:8941".to_owned())),
            "0.0.0.0:8941"
        );
    }

    #[tokio::test]
    async fn create_complete_and_streak_share_the_generated_dispatch_contract() {
        let state = AppState::in_memory();
        let create=execute_operation(&state,"task_create",generated::GeneratedOperationInput{path:BTreeMap::new(),query:BTreeMap::new(),body:json!({"name":"walk","due":"2026-09-28","recurrence":{"kind":"daily"},"anchor":"2026-09-28","created_at":"2026-09-28T00:00:00"})}).await.unwrap();
        let id = create["id"].as_i64().unwrap().to_string();
        assert_eq!(create["collapse"], false);
        assert_eq!(create["advance"], "anchor");
        let mut path = BTreeMap::new();
        path.insert("task_id".into(), id.clone());
        execute_operation(
            &state,
            "task_complete",
            generated::GeneratedOperationInput {
                path: path.clone(),
                query: BTreeMap::new(),
                body: json!({"at":"2026-09-28T12:00:00"}),
            },
        )
        .await
        .unwrap();
        let mut query = BTreeMap::new();
        query.insert("as_of".into(), "2026-09-29".into());
        let streak = execute_operation(
            &state,
            "task_streak",
            generated::GeneratedOperationInput {
                path,
                query,
                body: Value::Null,
            },
        )
        .await
        .unwrap();
        assert_eq!(streak["current"], 1);
        assert_eq!(streak["best"], 1);
    }

    async fn http_json(
        state: &AppState,
        method: axum::http::Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        use axum::body::{to_bytes, Body};
        use axum::http::Request;
        use tower::ServiceExt;

        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .unwrap();
        let response = http_router(state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    #[tokio::test]
    async fn generated_http_round_trips_settings_and_preserves_them_on_replacement_update() {
        use axum::http::Method;

        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("litany.db");
        let state = AppState::open(&database).unwrap();
        let (status, created) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic walk",
                "due": "2026-11-01",
                "recurrence": {"kind": "daily"},
                "anchor": "2026-11-01",
                "collapse": true,
                "advance": "slide",
                "created_at": "2026-10-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let id = created["id"].as_i64().unwrap();
        assert_eq!(created["collapse"], true);
        assert_eq!(created["advance"], "slide");

        // Legacy clients send only the original replacement fields. The new
        // optional settings must survive unchanged rather than resetting.
        let (status, updated) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{id}/update"),
            Some(json!({
                "name": "renamed synthetic walk",
                "due": "2026-11-01",
                "recurrence": {"kind": "daily"},
                "anchor": "2026-11-01"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["collapse"], true);
        assert_eq!(updated["advance"], "slide");

        let (status, completed) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{id}/complete"),
            Some(json!({"at": "2026-11-05T23:59:12"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{completed}");
        assert_eq!(completed["due_date"], "2026-11-05");
        assert_eq!(completed["completed_at"], "2026-11-05T23:59:12");
        drop(state);

        // A fresh connection proves the schema migration and settings/history
        // survive SQLite close/reopen without accessing any live data.
        let reopened = AppState::open(&database).unwrap();
        let (status, tasks) = http_json(&reopened, Method::GET, "/tasks", None).await;
        assert_eq!(status, StatusCode::OK, "{tasks}");
        assert_eq!(tasks.as_array().unwrap().len(), 1);
        assert_eq!(tasks[0]["collapse"], true);
        assert_eq!(tasks[0]["advance"], "slide");
        assert_eq!(tasks[0]["due"], "2026-11-06");

        let (status, history) = http_json(
            &reopened,
            Method::GET,
            &format!("/tasks/{id}/history"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{history}");
        assert_eq!(history.as_array().unwrap().len(), 1);
        assert_eq!(history[0]["due_date"], "2026-11-05");
        assert_eq!(history[0]["completed_at"], "2026-11-05T23:59:12");
    }

    #[tokio::test]
    async fn generated_http_streaks_follow_slide_intervals_without_repairing_gaps() {
        use axum::http::Method;

        let state = AppState::in_memory();
        let (status, continuous) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic continuous slide",
                "due": "2026-01-31",
                "recurrence": {"kind": "monthly", "day": 31},
                "anchor": "2026-01-31",
                "collapse": true,
                "advance": "slide",
                "created_at": "2026-01-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{continuous}");
        let continuous_id = continuous["id"].as_i64().unwrap();
        for (at, expected_due) in [
            ("2026-01-31T12:00:00", "2026-01-31"),
            ("2026-02-28T12:00:00", "2026-02-28"),
            ("2026-03-28T12:00:00", "2026-03-28"),
        ] {
            let (status, occurrence) = http_json(
                &state,
                Method::POST,
                &format!("/tasks/{continuous_id}/complete"),
                Some(json!({"at": at})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{occurrence}");
            assert_eq!(occurrence["due_date"], expected_due);
        }
        let (status, streak) = http_json(
            &state,
            Method::GET,
            &format!("/tasks/{continuous_id}/streak?as_of=2026-04-29"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{streak}");
        assert_eq!(streak["current"], 3);
        assert_eq!(streak["best"], 3);

        let (status, gapped) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic gapped slide",
                "due": "2026-01-31",
                "recurrence": {"kind": "monthly", "day": 31},
                "anchor": "2026-01-31",
                "collapse": true,
                "advance": "slide",
                "created_at": "2026-01-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{gapped}");
        let gapped_id = gapped["id"].as_i64().unwrap();
        for at in ["2026-01-31T12:00:00", "2026-02-28T12:00:00"] {
            let (status, occurrence) = http_json(
                &state,
                Method::POST,
                &format!("/tasks/{gapped_id}/complete"),
                Some(json!({"at": at})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{occurrence}");
        }
        // March 28 is skipped. The April 30 collapse records only the latest
        // scheduled due (April 28); the missed March period remains a gap.
        let (status, collapsed) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{gapped_id}/complete"),
            Some(json!({"at": "2026-04-30T12:00:00"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{collapsed}");
        assert_eq!(collapsed["due_date"], "2026-04-28");
        let (status, streak) = http_json(
            &state,
            Method::GET,
            &format!("/tasks/{gapped_id}/streak?as_of=2026-04-30"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{streak}");
        assert_eq!(streak["current"], 1);
        assert_eq!(streak["best"], 2);
    }

    #[test]
    fn hydra_generated_cli_mcp_and_typescript_include_both_settings() {
        use clap::Parser;

        #[derive(clap::Parser)]
        struct Cli {
            #[command(subcommand)]
            command: generated_cli::GeneratedCommand,
        }
        let cli = Cli::try_parse_from([
            "litany",
            "task-create",
            "--name",
            "synthetic",
            "--due",
            "2026-11-01",
            "--recurrence",
            "{\"kind\":\"daily\"}",
            "--anchor",
            "2026-11-01",
            "--collapse",
            "true",
            "--advance",
            "slide",
            "--created-at",
            "2026-10-01T00:00:00",
        ])
        .unwrap();
        let params = cli.command.parameters_json();
        assert_eq!(params["collapse"], true);
        assert_eq!(params["advance"], "slide");

        let update = generated_cli::GeneratedCommand::TaskUpdate(generated_cli::TaskUpdateArgs {
            task_id: "1".into(),
            name: "synthetic".into(),
            due: Some("2026-11-01".into()),
            recurrence: Some(json!({"kind": "daily"})),
            anchor: "2026-11-01".into(),
            collapse: Some(false),
            advance: Some("anchor".into()),
        })
        .parameters_json();
        assert_eq!(update["collapse"], false);
        assert_eq!(update["advance"], "anchor");

        let mcp: Value = serde_json::from_str(GENERATED_MCP_JSON).unwrap();
        for operation in ["task_create", "task_update"] {
            let locations = &mcp["locations"][operation];
            assert_eq!(locations["collapse"], "body");
            assert_eq!(locations["advance"], "body");
            let tool = mcp["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == operation)
                .unwrap();
            let properties = &tool["inputSchema"]["properties"];
            assert_eq!(properties["collapse"]["type"], "boolean");
            assert_eq!(properties["advance"]["type"], "string");
            assert!(!tool["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "collapse" || field == "advance"));
        }

        let typescript = include_str!("../../../generated/ts-client/index.ts");
        assert!(typescript.contains("collapse?: boolean;"));
        assert!(typescript.contains("advance?: string;"));
        assert!(typescript.contains("\"collapse\": params.collapse"));
        assert!(typescript.contains("\"advance\": params.advance"));
    }

    #[tokio::test]
    async fn generated_mcp_locations_dispatch_configuration_through_shared_operation() {
        let state = AppState::in_memory();
        let mcp: Value = serde_json::from_str(GENERATED_MCP_JSON).unwrap();
        let args = json!({
            "name": "synthetic mcp task",
            "due": "2026-11-01",
            "recurrence": {"kind": "daily"},
            "anchor": "2026-11-01",
            "collapse": true,
            "advance": "slide",
            "created_at": "2026-10-01T00:00:00"
        });
        let mut body = serde_json::Map::new();
        for (key, location) in mcp["locations"]["task_create"].as_object().unwrap() {
            if location == "body" {
                if let Some(value) = args.get(key) {
                    body.insert(key.clone(), value.clone());
                }
            }
        }
        let created = execute_operation(
            &state,
            "task_create",
            generated::GeneratedOperationInput {
                path: BTreeMap::new(),
                query: BTreeMap::new(),
                body: Value::Object(body),
            },
        )
        .await
        .unwrap();
        assert_eq!(created["collapse"], true);
        assert_eq!(created["advance"], "slide");

        let update_args = json!({
            "task_id": created["id"].as_i64().unwrap().to_string(),
            "name": "updated synthetic mcp task",
            "due": "2026-11-01",
            "recurrence": {"kind": "daily"},
            "anchor": "2026-11-01"
        });
        let mut path = BTreeMap::new();
        let mut update_body = serde_json::Map::new();
        for (key, location) in mcp["locations"]["task_update"].as_object().unwrap() {
            let value = update_args.get(key).cloned().unwrap_or(Value::Null);
            match location.as_str().unwrap() {
                "path" => {
                    path.insert(key.clone(), value.as_str().unwrap_or_default().to_owned());
                }
                "body" => {
                    update_body.insert(key.clone(), value);
                }
                other => panic!("unexpected generated MCP parameter location: {other}"),
            }
        }
        let updated = execute_operation(
            &state,
            "task_update",
            generated::GeneratedOperationInput {
                path,
                query: BTreeMap::new(),
                body: Value::Object(update_body),
            },
        )
        .await
        .unwrap();
        assert_eq!(updated["collapse"], true);
        assert_eq!(updated["advance"], "slide");
    }
}
