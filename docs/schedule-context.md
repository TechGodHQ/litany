# Persisted Schedule Context and Streak History

**Scope:** this is the COD-547, docs-only contract. It records the verified owner ruling and the current storage/derivation boundary so later implementation can be split into reviewed slices. It does not change runtime behavior, schema, generated surfaces, data, deployment, or release policy.

**Decision source:** Plane COD-547 comment `a692b417-0e63-58bf-80e0-90f2d8f4c7e5` (Shiv, 2026-10-13), applied in same-issue comment `4d57a43a-80aa-5d3a-a9b0-2a4cbfbf51ed`. The ruling is prospective by default; retroactive interpretation requires explicit opt-in. It does not authorize runtime implementation or a migration in this ticket.

## Current implementation boundary

The repository's current model is not yet this contract:

- `crates/litany-core/src/model.rs`: `Task.due` is the task's current/next due date; `Task.recurrence` and `Task.anchor` describe its current schedule. `Task` has no persisted `initial_due` or schedule-version history. `Occurrence.due_date` identifies the recurrence period satisfied; `Occurrence.completed_at` is the real UTC completion timestamp.
- `crates/litany-store/src/schema.sql`: `tasks` stores the current due date, recurrence, and anchor; `occurrences` stores `due_date` and `completed_at`. There is no schedule-segment table or audit trail for edits.
- `crates/litany-store/src/lib.rs`: `update_task` replaces editable task fields while retaining occurrence rows. `record_occurrence` records the scheduled due date and actual completion time, then advances a recurring task from that due date in the same transaction. Reopening the database reloads those persisted rows; it does not recover schedule edits that were never recorded.
- `crates/litany-server/src/lib.rs`: `task_streak` currently chooses the first occurrence due date as `initial_due`, falling back to the task's current due date, then evaluates all retained history using the task's current rule and anchor. This is a current implementation shortcut, not proof that an old initial due date or schedule can be reconstructed after edits.
- `crates/litany-core/src/streak.rs` uses explicit `initial_due`, `anchor`, and caller-supplied `as_of`. Period identity comes from occurrence `due_date`, not `completed_at`. Missed periods have no synthetic occurrence and break strict adjacency. Existing current-streak semantics include one open-period grace: a completed run remains current through the immediately following open period and expires when the next period begins without completion. `best` is the longest strictly consecutive run.
- `crates/litany-core/src/recurrence.rs` defines recurrence boundaries. For example, a monthly day of 31 clamps to the destination month's last day; a `None` monthly day uses `anchor.day()`.

The current fallback from history/current `due` must not be described as recovered historical truth. No code or persisted records are changed by this document.

## Approved contract

### Schedule identity and lifecycle

For future implementation, a recurring schedule segment must retain the values needed to derive its periods independently of mutable task fields:

- `initial_due`: the first due date in that segment. It is not interchangeable with either `Task.due` (the next outstanding due date) or `created_at`.
- `anchor`: the explicit UTC date used by recurrence variants whose grid depends on an anchor. It may differ from `initial_due`.
- recurrence rule and the segment's effective boundary, so an edit does not silently make old history appear to have used the new rule.

Creating a recurring task starts a segment from the explicitly supplied first due date, anchor, and recurrence. Completing a due task appends one occurrence with the scheduled period in `due_date` and the actual instant in `completed_at`; the current due advances atomically, while the segment's initial due and anchor remain unchanged. A missed period creates no occurrence and is not repaired by inventing one. A name-only edit does not create a schedule segment.

Changing due date, recurrence, or anchor after history exists starts a new schedule segment at an explicit boundary. The old occurrence rows and their original `due_date` / `completed_at` values remain truthful and immutable. On database reopen, segment context and occurrence history must be read from durable storage and yield the same result for the same `as_of`; no mutable streak counter is persisted.

### Schedule edits: prospective default

An edit does not retroactively run old occurrences through the replacement schedule by default. At the edit boundary:

- If the pre-edit current streak is active, carry that current value forward unchanged. The new rule governs periods after the boundary; subsequent qualifying completions extend the continuing run.
- If the pre-edit current streak is inactive, it remains inactive. Merely changing due date, recurrence, or anchor cannot create a current streak.
- `best` retains the best genuinely achieved run; a continuing run may span schedule segments. Both values are derived across segment boundaries and occurrences, never stored as counters.
- If a schedule edit occurs before any completion history exists, it establishes/replaces the prospective schedule context; it creates no streak.

The existing open-period grace and recurrence clamping rules remain in force within each segment. A future implementation must state how an edit boundary interacts with an already-open period and test that boundary explicitly; it must not resolve that edge by rewriting occurrence rows.

### Explicit retroactive opt-in

A user may explicitly opt into evaluating retained occurrences under the replacement cadence. This is not the default and is not enabled by this document. Credit extends backward only through periods whose stored occurrence `due_date` values actually form a consecutive sequence under the selected rule and anchor. A late completion's `completed_at` remains the true timestamp but does not turn an occurrence into a different due period. Gaps and occurrences outside the candidate sequence do not receive retroactive credit. The implementation must keep the prospective result and the opt-in result distinguishable and must not expose a new UI or API control without its own authorized implementation scope.

### Legacy-row policy

The existing schema cannot reveal every historical initial due date or prior rule/anchor edit. In particular, neither the current mutable due date nor the first surviving occurrence proves the original schedule context. A future migration must therefore:

1. Preserve every existing occurrence row and timestamp unchanged.
2. Mark missing pre-migration schedule origin/context as **unknown** rather than backfilling a guessed `initial_due`, edit boundary, or prior rule.
3. Establish a known forward-only schedule segment at an explicit migration/adoption boundary. Pre-boundary current/best derived from unknown context is not certified by this contract; no retroactive streak credit may be manufactured from inferred fields.
4. Leave any user-visible treatment of an unknown legacy streak explicit in the separately authorized migration/consumer acceptance. This docs ticket does not choose a data-loss workaround, rewrite history, or claim all legacy streaks are correct.

## Dated scenario matrix

All dates are UTC. `due_date@completed_at` means the occurrence's scheduled period and actual completion timestamp, respectively. `current/best` values below are the approved contract expectations, not a claim that current `main` implements edited schedule segments. The existing core's open-period grace is used where relevant.

| Scenario | Rule, anchor, and input history | Edit / `as_of` | Expected current / best |
|---|---|---|---|
| First due differs from anchor | `monthly(day=None)`; `initial_due=2026-01-30`, `anchor=2026-01-31`; occurrences `2026-01-30@2026-01-30T12:00:00`, `2026-02-28@2026-02-28T12:00:00`. The anchor supplies day 31; February clamps to day 28. | No edit; `as_of=2026-02-28`. | **2 / 2**. The first period is Jan 30, while the next anchored period is Feb 28. Do not replace one input with the other. |
| No completions | `daily`; `initial_due=anchor=2026-09-01`; no occurrences. | No edit; `as_of=2026-09-05`. | **0 / 0**. A due date or schedule alone creates no completion streak. |
| Missed period | `daily`; `initial_due=anchor=2026-09-01`; occurrences `2026-09-01@2026-09-01T12:00:00`, `2026-09-03@2026-09-03T12:00:00`; no Sep 2 occurrence. | No edit; `as_of=2026-09-03`. | **1 / 1**. The Sep 2 gap breaks the run; do not synthesize a row. |
| Completion, advance, and database reopen | `daily`; `initial_due=anchor=2026-09-01`; occurrence `2026-09-01@2026-09-02T00:30:00`. The task's current due advances to Sep 2; the occurrence remains attributed to Sep 1. | Close/reopen the store; evaluate at `as_of=2026-09-02`. | **1 / 1**. The open-period grace preserves the run, and the original due period and actual completion timestamp survive reopen unchanged. |
| Name-only edit | `daily`; `initial_due=anchor=2026-09-01`; occurrences due Sep 1 and Sep 2, completed at `2026-09-01T12:00:00` and `2026-09-02T12:00:00`. | Rename on Sep 2 without changing due/rule/anchor; `as_of=2026-09-02`. | **2 / 2**. No new segment and no history reinterpretation. |
| Due-date edit while active | `daily`; `initial_due=anchor=2026-09-01`; occurrences due Sep 1, 2, and 3, each completed at 12:00 UTC; current value before edit is 3. | At Sep 3, change the next due from Sep 4 to Sep 10; start the new segment with first due Sep 10. | **3 / 3 at the edit boundary**. The active value carries forward. After a qualifying Sep 10 completion, **4 / 4**. |
| Due-date edit while inactive | `daily`; `initial_due=anchor=2026-09-01`; only occurrence `2026-09-01@2026-09-01T12:00:00`. At `as_of=2026-09-03`, the open-period grace has expired. | Change the next due to Sep 10 and start a new segment. | **0 / 1** at the edit boundary. The edit does not reactivate the current streak; historical best remains 1. |
| Recurrence edit, prospective | Old rule `every 3 days`, `initial_due=anchor=2026-09-01`; occurrences due Sep 1, 4, and 7; current/best before edit are 3/3. | At Sep 7, change to `every 2 days`; next due is Sep 9. No retroactive opt-in. | **3 / 3 at the edit boundary**. After a qualifying Sep 9 completion, **4 / 4** across the segment boundary. |
| Anchor edit, prospective | `weekly(weekday=None)`, `initial_due=2026-09-02`, `anchor=2026-09-02` (Wednesday); occurrences due Sep 2, 9, and 16. | At Sep 16, change anchor to Sep 3 (Thursday); the next due is Sep 17. | **3 / 3 at the edit boundary**. The active run carries; the new anchor controls future periods only. |
| Monthly clamping | `monthly(day=None)`, `initial_due=anchor=2026-01-31`; occurrences due Jan 31, Feb 28, and Mar 31. | No edit; `as_of=2026-03-31`. | **3 / 3**. February clamps to its last day; the anchor remains day 31 for the next month's schedule. |
| Every-3-to-every-2 opt-in, compliant suffix | Old rule `every 3 days`, `initial_due=anchor=2026-01-01`; occurrences `2026-01-01@2026-01-01T12:00:00Z`, `2026-01-04@2026-01-04T12:00:00Z`, `2026-01-06@2026-01-06T12:00:00Z`, `2026-01-08@2026-01-08T12:00:00Z`, `2026-01-10@2026-01-10T12:00:00Z`. Change to `every 2 days` after Jan 4; explicitly opt in to retroactive evaluation under the new rule. | Evaluate at `as_of=2026-01-10` using the new cadence from Jan 1. | **4 / 4**. Jan 1→Jan 4 is not a two-day step; Jan 4→6→8→10 is a compliant suffix of four periods. Do not credit Jan 1 as part of that run. |
| Every-3-to-every-2 opt-in, unsupported history | Old rule `every 3 days`, `initial_due=anchor=2026-01-01`; occurrences `2026-01-01@2026-01-01T12:00:00Z`, `2026-01-04@2026-01-04T12:00:00Z`, `2026-01-07@2026-01-07T12:00:00Z`. Change to `every 2 days` after Jan 7; explicitly opt in; no new-rule completion yet. | Evaluate at `as_of=2026-01-07` under `every 2 days`. | **1 / 1**. The three-day gaps do not form a two-day run; the old three-period streak must not be credited as three. |
| Legacy schedule context absent | Stored task has current mutable `due=2026-09-10`, current rule `daily`, and current `anchor=2026-09-01`; retained occurrences are `2026-09-01@2026-09-01T12:00:00Z` and `2026-09-03@2026-09-03T12:00:00Z`. Persisted `initial_due` and any pre-migration rule/anchor/edit boundaries are unknown. | Before an explicit forward-only adoption boundary; evaluate at `as_of=2026-09-10`. | **Unknown / Unknown** for the uncertified pre-boundary schedule interpretation. Preserve rows; do not infer missing facts from current due or first occurrence. |

The opt-in examples use `Occurrence.due_date` as the period identity. `completed_at` is still retained as the real timestamp; completing early or late does not rewrite the period to a different cadence slot.

## Follow-on slices and test map

These are dependency-ordered future implementation targets, not work authorized or performed by COD-547:

1. **Persist schedule identity and compatibility.** Extend `crates/litany-core/src/model.rs`; add the versioned persistence change under `crates/litany-store/src/` alongside `schema.sql`; define explicit unknown legacy context and a forward-only adoption boundary. Test migration and reopen without changing any occurrence `due_date` or `completed_at`.
2. **Store transitions.** Update `crates/litany-store/src/lib.rs` so create establishes context, completion advances the mutable due without changing segment identity, name-only edits preserve context, and schedule edits atomically create a segment while retaining occurrence history. Add close/reopen and transaction-boundary tests.
3. **Pure derivation.** Extend `crates/litany-core/src/streak.rs` and, where needed, `crates/litany-core/src/recurrence.rs` to derive current/best across schedule boundaries for prospective default and explicit opt-in. Add deterministic fixed-date tests for every matrix row, including inactive edits, grace, missed periods, anchors distinct from first due, clamping, and compliant/noncompliant retroactive suffixes. Continue to receive `as_of` from the caller; do not add clock reads or mutable streak counters.
4. **Shared update contract and dispatch.** Update `api/operations.yaml` and `crates/litany-server/src/lib.rs` only after a separately reviewed contract defines how an explicit retroactive opt-in is represented. Regenerate all declared surfaces from the single operation definition; add dispatcher tests for create/update/complete/history/streak and migration-backed legacy behavior. Do not add an implicit retroactive default or a client toggle in this docs slice.
5. **Full repository acceptance.** For the implementation slices, run `cargo test --locked -p litany-core`, `cargo test --locked -p litany-store`, `cargo test --locked -p litany-server`, `cargo test --locked --workspace`, `cargo run --locked -p litany-codegen -- check`, `cargo fmt --all -- --check`, and `cargo clippy --locked --workspace --all-targets -- -D warnings`, plus migration/reopen integration checks. These are planned future gates, not results claimed for this document.

For COD-547 itself, acceptance is limited to this file being present and nonempty, `git diff --check`, an exact changed-path allowlist of `docs/schedule-context.md` and `README.md`, verification that each referenced source path exists, and review that every matrix scenario has explicit inputs and an unambiguous contract outcome. No runtime, migration, codegen, deployment, or release criterion is completed here.
