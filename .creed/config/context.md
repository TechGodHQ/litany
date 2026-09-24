# Litany — repo doctrine

Litany is a recurring task engine in the TechGodHQ fleet. This directory
copies the creed pattern (see TechGodHQ/creed): context that AI agents and
humans both consume.

## Design laws

1. **Pure core.** `litany-core` contains domain logic only. No HTTP, no
   Telegram, no provider code, no clock reads (time enters as arguments —
   the caller supplies "now" so the engine is deterministic and testable).
2. **Occurrence-sourced truth.** Every completion is an `occurrences` row
   with its real timestamp. Streaks, history, and compliance stats are
   derived queries, never stored counters.
3. **Hydra-only surfaces.** CLI/HTTP/MCP surfaces are generated from
   `api/operations.yaml`. No hand-rolled routes. One definition per
   operation; every surface derives from it.
4. **LLM-native, never LLM-required.** The engine works with plain data;
   LLM consumers get the same surfaces humans do.

## Conventions

- Rust, workspace at repo root, core in `crates/litany-core`.
- SQLite (`rusqlite`, bundled) at `/data/litany.db` in deployment.
- Dates are UTC `NaiveDate`; timestamps UTC `NaiveDateTime`. Local-time
  rendering is a consumer concern, never the engine's.
- License: MIT. Repo public.
