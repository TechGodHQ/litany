use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::{http::StatusCode, response::IntoResponse, Router};
use chrono::{NaiveDate, NaiveDateTime};
use litany_core::{streak, Recurrence, Task};
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
    created_at: String,
}
#[derive(Deserialize)]
struct Update {
    name: String,
    due: Option<String>,
    recurrence: Option<Value>,
    anchor: String,
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
                current: streak::current(&history, rule, initial_due, task.anchor, as_of),
                best: streak::best(&history, rule, initial_due, task.anchor),
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
}
