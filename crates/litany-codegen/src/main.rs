//! Regenerate or verify Litany's four committed Hydra surface artifacts.
//!
//! Run from the repository root: `cargo run --locked -p litany-codegen -- check`.

use anyhow::{Context, Result};
use hydra_codegen::{
    generate_all, validate_generation, verify_generated, write_generated, GenerateConfig,
};
use std::path::Path;

const DEFINITION: &str = "api/operations.yaml";
const CONFIG: &str = "hydra.yaml";
const GENERATED: &str = "generated";

fn main() -> Result<()> {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "check".to_string());
    let config = load_config()?;

    match command.as_str() {
        "write" => {
            let definition = load_definition()?;
            validate_generation(&definition, &config)
                .context("validate definition/configuration for generation")?;
            write_generated(GENERATED, &generate_all(&definition, &config))?;
            println!("wrote generated/cli.rs, http.rs, mcp.json, ts-client/index.ts");
        }
        "check" => {
            // verify_generated reloads the definition, validates collision rules, and compares
            // every committed artifact without modifying it.
            verify_generated(DEFINITION, GENERATED, &config)
                .context("generated artifacts are stale")?;
            println!("generated artifacts are current");
        }
        other => anyhow::bail!("unknown command {other:?}; expected `check` or `write`"),
    }
    Ok(())
}

fn load_definition() -> Result<hydra_core::ApiDefinition> {
    hydra_core::load_api_definition(DEFINITION).context("load api/operations.yaml")
}

fn load_config() -> Result<GenerateConfig> {
    let path = Path::new(CONFIG);
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_yaml::Value =
        serde_yaml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    reject_unknown_config_keys(&value)?;
    let config: GenerateConfig =
        serde_yaml::from_value(value).with_context(|| format!("parse {}", path.display()))?;
    config
        .validate()
        .with_context(|| format!("validate {}", path.display()))?;
    Ok(config)
}

fn reject_unknown_config_keys(value: &serde_yaml::Value) -> Result<()> {
    let mapping = value.as_mapping().context("hydra.yaml must be a mapping")?;
    for key in mapping.keys() {
        let key = key.as_str().context("hydra.yaml keys must be strings")?;
        if !matches!(
            key,
            "http_dispatch_fn"
                | "http_state_type"
                | "sse_binding_prefix"
                | "http_raw_dispatch_fn"
                | "generator_name"
                | "ts_client_name"
        ) {
            anyhow::bail!("unknown hydra.yaml key {key:?}");
        }
    }
    Ok(())
}
