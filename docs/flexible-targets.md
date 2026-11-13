# Flexible period targets — prospective contract

**Status:** design and acceptance contract for separately reviewed future work. This document does not describe implemented flexible targets. The current Litany model, store, and generated operations remain unchanged; in particular, they do not yet persist target-policy versions, duration evidence, target snapshots, or calendar rules. The current model and existing completion contract are in `crates/litany-core/src/model.rs` and `api/operations.yaml`; storage and fixed-recurrence behavior are in `crates/litany-store/src/lib.rs`, `crates/litany-store/src/schema.sql`, `crates/litany-core/src/streak.rs`, and `crates/litany-server/src/lib.rs`. Existing schedule-history context is separately documented in `docs/schedule-context.md`.

This contract is a specification, not a test report, migration plan, runtime dependency choice, or authorization to implement its future slices. Expected fixture outputs below are design expectations, not product-test results.

## 1. Product model and compatibility boundary

A target asks whether a task has at least **N qualifying completions** or **N distinct qualifying local dates** in a selected period. The dimensions are independent:

- target count `N`;
- period kind: an arbitrary positive number of local calendar dates, calendar week, calendar month, or calendar year;
- window mode: nonoverlapping anchored periods or a rolling lookback;
- count mode: qualifying occurrence IDs or distinct resolved local-day buckets;
- optional per-occurrence duration threshold, entered as evidence by the caller (no timer);
- explicit anchor, week start where applicable, timezone/calendar identity, immutable policy version, and caller-supplied evaluation cutoff.

Both window modes and both count modes are required. Arbitrary positive day spans remain supported; a month or year is a calendar period, not an approximation of 30 or 365 days. Day windows count local calendar dates, not elapsed 24-hour blocks. Calendar week boundaries use an explicit week start. The task's review schedule must not silently define a target boundary.

Target membership uses the occurrence's true `completed_at`, not its scheduled `due_date` or position in a history list. Due-date attribution remains the input to existing fixed-recurrence streak semantics. A target read is pure: it cannot create an occurrence, advance a due date, change collapse/advance settings, or write a progress counter. Target policy edits are prospective immutable versions; they cannot silently reinterpret past facts. A window crossing an adoption/version boundary is clipped and marked partial rather than prorating `N` or manufacturing old-version credit/debt.

The existing model does not have these target fields or evidence semantics. Existing offset-free UTC completion wire syntax stays as documented in `api/operations.yaml`; `Z` below is fixture notation for UTC, not an unreviewed wire-format change. Existing legacy rows are not rewritten or retroactively assigned invented duration, timezone, policy, or history-coverage evidence. No client UI is part of this service contract. Surfaces, if separately authorized later, remain Hydra-generated from one operation definition.

## 2. Resolver request, window identity, and pure evaluator boundary

The proposed server-owned resolver (the future boundary in `crates/litany-server`; not an implementation authorization) accepts a request of this shape:

```text
ResolveTargetRequest {
  policy_selector: explicit_version(policy_version_id) | version_at_cutoff,
  cutoff_utc,
  window_selection
}

WindowSelection =
    anchored_containing_cutoff
  | anchored_period { policy_version_id, calendar_context_id, period_index: i64 }
  | rolling_lookback
```

The resolved immutable policy version carries `policy_version_id`, `calendar_context_id`, `zone_id`, `rules_manifest_id`, `count_mode`, target, period definition, optional duration threshold, and effective start plus optional exclusive effective end. `version_at_cutoff` must select exactly one version effective at the supplied instant or return a typed error. Explicit historical-version selection is separate and remains available only with its retained calendar context and rules.

Anchored and rolling selection are not interchangeable. An anchored policy rejects `rolling_lookback`; a rolling policy rejects both anchored selectors (`selection_mode_mismatch`). An explicit anchored period binds the period index to the exact policy-version and calendar-context identities; mismatched bindings fail before counting (`selection_identity_mismatch`). For day blocks, index zero begins at the version's explicit local-midnight anchor and index `j` is `j * span` local dates from it. A week index zero begins at the explicitly configured origin week boundary, whose weekday must match `week_start`. Month and year index zero begins at an explicit first-of-month or January 1 local-midnight origin. Negative indices are allowed only when representable and overlapping the version's effective interval. Containing selection returns the half-open anchored period containing the cutoff and its index. Explicit-index selection is independent of the cutoff and can select the previous closed period without moving the cutoff. No epsilon subtraction, implicit previous-period fallback, or arbitrary consumer-supplied UTC bounds is allowed. A future-only selected period or a period with no overlap with its version is an error; an empty evaluated slice at the exact start of an otherwise valid period is allowed.

The resolved window carries policy/calendar/rules identities, requested selector, period index when anchored, cutoff, nominal UTC bounds, version-effective bounds, evaluated UTC bounds, and ordered UTC day-bucket intervals with context-bound opaque bucket IDs. Local labels and rendering remain outside the core. The resolver proves the named-zone mapping; the pure evaluator cannot certify a timezone merely by checking interval shape. The evaluator validates interval order, clipping, identity consistency, and bucket coverage without I/O, clock reads, timezone discovery, or local rendering.

For a nonempty intersection:

```text
evaluated_start = max(nominal_start, effective_start)
evaluated_end   = min(nominal_end, cutoff_utc, effective_end if present)
```

All intervals are half-open: exact start is included; exact end, cutoff, and future instants are excluded. A cutoff before a version's effective start is `not_effective_at_cutoff`. A historical version evaluated after its end is clipped at that exclusive end and reports `version_boundary` when the nominal period crosses it. If clipping leaves no evidence, return an explicit empty slice at its valid boundary and its reason, never reversed bounds.

## 3. Calendar, rules, precision, and ranges

Use proleptic Gregorian calendar arithmetic. Proposed target-facing UTC timestamps preserve nanoseconds exactly and support years 0001 through 9999, seconds 00 through 59. Reject leap-second encodings, non-finite values, excess precision, out-of-range values, and unrepresentable intermediate local/UTC endpoints; never truncate or silently round. This does not narrow or rewrite existing fixed-recurrence rows. Unsupported legacy evidence is reported explicitly.

Counts and positive day spans are integers from 1 through 4,294,967,295. Duration values and thresholds are integer seconds from 0 through 4,294,967,295 inclusive. Reject booleans used as integers, nonpositive counts/spans, negative or fractional durations, non-finite values, and larger inputs. Use checked arithmetic for index-times-span and all count accumulation. Observed counts use unsigned 64-bit range; overflow beyond 18,446,744,073,709,551,615 returns `count_overflow`, without saturation, rollover credit/debt, or mutation. Any arithmetic-valid positive day span is supported unless its requested window is unrepresentable; there is no arbitrary span allowlist.

Anchored day blocks count local dates. Weeks use explicit `week_start`; month/year periods use actual calendar boundaries. Rolling days subtract local calendar dates while preserving the cutoff's local wall time. Rolling weeks subtract seven local dates independently of `week_start`. Rolling months/years subtract one calendar month/year with destination end-of-month clamping and preserve wall time. A rolling midday interval may intersect partial dates at both ends; distinct-day capacity is the actual number of nonempty date intersections, not automatically the number of dates subtracted.

Resolve ambiguous derived local boundaries to the earlier UTC candidate. A nonexistent derived boundary returns `nonexistent_local_boundary`; do not shift it. Apply the same rule to counting-day boundaries; a required boundary that is nonexistent or unrepresentable fails resolution. A caller-supplied cutoff is already an instant, so a repeated local wall time at the cutoff is not ambiguous; the rule concerns derived local boundaries.

Rules must be reproducible and bundled by the future server resolver. An immutable manifest records the IANA release identifier, the **actual** payload SHA-256, resolver implementation/dependency versions, and supported range. `calendar_context_id` binds the zone, manifest, and boundary rule; policy versions retain that identity. Evaluation must not read OS tzdb, clock, or network or silently adopt host updates. Missing historical manifest/payload returns `rules_unavailable`, never substitution. Resolver fixtures must run against the exact declared payload and record its real digest. No dependency has been selected and no payload digest is asserted here. The Denver values below are expected design fixtures, not proof of a Rust resolver or a pinned rules payload.

## 4. Occurrences, evidence, bounds, and verdicts

The pure evaluator consumes an immutable policy, resolved UTC window, occurrence facts, and explicit evidence scope. Each fact has stable occurrence ID, task ID, actual `completed_at`, and optional `duration_seconds`; due attribution remains separate. Identical repeats of one ID deduplicate. Contradictory copies of an ID (including task, timestamp, or duration differences) are input errors. Different IDs are never deduplicated.

A duration threshold applies to each occurrence independently; short sessions are never summed. Equality qualifies, and known zero qualifies against zero. If a threshold exists, absent duration is unresolved even when the threshold is zero. If there is no threshold, absent duration does not disqualify an otherwise valid legacy occurrence. A known qualifying occurrence qualifies its resolved local-day bucket in distinct-day mode even if another occurrence or a history gap on that same bucket is unresolved.

`EvidenceScope` is snapshot-bound and contains `snapshot_id`, covered UTC intervals, `complete_through_utc`, and explicit gaps. It must describe actual storage proof, not infer completeness from the first retained occurrence. Coverage is half-open and clipped to the evaluated slice; a gap overrides a coverage claim and malformed or contradictory coverage is an error. A complete-through instant excludes that instant. Snapshot identity binds facts, coverage, policy, and calendar context. The same persisted snapshot/policy/context/cutoff must reproduce the same semantic result after reopen. A newly added backdated fact creates a new snapshot identity and may change the result at the same cutoff; `as_of` alone does not freeze history.

`K` is the known qualifying lower bound. `U` is the possible upper bound for the evaluated slice:

- In completion mode, with complete history, add unresolved-duration unique IDs to `K`. Any uncovered nonempty past interval makes `U` unbounded (`null`).
- In distinct-day mode, each bucket contributes at most one. A bucket already known qualified cannot raise `U`, even if it contains missing durations or a gap. An unqualified bucket can raise `U` only when it contains an unresolved-duration fact or a nonempty past-coverage gap. Thus `U = K + potentially qualifying still-unqualified buckets`, never `K + raw unknown-duration count`.
- `count_exact` is true exactly when `U` is finite and `U = K`. Keep raw `unknown_duration_count` as a separate diagnostic.

Adoption/startup clipping and version clipping report `startup_partial` or `version_boundary` separately from storage gaps and never prorate `N`. Excluded old-version facts are neither counted nor retrospectively classified. Such nominal-policy partiality means a below-target result is unknown even when the evaluated-slice count itself is exact. Validate distinct-day feasibility against the nominal window's full capacity, not observed-slice length; an impossible target is `unattainable_target`, not partial failure. Future remainder of an open anchored period is not missing past evidence.

Verdict precedence is:

1. `K >= N` → `satisfied`, even with gaps or nominal partiality.
2. Otherwise, startup/version partiality → `unknown`.
3. Otherwise, `U > K` or unbounded `U` → `unknown`.
4. Otherwise, an anchored window with `cutoff < nominal_end` → `not_yet_satisfied`.
5. Otherwise, a complete closed anchored window or rolling lookback → `unsatisfied`.

A rolling interval is closed at its supplied cutoff; a later cutoff describes another interval. Compute `remaining_upper_bound = max(0, N-K)`. For finite `U`, `remaining_lower_bound = max(0, N-U)`; for unbounded `U`, it is zero. For below-target nominal partiality, the lower bound is also zero. `remaining_exact` is true when the two bounds agree, including satisfaction. Never report a single exact deficit when evidence can change it. Return the identities, nominal/evaluated bounds, `K`, `U`, `count_exact`, `N`, both remaining bounds, exactness, verdict, and independent gap/partiality reasons.

## 5. Fixed-input design fixtures

### Shared conventions

Unless a row overrides them: unzoned instants are UTC; the interval is half-open; the UTC calendar context is `cal-utc-1`; each explicitly named example policy version is selected with `explicit_version(policy_version_id)` and is effective from `2026-10-01T00:00:00Z` with no end; history is complete through the row's cutoff with no gaps; there is no duration threshold; every listed ID is a distinct occurrence for one synthetic task; and the snapshot contains exactly the listed facts. `Z` is fixture notation only. `count_exact` means `U=K`; `remaining_exact` means the two remaining bounds agree, so a partial-policy row can have an exact slice count but inexact remaining bounds. A displayed remaining interval `[x,x]` is exact; `[0,1]` is not. Each row states or inherits its window selector, bounds, count mode/target, evidence, and expected result. UTC windows are provided by the resolver in a future implementation; these examples do not claim a current runtime capability.

### A — anchored three-day boundary

`A-v1` is a three-local-date anchored policy with origin `2026-11-01T00:00:00Z`, `N=2`. Select `anchored_containing_cutoff` at `2026-11-03T12:00:00Z`. Nominal bounds are `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`; evaluated bounds are `[2026-11-01T00:00:00Z,2026-11-03T12:00:00Z)`. Complete evidence covers that evaluated interval. Facts: `a` at Nov 1 10:00Z, `b` at Nov 1 11:00Z, `c` at Nov 3 12:00Z, `d` at Nov 4 00:00Z.

- `A-completions`: completion mode; `c` is exactly cutoff and `d` is at period end, so only `a,b` count: `K=U=2`, exact, remaining `[0,0]`, `satisfied`.
- `A-days`: distinct-day mode; `a,b` share one bucket: `K=U=1`, exact, remaining `[1,1]`, `not_yet_satisfied`.
- `A-idempotency`: repeating unchanged `a` leaves both results unchanged. Reusing ID `a` with a different timestamp or other contradictory fact is an input error; it is not a second completion.

### B — rolling three-day boundary

`B-v1` is a three-local-date rolling policy, `N=2`. Select `rolling_lookback` at `2026-11-04T00:00:00Z`; nominal and evaluated bounds are `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`. Use the same stable facts `a,b,c,d` and complete coverage of the whole interval. `d` is exactly cutoff/end and is excluded.

- `B-completions`: completion mode gives `K=U=3` (`a,b,c`), exact, remaining `[0,0]`, `satisfied`.
- `B-days`: distinct-day mode gives `K=U=2` (Nov 1 and Nov 3), exact, remaining `[0,0]`, `satisfied`.
- `B-before-start`: in a separate snapshot move `a` to `2026-10-31T23:59:59Z`, leaving `b,c,d` unchanged. The moved fact is excluded. `b` and `c` remain two completions on two dates, so both completion and distinct-day modes give `K=U=2`, exact, remaining `[0,0]`, `satisfied`.

### C — explicit anchored-period versus containing-period selection

Use UTC rules context `cal-utc-1`; immutable version `c-v1` (and a separate otherwise-identical `c-roll-v1`) is effective from `2026-10-01T00:00:00Z`, with no end. Complete evidence runs through each cutoff. The anchored policy has a 13-local-date span and origin `2026-11-01T00:00:00Z`; `N=6`. Stable facts are `c1,c2` at Nov 1 10:00Z (two distinct IDs at that same instant), `c3` Nov 3 10:00Z, `c4` Nov 5 10:00Z, `c5` Nov 7 10:00Z, and `c6` Nov 9 10:00Z. Cutoff is `2026-11-14T00:00:00Z` in all three selector comparisons.

- `C-previous`: select `anchored_period {policy_version_id:c-v1, calendar_context_id:cal-utc-1, period_index:0}`. Nominal and evaluated bounds are `[2026-11-01T00:00:00Z,2026-11-14T00:00:00Z)`. Completion mode: `K=U=6`, exact, remaining `[0,0]`, `satisfied`. Distinct-day mode: `K=U=5`, exact, remaining `[1,1]`, `unsatisfied`. Echo index 0.
- `C-containing`: select `anchored_containing_cutoff` with the same anchored policy and cutoff. Nominal bounds are `[2026-11-14T00:00:00Z,2026-11-27T00:00:00Z)`; evaluated bounds are the valid empty slice `[2026-11-14T00:00:00Z,2026-11-14T00:00:00Z)`. Both modes: `K=U=0`, exact, remaining `[6,6]`, `not_yet_satisfied`. Echo index 1. The six prior facts do not roll forward.
- `C-rolling`: select `rolling_lookback` on `c-roll-v1`; nominal/evaluated bounds are `[2026-11-01T00:00:00Z,2026-11-14T00:00:00Z)`. Completion mode is `K=U=6`, remaining `[0,0]`, `satisfied`; distinct-day mode is `K=U=5`, remaining `[1,1]`, `unsatisfied` (rolling is closed).
- `C-selector-error`: ask `c-roll-v1` for `anchored_period`; return `selection_mode_mismatch` before counting. Passing a different policy-version or calendar-context identity to `anchored_period` returns `selection_identity_mismatch` before counting. No epsilon or implicit previous-period selection is valid.

### D — explicit Monday-week selection

`d-v1` is an anchored seven-local-date policy with explicit Monday week start and origin `2026-11-02T00:00:00Z`; `N=4`, `cal-utc-1`, effective from Oct 1, complete evidence. Facts: `d1` Nov 2 12:00Z, `d2` Nov 3 12:00Z, `d3` Nov 4 12:00Z, `d4` Nov 8 12:00Z, and `d5` Nov 9 00:00Z. Both requests use cutoff `2026-11-09T00:00:00Z`.

- `D-previous`: select `anchored_period {d-v1,cal-utc-1,0}`. Nominal/evaluated bounds `[2026-11-02T00:00:00Z,2026-11-09T00:00:00Z)` exclude `d5`. Both count modes give `K=U=4`, exact, remaining `[0,0]`, `satisfied`; echo index 0.
- `D-containing`: select `anchored_containing_cutoff`. Nominal bounds `[2026-11-09T00:00:00Z,2026-11-16T00:00:00Z)`; evaluated empty slice at Nov 9 00:00Z. `d5` is at the excluded cutoff. Both modes give `K=U=0`, exact, remaining `[4,4]`, `not_yet_satisfied`; echo index 1.

Sunday-start is a different explicitly versioned policy and must never be inferred from a weekly review schedule or substituted for the Monday policy.

### E — rolling month/year clamping and anchored leap year

Each rolling row below uses a separate immutable UTC policy version, explicit `rolling_lookback`, no end to the policy's effective interval, `N=1`, and complete evidence. Include one stable ID at the exact listed start and one at the exact cutoff. Both facts have no duration requirement. The start is included, cutoff is excluded; in either count mode `K=U=1`, exact, remaining `[0,0]`, `satisfied`.

- `E-month-2026` (`e-month-2026-v1`): policy version effective `2020-01-01T00:00:00Z`; cutoff `2026-03-31T12:00:00Z`; nominal/evaluated `[2026-02-28T12:00:00Z,2026-03-31T12:00:00Z)`. IDs `e26-start` at start and `e26-cutoff` at cutoff.
- `E-month-leap` (`e-month-leap-v1`): policy version effective `2020-01-01T00:00:00Z`; cutoff `2024-03-31T12:00:00Z`; bounds `[2024-02-29T12:00:00Z,2024-03-31T12:00:00Z)`. IDs `em-start`, `em-cutoff` at the respective bounds.
- `E-year-2025` (`e-year-2025-v1`): policy version effective `2020-01-01T00:00:00Z`; cutoff `2025-02-28T12:00:00Z`; bounds `[2024-02-28T12:00:00Z,2025-02-28T12:00:00Z)`. IDs `ey25-start`, `ey25-cutoff`.
- `E-year-leap-cutoff` (`e-year-leap-v1`): policy version effective `2020-01-01T00:00:00Z`; cutoff `2024-02-29T12:00:00Z`; bounds `[2023-02-28T12:00:00Z,2024-02-29T12:00:00Z)`. IDs `ey24-start`, `ey24-cutoff`.

`E-anchored-year` is a distinct anchored calendar-year policy `e-year-v1` effective `2020-01-01T00:00:00Z`, with origin `2024-01-01T00:00:00Z`, `cal-utc-1`, `N=2`, and cutoff `2025-01-01T00:00:00Z`. Select `anchored_period {e-year-v1,cal-utc-1,0}` (not containing selection at the exclusive end); nominal/evaluated bounds are `[2024-01-01T00:00:00Z,2025-01-01T00:00:00Z)`. Facts `ey-leap` at Feb 29 12:00Z, `ey-last` at Dec 31 12:00Z, and `ey-end` at exact end. Both count modes give `K=U=2`, exact, remaining `[0,0]`, `satisfied`; `ey-end` is excluded. Nominal capacity is 366 dates, not 365.

### F — Denver calendar-day and repeated-hour fixtures

These fixtures state expected resolver outputs for `America/Denver` under a future immutable test rules manifest. The manifest ID and actual payload digest must be recorded by that future resolver test; neither is selected or claimed here.

- `F-spring-days`: anchored three-local-date policy `denver-spring-v1`, effective `2020-01-01T00:00:00Z`, context `denver-spring-example` (fixture identity; future resolver run must supply its actual `rules_manifest_id` and digest). Select `anchored_period {policy_version_id: denver-spring-v1, calendar_context_id: denver-spring-example, period_index: 0}` at cutoff `2026-03-10T06:00:00Z`, the exclusive period end. Local bounds `[2026-03-07 00:00,2026-03-10 00:00)`, `N=3`. Expected UTC bounds `[2026-03-07T07:00:00Z,2026-03-10T06:00:00Z)` (71 hours); buckets are `[Mar 7 07:00Z,Mar 8 07:00Z)`, `[Mar 8 07:00Z,Mar 9 06:00Z)`, `[Mar 9 06:00Z,Mar 10 06:00Z)`. Facts `fs1` at Mar 7 12:00Z, `fs2` at Mar 8 12:00Z, `fs3` at Mar 9 12:00Z. Complete evidence; both count modes have `K=U=3`, exact, remaining `[0,0]`, `satisfied`.
- `F-fall-days`: anchored three-local-date policy `denver-fall-v1`, effective `2020-01-01T00:00:00Z`, context `denver-fall-example` (fixture identity; future resolver run must supply its actual `rules_manifest_id` and digest). Select `anchored_period {policy_version_id: denver-fall-v1, calendar_context_id: denver-fall-example, period_index: 0}` at cutoff `2026-11-03T07:00:00Z`, the exclusive period end. Local bounds `[2026-10-31 00:00,2026-11-03 00:00)`, `N=3`. Expected UTC bounds `[2026-10-31T06:00:00Z,2026-11-03T07:00:00Z)` (73 hours); buckets are `[Oct 31 06:00Z,Nov 1 06:00Z)`, `[Nov 1 06:00Z,Nov 2 07:00Z)`, `[Nov 2 07:00Z,Nov 3 07:00Z)`. Facts `ff1` Oct 31 12:00Z; `ff2` Nov 1 07:45Z; `ff3` Nov 1 08:45Z; `ff4` Nov 2 12:00Z. The two Nov 1 facts are distinct UTC instants in the same local-date bucket. Completion mode `K=U=4`; distinct-day mode `K=U=3`; both exact, remaining `[0,0]`, `satisfied` at `N=3`.

### G — duration threshold and unknown-duration qualification

Use UTC policy `g-v1`, context `cal-utc-1`, version effective before the interval, `anchored_containing_cutoff`, nominal bounds `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, cutoff `2026-11-03T00:00:00Z`, evaluated bounds `[Nov 1 00:00Z,Nov 3 00:00Z)`, complete evidence/no gaps, and threshold 600 seconds unless overridden.

- `G-mixed`: `N=2`; distinct IDs `g600` (Nov 1 10:00Z, 600s), `g599` (Nov 1 11:00Z, 599s), and `g-unknown` (Nov 1 12:00Z, duration absent). Completion mode: `K=1`, one unresolved-duration ID, `U=2`, `count_exact=false`, remaining `[0,1]`, `unknown`. Distinct-day mode: the date is already qualified by `g600`, so `K=U=1`, exact, remaining `[1,1]`, `not_yet_satisfied` for the open anchored period; the unknown on that same bucket does not increase `U`.
- `G-unknown-next-day`: move `g-unknown` to Nov 2 noon in a new snapshot. In distinct-day mode `K=1,U=2`, `count_exact=false`, remaining `[0,1]`, `unknown`.
- `G-no-threshold`: remove the threshold, use the same three IDs and `N=2`. All three count in completion mode (`K=U=3`); the one date counts once in distinct-day mode (`K=U=1`).
- `G-no-summing`: threshold 600, `N=1`, IDs `g300-a` and `g300-b` at separate times on Nov 1, each with 300 seconds. Neither qualifies: both modes have `K=U=0`, exact, remaining `[1,1]`, `not_yet_satisfied` for the open anchored period. Durations are never summed.
- `G-zero-known`: threshold 0, `N=1`, one Nov 1 fact `g0` with known duration 0. Both modes qualify: `K=U=1`, exact, remaining `[0,0]`, `satisfied`.
- `G-zero-unknown`: threshold 0, `N=1`, one Nov 1 fact `g0-missing` with absent duration. It is unresolved: both modes have `K=0,U=1`, inexact, remaining `[0,1]`, `unknown`. No duration is invented.

### H — policy-version boundary

Use UTC context `cal-utc-1`. Old policy `h-v1` ends exclusively at `2026-11-02T00:00:00Z`; new `h-v2` starts there. Select `anchored_containing_cutoff` on `h-v2`; its three-date anchored nominal period is `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, cutoff `2026-11-03T00:00:00Z`, and evaluated slice `[2026-11-02T00:00:00Z,2026-11-03T00:00:00Z)`. Evidence is complete on the evaluated slice, with no storage gaps. Facts: `h-a` Nov 1 noon, `h-b` Nov 2 noon, and, in the extended snapshot, `h-c` Nov 2 13:00Z. Target is 2.

- `H-one`: only `h-a,h-b`; `h-a` is excluded under the new version. Both count modes have `K=U=1` and `count_exact=true` for the evaluated slice, but nominal partiality makes `remaining_lower_bound=0`; remaining bounds `[0,1]`, `remaining_exact=false`, `version_boundary`, `unknown`.
- `H-satisfied`: add `h-c`; completion mode has `K=U=2`, remaining `[0,0]`, `satisfied` despite partiality. Distinct-day mode still has one qualified day (`K=U=1`, `count_exact=true`), but nominal partiality makes remaining `[0,1]` (`remaining_exact=false`), `version_boundary`, `unknown`.

A timezone edit creates a new version boundary; it does not retroactively change old bucket identities or reclassify old-version facts.

### I — missing history, coverage gaps, and exactness

All anchored UTC examples use version `i-v1`, context `cal-utc-1`, `anchored_containing_cutoff`, nominal bounds `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, cutoff `2026-11-03T00:00:00Z`, target 2, no duration threshold, and complete coverage except where the gap is specified. `I-empty-closed` overrides the selector to `rolling_lookback` and supplies its own cutoff/bounds.

- `I-one-gap`: fact `i1` Nov 1 10:00Z is known qualifying; explicit uncovered past interval `[2026-11-01T13:00:00Z,2026-11-01T14:00:00Z)` lies on the already-qualified Nov 1 bucket. Completion mode has `K=1,U=null`, `count_exact=false`, remaining `[0,1]`, `unknown`. Distinct-day mode has `K=U=1`, exact, remaining `[1,1]`, `not_yet_satisfied` because the period is open. Preserve the gap reason even though the bucket count is exact.
- `I-gap-next-day`: move the gap to `[2026-11-02T13:00:00Z,2026-11-02T14:00:00Z)`, on an unqualified bucket. Distinct-day mode has `K=1,U=2`, inexact, remaining `[0,1]`, `unknown`.
- `I-known-satisfies`: with the Nov 2 gap retained, add known qualifying fact `i2` Nov 2 10:00Z. Completion mode has `K=2,U=null`; distinct-day mode has `K=U=2` because both buckets are qualified. In both modes remaining bounds are `[0,0]`, exact, `satisfied` despite the gap; retain the gap reason.
- `I-empty-closed`: policy `i-empty-v1` is a complete, gap-free three-date rolling window at cutoff `2026-11-04T00:00:00Z`, nominal/evaluated `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, with no facts and target 2. Both modes have `K=U=0`, exact, remaining `[2,2]`, `unsatisfied`.
- Empty clipped startup/adoption evidence is the fully specified `O-empty-clipped` case below: `K=U=0` on its observed slice, remaining `[0,2]`, `unknown`, not a complete-window failure.

### J — completion-time membership versus scheduled attribution

Use UTC context `cal-utc-1`, no threshold, complete evidence, and one-day anchored policy `j-v1` effective `2026-10-01T00:00:00Z`, with origin `2026-11-01T00:00:00Z`, `N=1`. `J-early` contains occurrence `j-early` with `due_date=2026-11-03` and `completed_at=2026-11-01T12:00:00Z`; select `anchored_period {policy_version_id:j-v1,calendar_context_id:cal-utc-1,period_index:0}` at cutoff `2026-11-02T00:00:00Z`, nominal/evaluated `[Nov 1 00:00Z,Nov 2 00:00Z)`, so it counts. `J-late` contains `j-late` with `due_date=2026-11-01` and `completed_at=2026-11-03T12:00:00Z`; select `anchored_period {policy_version_id:j-v1,calendar_context_id:cal-utc-1,period_index:2}` at cutoff `2026-11-04T00:00:00Z`, nominal/evaluated `[Nov 3 00:00Z,Nov 4 00:00Z)`, so it counts. In each row, both count modes have `K=U=1`, exact, remaining `[0,0]`, `satisfied`. Existing fixed-recurrence streak attribution remains to the original due date in both cases. A before/after read leaves task and occurrence rows, scheduled debt, streak, and advance behavior unchanged.

### N — exact nanosecond cutoff and one synthetic write

Policy `n-v1`, UTC context `cal-utc-1`, `anchored_containing_cutoff` at the first cutoff, anchored nominal `[2026-11-01T00:00:00.000000000Z,2026-11-04T00:00:00.000000000Z)`, target 3 completions, complete evidence, no duration threshold. Initial snapshot `n-before` has exactly four IDs: `nano-before-start` at `2026-10-31T23:59:59.999999999Z`, `nano-start` at exact start, `nano-before-cutoff` at `2026-11-03T11:59:59.999999999Z`, and `nano-end` at exact period end.

- `N-before-write`: cutoff `2026-11-03T12:00:00.000000000Z`; evaluated bounds `[Nov 1 00:00:00.000000000Z,Nov 3 12:00:00.000000000Z)`. Only `nano-start` and `nano-before-cutoff` count: `K=U=2`, exact, remaining `[1,1]`, `not_yet_satisfied`.
- `N-after-one-write-at-cutoff`: perform exactly one synthetic completion write, ID `nano-cutoff`, at the exact cutoff. The final five-ID fact set is the four initial IDs plus this single ID; there is no other distinct-ID cutoff fact. A read at the same cutoff still has `K=U=2`, exact, remaining `[1,1]`, `not_yet_satisfied`; persisted history may show the new row while progress excludes it. Repeating the read writes nothing.
- `N-one-nanosecond-later`: cutoff `2026-11-03T12:00:00.000000001Z`; evaluated end advances by one nanosecond. `nano-cutoff` now counts and `nano-end` remains excluded: `K=U=3`, exact, remaining `[0,0]`, `satisfied`.

A second distinct ID written at the exact cutoff is not deduplicated with `nano-cutoff`; at cutoff+1ns both would count and produce four. Never add that second ID to this fixture to force the expected count of three.

### K — ambiguous and nonexistent rolling boundaries

Use `America/Denver` and separate future rules contexts whose real immutable manifests/digests remain to be selected and recorded by resolver tests. Both policies are effective before their requested interval; the example context labels below are fixture identities, not actual rules manifests.

- `K-ambiguous-start`: rolling three local dates, policy `k-fall-v1` effective `2020-01-01T00:00:00Z`, context `denver-fall-example`, cutoff `2026-11-04T08:30:00Z` (local Nov 4 01:30). The derived local start Nov 1 01:30 has candidates `2026-11-01T07:30:00Z` and `2026-11-01T08:30:00Z`; choose the earlier, `07:30Z`. Nominal/evaluated bounds are `[2026-11-01T07:30:00Z,2026-11-04T08:30:00Z)`. Facts `k1` at Nov 1 07:45Z and `k2` at Nov 1 08:45Z are two completions on the same local date. Target 2: completion mode `K=U=2`, remaining `[0,0]`, `satisfied`; distinct-day mode `K=U=1`, remaining `[1,1]`, `unsatisfied` for this closed rolling window.
- `K-nonexistent-start`: rolling three local dates, policy `k-spring-v1` effective `2020-01-01T00:00:00Z`, context `denver-spring-example`, `N=2`, cutoff `2026-03-11T08:30:00Z` (local March 11 02:30). Request both count modes. The derived start `2026-03-08 02:30` does not exist. Return `nonexistent_local_boundary`; there is no nominal/evaluated UTC window, count, remaining bound, or verdict. Required day-bucket boundary failures follow the same rule.

### L — anchored calendar month/year and nominal capacity

- `L-month`: `cal-utc-1`, policy `l-month-v1` effective `2020-01-01T00:00:00Z`, anchored calendar-month origin `2024-02-01T00:00:00Z`, explicit period index 0, cutoff `2024-03-01T00:00:00Z`, target 2. Nominal/evaluated bounds `[2024-02-01T00:00:00Z,2024-03-01T00:00:00Z)`. Facts `lm1` at exact start, `lm2` Feb 29 12:00Z, `lm-end` at exact end. Both count modes have `K=U=2`, exact, remaining `[0,0]`, `satisfied`; last fact excluded, nominal capacity 29 dates. A distinct-day target of 30 for this same nominal period returns `unattainable_target`, even if only part of the window has been observed.
- `L-year`: `cal-utc-1`, policy `l-year-v1` effective `2020-01-01T00:00:00Z`, anchored calendar-year origin Jan 1 2024, explicit index 0, cutoff Jan 1 2025, target 2. Nominal/evaluated bounds `[2024-01-01T00:00:00Z,2025-01-01T00:00:00Z)`. Facts `ly1` Feb 29 12:00Z, `ly2` Dec 31 12:00Z, `ly-end` at exact end. Both modes have `K=U=2`, exact, remaining `[0,0]`, `satisfied`; final fact excluded, capacity 366 dates.

### M — remaining duration edge cases

`M-duration-detail` uses UTC policy `m-v1`, context `cal-utc-1`, `anchored_containing_cutoff`, nominal `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, cutoff `2026-11-03T00:00:00Z`, evaluated `[Nov 1 00:00Z,Nov 3 00:00Z)`, target 2, threshold 600, complete evidence. Facts `m600` at Nov 1 10:00Z with 600 seconds and `m-missing` Nov 1 noon with absent duration. Completion mode `K=1,U=2`, inexact, remaining `[0,1]`, `unknown`. Distinct-day mode `K=U=1`, exact, remaining `[1,1]`, `not_yet_satisfied` because the only bucket is already qualified and the window is open. Move `m-missing` to Nov 2 noon in `M-unknown-new-day`: distinct-day mode becomes `K=1,U=2`, inexact, remaining `[0,1]`, `unknown`. Threshold 0 with known duration 0 qualifies; threshold 0 with absent duration remains unknown (`G-zero-known`, `G-zero-unknown`). Two 300-second occurrences do not satisfy a 600-second per-occurrence threshold (`G-no-summing`). Invalid duration values are typed input errors, not unknown evidence.

### O — startup and rolling warm-up

Use UTC, three-local-date period, target 2, version/policy adoption at `2026-11-02T00:00:00Z`, cutoff `2026-11-03T00:00:00Z`, complete evidence over the evaluated slice and no storage gap. One qualifying fact `o1` is Nov 2 noon; the two-fact variant adds distinct ID `o2` at Nov 2 13:00Z. For anchored policy `o-anchored-v1`, origin Nov 1, select `anchored_containing_cutoff`; nominal bounds are `[Nov 1 00:00Z,Nov 4 00:00Z)`. For rolling policy `o-rolling-v1`, select `rolling_lookback`; nominal bounds are `[Oct 31 00:00Z,Nov 3 00:00Z)`. Both versions have effective start Nov 2 and no end; in both cases evaluated bounds are `[Nov 2 00:00Z,Nov 3 00:00Z)` and the reason is `startup_partial`.

- `O-one`: either selector with `o1`: `K=U=1` and `count_exact=true` for the evaluated slice, but nominal partiality gives remaining bounds `[0,1]` (`remaining_exact=false`) and `unknown`, in either count mode.
- `O-two`: with `o1,o2`, completion mode has `K=U=2`, remaining `[0,0]`, `satisfied` despite partiality. Distinct-day mode has `K=U=1`, `count_exact=true`, but partiality gives remaining `[0,1]` (`remaining_exact=false`) and `unknown`.
- `O-empty-full`: policy `o-empty-v1`, UTC context, `rolling_lookback` at cutoff `2026-11-04T00:00:00Z`; nominal/evaluated `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, target 2, complete evidence, no facts. Both modes have `K=U=0`, exact, remaining `[2,2]`, `unsatisfied`.
- `O-empty-clipped`: either selector's clipped bounds above, no facts, complete evidence on the evaluated slice, and nominal partiality; both modes have `K=U=0`, remaining `[0,2]` (`remaining_exact=false`), `unknown`.

Pre-adoption facts never qualify under the new version.

### P — retained historical timezone/version contexts

Old anchored policy `p-v1` uses UTC context `cal-utc-1`, effective `[2026-10-01T00:00:00Z,2026-11-02T00:00:00Z)`. New anchored policy `p-v2` uses Denver context `cal-denver-v2-example` (fixture identity; actual rules manifest/digest is future work) and is effective `[2026-11-02T00:00:00Z,infinity)`. Both have a three-local-date anchored origin of local Nov 1, target 2, and cutoff `2026-11-03T00:00:00Z`. Facts: `p-x` at Nov 1 12:00Z and `p-y` at Nov 2 12:00Z; complete evidence on each evaluated slice. Explicit selector for the anchored comparisons is `anchored_period` at index 0, bound to the selected version and calendar context.

- `P-v1-index0`: explicitly select `anchored_period {p-v1,cal-utc-1,0}`. Nominal bounds `[Nov 1 00:00Z,Nov 4 00:00Z)`, evaluated bounds `[Nov 1 00:00Z,Nov 2 00:00Z)`. Only `p-x` counts: completion and distinct-day modes each have `K=U=1`, exact slice count, remaining `[0,1]`, `version_boundary`, `unknown`.
- `P-v2-index0`: explicitly select `anchored_period {policy_version_id:p-v2,calendar_context_id:cal-denver-v2-example,period_index:0}`. Nominal bounds `[Nov 1 06:00Z,Nov 4 07:00Z)`, evaluated bounds `[Nov 2 00:00Z,Nov 3 00:00Z)`. Only `p-y` qualifies: both modes `K=U=1`, exact slice count, remaining `[0,1]`, `version_boundary`, `unknown`. Do not count `p-x` under v2 or reinterpret v1 in Denver.
- `P-v1-historical-rolling`: use a separate immutable rolling policy `p-roll-v1` effective `[2026-10-01T00:00:00Z,2026-11-02T00:00:00Z)`, retaining UTC context `cal-utc-1` and the old rules; explicitly select `rolling_lookback` at cutoff `2026-11-02T00:00:00Z`; nominal/evaluated bounds `[2026-10-30T00:00:00Z,2026-11-02T00:00:00Z)`. With target 2, only `p-x` qualifies; `p-y` is after cutoff. Both modes `K=U=1`, exact, remaining `[1,1]`, `unsatisfied` for the complete closed window. If the old rules payload is missing, return `rules_unavailable`, never use v2 or host rules.

Target-policy reads/edits do not change the separate COD-547 schedule/streak history contract.

### Q — numeric, identity, and representability errors

Use explicit policy/version/context, selector, and UTC cutoff for every request; errors produce no fabricated window/count/verdict and no state mutation.

- `Q-range`: policy `q-range-v1`, UTC context `cal-utc-1`, `rolling_lookback`, cutoff `2026-11-04T00:00:00Z`, intended three-date bounds `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, `N=1` when valid. Separate invalid requests with target or span `0`, `-1`, or `4294967296` return `invalid_range` before evaluation. For the valid-boundary example, a distinct snapshot uses completion mode, threshold `4294967295`, and one fact `q-duration-max` at Nov 1 12:00Z with duration `4294967295`; both count modes have `K=U=1`, exact, remaining `[0,0]`, `satisfied`. Threshold `4294967296` returns `invalid_range`; `0.5` and non-finite `NaN` return `invalid_type`. Booleans are not integers for this contract.
- `Q-underflow`: policy `q-underflow-v1` effective `0001-01-01T00:00:00Z`, UTC context `cal-utc-1`, rolling one-date selection at cutoff `0001-01-01T00:00:00Z`, completion mode, `N=1`; the start is unrepresentable. Return `unrepresentable_window`; there are no bounds, facts, count, remaining value, or verdict.
- `Q-overflow`: `q-v1`, UTC context `cal-utc-1`, one-day anchored span with origin `9999-12-31T00:00:00Z`, select `anchored_period {q-v1,cal-utc-1,0}` at cutoff `9999-12-31T12:00:00Z`, completion mode, `N=1`, no facts. Its exclusive end would be in year 10000; return `unrepresentable_window` before evaluating facts.
- `Q-selector`: policy `q-roll-v1`, UTC context `cal-utc-1`, rolling three-date policy, `N=1`, no threshold or facts; an `anchored_period` request at cutoff `2026-11-04T00:00:00Z` returns `selection_mode_mismatch`. A separate anchored `q-anchor-v1` request for `anchored_period {policy_version_id:q-anchor-v1,calendar_context_id:wrong-calendar,period_index:0}` returns `selection_identity_mismatch` before counting.
- `Q-capacity`: policy `q-cap-v1`, UTC context `cal-utc-1`, three-date anchored origin Nov 1, select `anchored_period` index 0, nominal `[Nov 1 00:00Z,Nov 4 00:00Z)`. At closed cutoff Nov 4, four distinct-day target returns `unattainable_target` even with known fact `q-cap-known` at Nov 1 noon and complete coverage. In a separate request with target 2, cutoff Nov 2 12:00Z and evaluated bounds `[Nov 1 00:00Z,Nov 2 12:00Z)`, the same known fact and complete-through evidence give `K=U=1`, exact, remaining `[1,1]`, `not_yet_satisfied`; the otherwise feasible target is not invalid just because only part of its period has elapsed.
- `Q-count-overflow`: policy `q-count-v1`, UTC context `cal-utc-1`, completion mode, `N=1`, three-date anchored bounds `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, select explicit index 0, cutoff at the exclusive end, complete evidence. In an isolated arithmetic-boundary fixture, seed a checked accumulator at `18446744073709551615`; adding distinct qualifying ID `q-overflow-next` returns `count_overflow`, without saturating, wrapping, or mutating state.

### R — synthetic reopen, legacy evidence, and attribution safety

These are future isolated synthetic store/evaluator acceptance cases, not migrations or runtime results performed by this docs slice.

- `R-reopen`: synthetic legacy facts `r1` (`due_date=2026-11-01`, `completed_at=2026-11-01T10:00:00Z`, duration absent) and `r2` (`due_date=2026-11-02`, `completed_at=2026-11-02T11:00:00Z`, duration absent). With no duration threshold, UTC policy `r-v1` effective `2026-10-01T00:00:00Z`, `anchored_containing_cutoff`, nominal `[2026-11-01T00:00:00Z,2026-11-04T00:00:00Z)`, cutoff `2026-11-03T00:00:00Z`, evaluated `[Nov 1 00:00Z,Nov 3 00:00Z)`, target 3, and complete coverage, both completion and day modes have `K=U=2`, exact, remaining `[1,1]`, `not_yet_satisfied`. A synthetic schema-upgrade/reopen fixture preserves IDs, due dates, completion timestamps, and absent durations exactly. Reopening the same facts, snapshot, policy, and calendar context reproduces the same semantic result. Read-only evaluation leaves task/occurrence rows unchanged.
- `R-late-fact`: add a backdated distinct ID `r3` at Nov 2 12:00Z in a new snapshot. The same cutoff/policy now yields `K=U=3`, remaining `[0,0]`, `satisfied`; the snapshot identity changes. Do not claim `as_of` alone freezes the earlier result.
- `R-duration-legacy`: use separate policy `r-duration-v1`, effective `2026-11-01T00:00:00Z`, with threshold 600 and the same UTC nominal/evaluated bounds, cutoff, facts, and complete evidence as `R-reopen`. The old rows' missing duration remains unresolved, never silently zero or qualifying. Completion mode has `K=0,U=2`, inexact, remaining `[0,2]`, `unknown`. If prior history coverage is also missing, `U` is unbounded. The forward-only adoption boundary is explicit; the first retained occurrence does not prove earlier completeness.
- `R-attribution`: use the exact `J-early` and `J-late` policies, selected anchored indices, cutoffs, bounds, and facts above. The early completion due Nov 3 but completed Nov 1 noon belongs to the Nov 1 target window; the late completion due Nov 1 but completed Nov 3 noon belongs to the Nov 3 target window. Both count modes yield `K=U=1`, exact, remaining `[0,0]`, `satisfied`. Existing streak attribution still uses the original `due_date`; the target read creates no occurrence and advances no due date.

## 6. Future slice boundaries and documentation acceptance

These are a dependency design only, not created issue IDs, written dependency relations, ready work, or implementation authority:

| Slice | Separately gated scope | Prerequisite |
|---|---|---|
| A | This prospective contract and complete fixtures in `docs/flexible-targets.md` plus a resolving `README.md` link. | Accepted design and current LITANY-19 gate. |
| B | Pure UTC evaluator and supplied-UTC fixtures; no I/O or clock reads. | A |
| C | Server-owned resolver, immutable bundled rules mapping, and selector/calendar fixtures. | A |
| D | Immutable policy versions, occurrence duration, and evidence persistence with isolated synthetic migration/reopen. | A |
| E | One Hydra operation definition, shared dispatch, and deterministic generated CLI/HTTP/MCP/client surfaces. | B, C, and D |
| F | Isolated SQLite plus real generated HTTP/CLI/MCP/client readback, including same-instant cutoff and mismatched selection. | E |

Future implementation acceptance includes workspace formatting, locked Clippy/tests, codegen freshness, two byte-identical generations, and synthetic integration as scoped by its own issue. E must preserve explicit parameter locations, one definition per operation, and no hand-written route/registry fork. No future slice authorizes a release, publication, live migration/history access, deployment, consumer repin, timer, UI, or constitution amendment. Actual follow-on tickets need current duplicate coverage and real blocked-by relations when they are created.

For this docs-only slice, acceptance is limited to:

```sh
test -s docs/flexible-targets.md
git diff --check "$BASE"...HEAD
git diff --name-only "$BASE"...HEAD
python3 -c 'import os,subprocess; paths=set(subprocess.check_output(["git","diff","--name-only",os.environ["BASE"]+"...HEAD"],text=True).splitlines()); assert paths=={"README.md","docs/flexible-targets.md"}, paths'
python3 -c 'from pathlib import Path; import re; p=Path("README.md"); links=re.findall(r"\]\(([^)]+)\)",p.read_text()); assert "docs/flexible-targets.md" in links; assert (p.parent/"docs/flexible-targets.md").is_file(); paths=[".creed/config/context.md","README.md","docs/schedule-context.md","crates/litany-core/src/model.rs","crates/litany-core/src/streak.rs","crates/litany-store/src/lib.rs","crates/litany-store/src/schema.sql","crates/litany-server/src/lib.rs","api/operations.yaml"]; assert all(Path(x).is_file() for x in paths)'
```

Set `BASE` to the actual unchanged implementation base before running those commands. Also verify every repository path referenced by the authored document and every relative README/document Markdown link. Independently map every fixture row against the exact retained A–J inventory and consolidated accepted contract, including the corrected C/D selectors, outputs, single-cutoff-write clarification, and all section-5 variants. Confirm that no sentence claims current runtime support, a selected timezone dependency, an actual rules payload/digest, or product test results. Record actual base/head, commands and exit codes, changed paths, link/path validation, and complete-row review evidence in the PR handoff. Ordinary CI may run, but it does not replace these docs checks. No evaluator/resolver, rules payload, Rust/store migration, runtime integration, or product build is required to accept this documentation.
