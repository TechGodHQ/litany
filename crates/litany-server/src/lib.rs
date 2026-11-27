use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::{http::StatusCode, response::IntoResponse, Router};
use chrono::{NaiveDate, NaiveDateTime};
use litany_core::{streak, AdvanceMode, Recurrence, Task};
use litany_store::{ArchivedFilter, SatisfactionFilter, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod generated {
    include!("../../../generated/http.rs");
}
pub mod generated_cli {
    include!("../../../generated/cli.rs");
}
pub const GENERATED_MCP_JSON: &str = include_str!("../../../generated/mcp.json");
pub const GENERATED_TS_CLIENT: &str = include_str!("../../../generated/ts-client/index.ts");

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
fn query_parameter(value: &Value) -> Option<String> {
    if value.is_null() {
        None
    } else if let Some(string) = value.as_str() {
        Some(string.to_owned())
    } else {
        Some(value.to_string())
    }
}
fn reject_unknown_query(
    input: &generated::GeneratedOperationInput,
    allowed: &[&str],
) -> Result<(), OperationError> {
    if let Some(key) = input
        .query
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(bad(format!("unknown query parameter: {key}")));
    }
    Ok(())
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
        "task_get" => {
            let id = path_i64(&input, "task_id")?;
            value(
                store
                    .get_task(id)
                    .map_err(store_error)?
                    .ok_or_else(|| missing("task not found"))?,
            )
        }
        "task_unarchive" => {
            let id = path_i64(&input, "task_id")?;
            value(
                store
                    .unarchive_task(id)
                    .map_err(store_error)?
                    .ok_or_else(|| missing("task not found"))?,
            )
        }
        "task_list" => {
            reject_unknown_query(
                &input,
                &["due_before", "recurring", "satisfied", "archived", "q"],
            )?;
            let due_before = query(&input, "due_before").map(date).transpose()?;
            let recurring = query(&input, "recurring")
                .map(|raw| {
                    raw.parse::<bool>()
                        .map_err(|_| bad("recurring must be true or false"))
                })
                .transpose()?;
            let satisfaction = match query(&input, "satisfied").unwrap_or("all") {
                "true" => SatisfactionFilter::Satisfied,
                "false" => SatisfactionFilter::Unsatisfied,
                "all" => SatisfactionFilter::All,
                _ => return Err(bad("satisfied must be true, false, or all")),
            };
            let archived = match query(&input, "archived").unwrap_or("false") {
                "false" => ArchivedFilter::Active,
                "true" => ArchivedFilter::Archived,
                "all" => ArchivedFilter::All,
                _ => return Err(bad("archived must be false, true, or all")),
            };
            let name_query = query(&input, "q").filter(|query| !query.is_empty());
            let tasks = store
                .list_tasks_with_filters(due_before, satisfaction, archived, name_query)
                .map_err(store_error)?
                .into_iter()
                .filter(|task| recurring.is_none_or(|wanted| task.recurrence.is_some() == wanted))
                .collect::<Vec<_>>();
            value(tasks)
        }
        "tasks_due" => {
            reject_unknown_query(&input, &["as_of"])?;
            let as_of = date(query(&input, "as_of").ok_or_else(|| bad("missing as_of"))?)?;
            value(
                store
                    .list_tasks_with_satisfaction(Some(as_of), SatisfactionFilter::Unsatisfied)
                    .map_err(store_error)?,
            )
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
                if let Some(value) = query_parameter(&param) {
                    query.insert(key.clone(), value);
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
                        Some("query") => {
                            if let Some(value) = query_parameter(&value) {
                                query.insert(key, value);
                            }
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
    fn query_parameters_preserve_decoded_strings_and_serialize_scalars() {
        assert_eq!(
            query_parameter(&json!("C:\\Inbox \"Today\"\nLater")),
            Some("C:\\Inbox \"Today\"\nLater".to_owned())
        );
        assert_eq!(query_parameter(&json!(false)), Some("false".to_owned()));
        assert_eq!(query_parameter(&json!(12)), Some("12".to_owned()));
        assert_eq!(query_parameter(&Value::Null), None);
    }

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
    async fn generated_http_satisfaction_filters_preserve_due_history_and_streak_semantics() {
        use axum::http::Method;

        let state = AppState::in_memory();
        let (status, completed_one_off) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic satisfied one-off",
                "due": "2026-11-01",
                "anchor": "2026-11-01",
                "created_at": "2026-10-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{completed_one_off}");
        let completed_id = completed_one_off["id"].as_i64().unwrap();

        let (status, due_before_completion) =
            http_json(&state, Method::GET, "/tasks/due?as_of=2026-11-01", None).await;
        assert_eq!(status, StatusCode::OK, "{due_before_completion}");
        assert!(due_before_completion
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == completed_id));

        let (status, completion) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{completed_id}/complete"),
            Some(json!({"at": "2026-11-01T12:00:00"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{completion}");

        let (status, due_after_completion) =
            http_json(&state, Method::GET, "/tasks/due?as_of=2026-11-01", None).await;
        assert_eq!(status, StatusCode::OK, "{due_after_completion}");
        assert!(!due_after_completion
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == completed_id));

        let (status, unsatisfied_one_off) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic unsatisfied one-off",
                "due": "2026-11-02",
                "anchor": "2026-11-02",
                "created_at": "2026-10-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{unsatisfied_one_off}");
        let unsatisfied_id = unsatisfied_one_off["id"].as_i64().unwrap();

        let (status, future_one_off) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic future one-off",
                "due": "2026-11-03",
                "anchor": "2026-11-03",
                "created_at": "2026-10-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{future_one_off}");
        let future_id = future_one_off["id"].as_i64().unwrap();
        let (status, future_completion) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{future_id}/complete"),
            Some(json!({"at": "2026-11-01T13:00:00"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{future_completion}");

        let (status, recurring) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic recurring",
                "due": "2026-11-01",
                "recurrence": {"kind": "daily"},
                "anchor": "2026-11-01",
                "created_at": "2026-10-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{recurring}");
        let recurring_id = recurring["id"].as_i64().unwrap();
        let (status, recurring_completion) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{recurring_id}/complete"),
            Some(json!({"at": "2026-11-01T14:00:00"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{recurring_completion}");
        assert_eq!(recurring_completion["due_date"], "2026-11-01");

        let (status, null_due) = http_json(
            &state,
            Method::POST,
            "/tasks",
            Some(json!({
                "name": "synthetic no-due task",
                "anchor": "2026-11-01",
                "created_at": "2026-10-01T00:00:00"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{null_due}");
        let null_due_id = null_due["id"].as_i64().unwrap();

        let (status, due_as_of_november_2) =
            http_json(&state, Method::GET, "/tasks/due?as_of=2026-11-02", None).await;
        assert_eq!(status, StatusCode::OK, "{due_as_of_november_2}");
        let due_ids = due_as_of_november_2
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert!(!due_ids.contains(&completed_id));
        assert!(
            !due_ids.contains(&future_id),
            "future dues stay outside as_of"
        );
        assert!(due_ids.contains(&unsatisfied_id));
        assert!(due_ids.contains(&recurring_id));
        assert!(!due_ids.contains(&null_due_id));

        let (status, due_as_of_future_due) =
            http_json(&state, Method::GET, "/tasks/due?as_of=2026-11-03", None).await;
        assert_eq!(status, StatusCode::OK, "{due_as_of_future_due}");
        assert!(!due_as_of_future_due
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["id"] == future_id));

        let (status, all_default) = http_json(&state, Method::GET, "/tasks", None).await;
        assert_eq!(status, StatusCode::OK, "{all_default}");
        let (status, all_explicit) =
            http_json(&state, Method::GET, "/tasks?satisfied=all", None).await;
        assert_eq!(status, StatusCode::OK, "{all_explicit}");
        assert_eq!(all_explicit, all_default);

        let (status, satisfied) =
            http_json(&state, Method::GET, "/tasks?satisfied=true", None).await;
        assert_eq!(status, StatusCode::OK, "{satisfied}");
        let satisfied_ids = satisfied
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert!(satisfied_ids.contains(&completed_id));
        assert!(satisfied_ids.contains(&future_id));
        assert!(!satisfied_ids.contains(&recurring_id));
        assert!(!satisfied_ids.contains(&null_due_id));

        let (status, unsatisfied) =
            http_json(&state, Method::GET, "/tasks?satisfied=false", None).await;
        assert_eq!(status, StatusCode::OK, "{unsatisfied}");
        let unsatisfied_ids = unsatisfied
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert!(!unsatisfied_ids.contains(&completed_id));
        assert!(!unsatisfied_ids.contains(&future_id));
        assert!(unsatisfied_ids.contains(&unsatisfied_id));
        assert!(unsatisfied_ids.contains(&recurring_id));
        assert!(unsatisfied_ids.contains(&null_due_id));

        let (status, composed) = http_json(
            &state,
            Method::GET,
            "/tasks?due_before=2026-11-02&recurring=true&satisfied=false",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{composed}");
        assert_eq!(
            composed
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![recurring_id]
        );

        let (status, invalid) = http_json(&state, Method::GET, "/tasks?satisfied=yes", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(invalid["error"], "satisfied must be true, false, or all");

        let (status, history_before_filters) = http_json(
            &state,
            Method::GET,
            &format!("/tasks/{recurring_id}/history"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{history_before_filters}");
        let (status, streak_before_filters) = http_json(
            &state,
            Method::GET,
            &format!("/tasks/{recurring_id}/streak?as_of=2026-11-02"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{streak_before_filters}");

        for path in [
            "/tasks",
            "/tasks?satisfied=true",
            "/tasks?satisfied=false",
            "/tasks?satisfied=all",
            "/tasks/due?as_of=2026-11-02",
        ] {
            let (status, _) = http_json(&state, Method::GET, path, None).await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }

        let (status, history_after_filters) = http_json(
            &state,
            Method::GET,
            &format!("/tasks/{recurring_id}/history"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{history_after_filters}");
        assert_eq!(history_after_filters, history_before_filters);
        let (status, streak_after_filters) = http_json(
            &state,
            Method::GET,
            &format!("/tasks/{recurring_id}/streak?as_of=2026-11-02"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{streak_after_filters}");
        assert_eq!(streak_after_filters, streak_before_filters);
    }

    #[test]
    fn generated_archive_recovery_contract_is_registered_on_all_surfaces() {
        let expected_routes = [
            ("task_get", "GET", "/tasks/{task_id}"),
            ("task_unarchive", "POST", "/tasks/{task_id}/unarchive"),
        ];
        for (name, method, path) in expected_routes {
            let route = generated::GENERATED_ROUTES
                .iter()
                .find(|route| route.name == name)
                .unwrap();
            assert_eq!((route.method, route.path), (method, path));
        }

        let get = generated_cli::GeneratedCommand::TaskGet(generated_cli::TaskGetArgs {
            task_id: "17".into(),
        });
        assert_eq!(get.operation_name(), "task_get");
        assert_eq!(get.parameters_json()["task_id"], "17");
        let unarchive =
            generated_cli::GeneratedCommand::TaskUnarchive(generated_cli::TaskUnarchiveArgs {
                task_id: "17".into(),
            });
        assert_eq!(unarchive.operation_name(), "task_unarchive");
        assert_eq!(unarchive.parameters_json()["task_id"], "17");
        let list = generated_cli::GeneratedCommand::TaskList(generated_cli::TaskListArgs {
            due_before: None,
            recurring: None,
            satisfied: None,
            archived: Some("all".into()),
            q: Some("literal_%".into()),
        });
        assert_eq!(list.parameters_json()["archived"], "all");
        assert_eq!(list.parameters_json()["q"], "literal_%");

        let manifest: Value = serde_json::from_str(GENERATED_MCP_JSON).unwrap();
        assert_eq!(manifest["locations"]["task_get"]["task_id"], "path");
        assert_eq!(manifest["locations"]["task_unarchive"]["task_id"], "path");
        assert_eq!(manifest["locations"]["task_list"]["archived"], "query");
        assert_eq!(manifest["locations"]["task_list"]["q"], "query");
        let tools = manifest["tools"].as_array().unwrap();
        let tool = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool("task_get")["inputSchema"]["required"][0], "task_id");
        assert_eq!(
            tool("task_unarchive")["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .len(),
            1,
            "unarchive takes only the stable task ID and no body"
        );
        assert_eq!(
            tool("task_list")["inputSchema"]["properties"]["archived"]["type"],
            "string"
        );
        assert_eq!(
            tool("task_list")["inputSchema"]["properties"]["q"]["type"],
            "string"
        );

        assert!(GENERATED_TS_CLIENT.contains("export interface TaskGetParams"));
        assert!(GENERATED_TS_CLIENT.contains("async taskGet(params: TaskGetParams)"));
        assert!(GENERATED_TS_CLIENT.contains("export interface TaskUnarchiveParams"));
        assert!(GENERATED_TS_CLIENT.contains("async taskUnarchive(params: TaskUnarchiveParams)"));
        assert!(GENERATED_TS_CLIENT.contains("archived?: string;"));
        assert!(GENERATED_TS_CLIENT.contains("q?: string;"));
    }

    #[tokio::test]
    async fn generated_http_archive_recovery_search_and_unarchive_are_exact() {
        use axum::http::Method;
        use std::collections::BTreeSet;

        async fn create_task(
            state: &AppState,
            name: &str,
            due: Option<&str>,
            recurrence: Option<Value>,
            collapse: bool,
            advance: &str,
        ) -> i64 {
            let mut body = json!({
                "name": name,
                "anchor": "2026-11-01",
                "collapse": collapse,
                "advance": advance,
                "created_at": "2026-10-01T00:00:00"
            });
            if let Some(due) = due {
                body["due"] = json!(due);
            }
            if let Some(recurrence) = recurrence {
                body["recurrence"] = recurrence;
            }
            let (status, created) = http_json(state, Method::POST, "/tasks", Some(body)).await;
            assert_eq!(status, StatusCode::OK, "{created}");
            created["id"].as_i64().unwrap()
        }

        let state = AppState::in_memory();
        let dated = create_task(
            &state,
            "Routine 100%_Done",
            Some("2026-11-01"),
            None,
            false,
            "anchor",
        )
        .await;
        let no_due = create_task(&state, "MixedCase no due", None, None, false, "slide").await;
        let mut recurring_ids = Vec::new();
        for (collapse, advance) in [
            (false, "anchor"),
            (false, "slide"),
            (true, "anchor"),
            (true, "slide"),
        ] {
            let name = format!("MixedCase recurring collapse={collapse} advance={advance}");
            let id = create_task(
                &state,
                &name,
                Some("2026-11-01"),
                Some(json!({"kind": "daily"})),
                collapse,
                advance,
            )
            .await;
            recurring_ids.push(id);
        }
        let active_decoy = create_task(
            &state,
            "Routine 100xyDone active",
            Some("2026-11-03"),
            None,
            false,
            "anchor",
        )
        .await;
        let active_later = create_task(
            &state,
            "active later",
            Some("2026-11-04"),
            None,
            false,
            "anchor",
        )
        .await;

        let mut archived_ids = vec![dated, no_due];
        archived_ids.extend(recurring_ids.iter().copied());
        for id in &archived_ids {
            let (status, completion) = http_json(
                &state,
                Method::POST,
                &format!("/tasks/{id}/complete"),
                Some(json!({"at": "2026-11-01T12:00:00"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{completion}");
        }

        let mut archived_records = Vec::new();
        for id in &archived_ids {
            let (history_status, history) =
                http_json(&state, Method::GET, &format!("/tasks/{id}/history"), None).await;
            assert_eq!(history_status, StatusCode::OK, "{history}");
            let (status, task) =
                http_json(&state, Method::POST, &format!("/tasks/{id}/archive"), None).await;
            assert_eq!(status, StatusCode::OK, "{task}");
            assert_eq!(task["archived"], true);
            archived_records.push((*id, task, history));
        }

        let (status, active_default) = http_json(&state, Method::GET, "/tasks", None).await;
        assert_eq!(status, StatusCode::OK, "{active_default}");
        let (status, active_false) =
            http_json(&state, Method::GET, "/tasks?archived=false", None).await;
        assert_eq!(status, StatusCode::OK, "{active_false}");
        assert_eq!(
            active_false, active_default,
            "omitted archived preserves the old set/order"
        );
        assert_eq!(
            active_default
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![active_decoy, active_later],
            "existing due-date ordering remains unchanged"
        );

        let (status, archived_only) =
            http_json(&state, Method::GET, "/tasks?archived=true", None).await;
        assert_eq!(status, StatusCode::OK, "{archived_only}");
        let archived_set = archived_only
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_i64().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(archived_set, archived_ids.iter().copied().collect());
        let (status, all_tasks) = http_json(&state, Method::GET, "/tasks?archived=all", None).await;
        assert_eq!(status, StatusCode::OK, "{all_tasks}");
        let all_ids = all_tasks
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_i64().unwrap())
            .collect::<Vec<_>>();
        let all_set = all_ids.iter().copied().collect::<BTreeSet<_>>();
        let expected_union = archived_ids
            .iter()
            .copied()
            .chain([active_decoy, active_later])
            .collect::<BTreeSet<_>>();
        assert_eq!(all_ids.len(), all_set.len(), "all must not duplicate tasks");
        assert_eq!(
            all_set, expected_union,
            "all is the disjoint union of archived and active"
        );
        let (status, empty_query) =
            http_json(&state, Method::GET, "/tasks?archived=true&q=", None).await;
        assert_eq!(status, StatusCode::OK, "{empty_query}");
        assert_eq!(empty_query, archived_only, "empty q is omitted");

        for (path, expected) in [
            ("/tasks?archived=true&q=RoUtInE", vec![dated]),
            ("/tasks?archived=true&q=%25_", vec![dated]),
            ("/tasks?archived=true&q=_", vec![dated]),
        ] {
            let (status, found) = http_json(&state, Method::GET, path, None).await;
            assert_eq!(status, StatusCode::OK, "{path}: {found}");
            assert_eq!(
                found
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|task| task["id"].as_i64().unwrap())
                    .collect::<Vec<_>>(),
                expected,
                "name search is case-insensitive and treats %/_ literally: {path}"
            );
        }
        let (status, mixed_case) =
            http_json(&state, Method::GET, "/tasks?archived=true&q=mIxEd", None).await;
        assert_eq!(status, StatusCode::OK, "{mixed_case}");
        assert_eq!(mixed_case.as_array().unwrap().len(), 5);
        let (status, no_match) = http_json(
            &state,
            Method::GET,
            "/tasks?archived=true&q=unmatched",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{no_match}");
        assert_eq!(no_match, json!([]));

        let (status, composed_one_off) = http_json(
            &state,
            Method::GET,
            "/tasks?archived=true&q=routine&due_before=2026-11-02&recurring=false&satisfied=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{composed_one_off}");
        assert_eq!(
            composed_one_off
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![dated]
        );
        let (status, composed_recurring) = http_json(
            &state,
            Method::GET,
            "/tasks?archived=true&q=RECURRING&due_before=2026-11-02&recurring=true&satisfied=false",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{composed_recurring}");
        assert_eq!(
            composed_recurring
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<BTreeSet<_>>(),
            recurring_ids.iter().copied().collect()
        );

        for (path, expected_message) in [
            ("/tasks?bogus=1", "unknown query parameter: bogus"),
            (
                "/tasks?archived=maybe",
                "archived must be false, true, or all",
            ),
        ] {
            let (status, error) = http_json(&state, Method::GET, path, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {error}");
            assert_eq!(error["error"], expected_message);
        }
        let (status, due_unknown) = http_json(
            &state,
            Method::GET,
            "/tasks/due?as_of=2026-11-03&bogus=1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{due_unknown}");
        assert_eq!(due_unknown["error"], "unknown query parameter: bogus");
        let (status, due_valid) =
            http_json(&state, Method::GET, "/tasks/due?as_of=2026-11-03", None).await;
        assert_eq!(status, StatusCode::OK, "{due_valid}");
        assert_eq!(
            due_valid
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![active_decoy],
            "due remains active-only"
        );

        for (id, archived_task, history_before) in &archived_records {
            let (status, found) =
                http_json(&state, Method::GET, &format!("/tasks/{id}"), None).await;
            assert_eq!(status, StatusCode::OK, "{found}");
            assert_eq!(found, *archived_task);

            let mut expected = archived_task.clone();
            expected["archived"] = json!(false);
            let (first_status, first) = http_json(
                &state,
                Method::POST,
                &format!("/tasks/{id}/unarchive"),
                None,
            )
            .await;
            assert_eq!(first_status, StatusCode::OK, "{first}");
            assert_eq!(first, expected, "only archived changes");
            let (second_status, second) = http_json(
                &state,
                Method::POST,
                &format!("/tasks/{id}/unarchive"),
                None,
            )
            .await;
            assert_eq!(second_status, StatusCode::OK, "{second}");
            assert_eq!(second, first, "repeated unarchive is idempotent");
            let (history_status, history_after) =
                http_json(&state, Method::GET, &format!("/tasks/{id}/history"), None).await;
            assert_eq!(history_status, StatusCode::OK, "{history_after}");
            assert_eq!(
                history_after, *history_before,
                "occurrence rows are unchanged"
            );
        }

        let (status, active_before) =
            http_json(&state, Method::GET, &format!("/tasks/{active_later}"), None).await;
        assert_eq!(status, StatusCode::OK, "{active_before}");
        assert_eq!(active_before["archived"], false);
        let (status, active_once) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{active_later}/unarchive"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{active_once}");
        assert_eq!(active_once, active_before);
        let (status, active_twice) = http_json(
            &state,
            Method::POST,
            &format!("/tasks/{active_later}/unarchive"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{active_twice}");
        assert_eq!(active_twice, active_before);

        let (status, missing_get) =
            http_json(&state, Method::GET, "/tasks/9223372036854775807", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing_get}");
        let (status, missing_unarchive) = http_json(
            &state,
            Method::POST,
            "/tasks/9223372036854775807/unarchive",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing_unarchive}");
        let (status, no_archived_left) =
            http_json(&state, Method::GET, "/tasks?archived=true", None).await;
        assert_eq!(status, StatusCode::OK, "{no_archived_left}");
        assert_eq!(no_archived_left, json!([]));
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
    fn hydra_generated_cli_mcp_and_typescript_include_settings_and_satisfaction_filter() {
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

        let cli_list = Cli::try_parse_from(["litany", "task-list", "--satisfied", "true"]).unwrap();
        assert_eq!(cli_list.command.operation_name(), "task_list");
        let list_params = cli_list.command.parameters_json();
        assert_eq!(list_params["satisfied"], "true");

        let list_locations = &mcp["locations"]["task_list"];
        assert_eq!(list_locations["satisfied"], "query");
        let list_tool = mcp["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "task_list")
            .unwrap();
        assert_eq!(
            list_tool["inputSchema"]["properties"]["satisfied"]["type"],
            "string"
        );
        assert!(!list_tool["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "satisfied"));

        let typescript = include_str!("../../../generated/ts-client/index.ts");
        assert!(typescript.contains("collapse?: boolean;"));
        assert!(typescript.contains("advance?: string;"));
        assert!(typescript.contains("\"collapse\": params.collapse"));
        assert!(typescript.contains("\"advance\": params.advance"));
        assert!(typescript.contains("satisfied?: string;"));
        assert!(
            typescript.contains("url.searchParams.set(\"satisfied\", String(params.satisfied));")
        );
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

    #[tokio::test]
    async fn generated_cli_and_mcp_satisfaction_arguments_reach_shared_dispatch() {
        use clap::Parser;

        #[derive(clap::Parser)]
        struct Cli {
            #[command(subcommand)]
            command: generated_cli::GeneratedCommand,
        }

        fn query_from_generated_args(args: &Value, locations: &Value) -> BTreeMap<String, String> {
            locations
                .as_object()
                .unwrap()
                .iter()
                .filter_map(|(key, location)| {
                    if location != "query" {
                        return None;
                    }
                    let value = args.get(key)?;
                    if value.is_null() {
                        return None;
                    }
                    let value = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string().trim_matches('"').to_owned());
                    Some((key.clone(), value))
                })
                .collect()
        }

        let state = AppState::in_memory();
        let completed = execute_operation(
            &state,
            "task_create",
            generated::GeneratedOperationInput {
                path: BTreeMap::new(),
                query: BTreeMap::new(),
                body: json!({
                    "name": "synthetic cli-mcp satisfied",
                    "due": "2026-11-01",
                    "anchor": "2026-11-01",
                    "created_at": "2026-10-01T00:00:00"
                }),
            },
        )
        .await
        .unwrap();
        let completed_id = completed["id"].as_i64().unwrap();
        let mut path = BTreeMap::new();
        path.insert("task_id".into(), completed_id.to_string());
        execute_operation(
            &state,
            "task_complete",
            generated::GeneratedOperationInput {
                path,
                query: BTreeMap::new(),
                body: json!({"at": "2026-11-01T12:00:00"}),
            },
        )
        .await
        .unwrap();

        let unsatisfied = execute_operation(
            &state,
            "task_create",
            generated::GeneratedOperationInput {
                path: BTreeMap::new(),
                query: BTreeMap::new(),
                body: json!({
                    "name": "synthetic cli-mcp unsatisfied",
                    "due": "2026-11-02",
                    "anchor": "2026-11-02",
                    "created_at": "2026-10-01T00:00:00"
                }),
            },
        )
        .await
        .unwrap();
        let unsatisfied_id = unsatisfied["id"].as_i64().unwrap();

        let mcp: Value = serde_json::from_str(GENERATED_MCP_JSON).unwrap();
        let cli = Cli::try_parse_from(["litany", "task-list", "--satisfied", "true"]).unwrap();
        let cli_args = cli.command.parameters_json();
        let cli_query = query_from_generated_args(&cli_args, &mcp["locations"]["task_list"]);
        let cli_result = execute_operation(
            &state,
            "task_list",
            generated::GeneratedOperationInput {
                path: BTreeMap::new(),
                query: cli_query,
                body: Value::Null,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            cli_result
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![completed_id]
        );

        let mcp_args = json!({"satisfied": "false"});
        let mcp_query = query_from_generated_args(&mcp_args, &mcp["locations"]["task_list"]);
        let mcp_result = execute_operation(
            &state,
            "task_list",
            generated::GeneratedOperationInput {
                path: BTreeMap::new(),
                query: mcp_query,
                body: Value::Null,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            mcp_result
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![unsatisfied_id]
        );
    }
}
