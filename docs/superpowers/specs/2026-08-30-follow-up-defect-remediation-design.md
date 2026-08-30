# Follow-up Defect Remediation Design

**Date:** 2026-08-30

**Status:** Approved by delegated user authority (`你自主推进`)

**Baseline:** `3fc3c904075c29272adbd09f1959421d29214e60`

## Goal

Close the confirmed defects found by the post-release review while preserving the
existing CLI flags, Action inputs, JSON shapes, grouping semantics, and supported
feature combinations. The work also updates GitHub CI and the composite Action so
that the repaired behavior is exercised and its cache is actually persisted on
supported runners.

## Scope

This design includes:

1. `--me github:<login>` identity resolution and case-insensitive email lookup.
2. Remote deduplication across every selected repository's GitHub `origin`.
3. Per-commit author/co-author identity normalization.
4. Windows repository selector and exclude matching.
5. Non-zero fractional-day duration handling and accurate filter diagnostics.
6. Future GitHub `--since` rejection.
7. A v4 rolling-window cache lifecycle with stable semantic keys.
8. Bounded, visibly partial GitHub user-email resolution.
9. Correct 429/403 rate-limit decisions.
10. Cross-platform Action cache persistence without changing Action inputs.
11. CI coverage for formatting, strict Clippy, locked dependencies, release, and
    Windows behavior.
12. Documentation updates needed to explain existing public behavior and partial
    identity-resolution guarantees.

Out of scope:

- pushing, tagging, publishing, or changing package version;
- new CLI flags or removal/renaming of existing flags and Action inputs;
- JSON schema redesign;
- macOS CI, an MSRV declaration, or prebuilt binary distribution;
- benchmark-free rewrites of Git diff, hierarchical aggregation, or GitHub
  language apportionment;
- broad splitting of `main.rs` or `github/api.rs` unrelated to these defects.

## Compatibility Contract

- All current CLI flags and aliases remain accepted with the same meanings.
- All existing Action inputs and the `svg-path` output remain unchanged.
- Flat and hierarchical JSON keys, value types, and envelopes remain unchanged.
- `--group` remains an ordered fallback list; `--groups` remains subgroup levels.
- GitHub data still rejects author grouping because contribution records do not
  provide author identity.
- Existing cache files are never deleted. The v4 namespace naturally misses v3
  entries.
- New incomplete-data and identity-resolution information is written to stderr;
  successful JSON stdout remains machine-compatible.
- `--no-cache` means no cache initialization, read, or write. If combined with
  `--refresh-cache`, it wins and emits one warning.

## Approaches Considered

### A. Stable semantic current key with explicit coverage — selected

Use stable keys for rolling/anchored windows and store the actual requested range,
checked boundary, observation time, and completeness in the value. Commit history
is trimmed and incrementally extended. GitHub contribution aggregates are fully
refetched whenever an open rolling boundary changes because the aggregate cannot
safely remove events that leave the left edge.

This is the smallest correct design. It makes the expensive per-repository commit
history reusable without pretending that an aggregate can be subtracted.

### B. Immutable calendar contribution shards

Calendar shards can reuse completed contribution data, but arbitrary fractional
rolling windows require moving first and last edge shards. Correct saturation and
summary merging would substantially expand the schema and request planner. This is
rejected for the current repair.

### C. Date-rounded keys or full overwrite under exact keys

Rounding changes requested time semantics, especially fractional days. Exact keys
continue to miss on every run. Full overwrite becomes useful only after adding the
stable scope and coverage envelope from Approach A, so it is not a separate design.

## Architecture

### 1. Time and cache policy

One `DateTime<Utc>` is captured at each command boundary and passed into the
GitHub orchestration. Internal functions do not independently call `Utc::now()`
when deriving one request's windows or cache keys.

The internal model distinguishes:

```rust
enum CachePolicy {
    ReadOnly,
    Refresh,
    Disabled,
}

enum CacheWindowScope {
    Fixed {
        from: DateTime<Utc>,
        until_exclusive: DateTime<Utc>,
    },
    Rolling {
        lookback_nanoseconds: i64,
    },
    Anchored {
        from: DateTime<Utc>,
    },
}

struct QueryWindow {
    scope: CacheWindowScope,
    requested_from: DateTime<Utc>,
    until_exclusive: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    completed: bool,
}
```

`--days` and named multi periods create `Rolling` scopes. A lone `--since`
creates an open `Anchored` scope. A range with an elapsed explicit `--until`
creates a completed fixed scope. Future `--since` is rejected before token or
network access. Every range remains `[from, until_exclusive)`.

`duration_for_days` converts checked fractional seconds with ceiling semantics so
every positive accepted input represents at least one second. Existing displayed
integer day metadata continues to use the current ceiling behavior.

Cache policy is derived once:

| Flags | Policy | Read | Write |
|---|---|---:|---:|
| default | `ReadOnly` | yes | no |
| `--refresh-cache` | `Refresh` | yes | yes |
| `--no-cache` | `Disabled` | no | no |
| both | `Disabled` | no | no |

### 2. v4 cache scope and envelope

Both contribution and history keys move to the `v4_` namespace. Existing
collision-free component encoding remains.

The semantic scope contains the authenticated target node ID, lower-cased login,
include flags, and window scope. History keys additionally contain lower-cased
owner/repository identity. Rolling keys encode the duration, not moving timestamps;
anchored keys encode the fixed start and an open marker; completed keys encode exact
bounds.

```rust
struct CacheEnvelope<T> {
    requested_from: DateTime<Utc>,
    checked_until: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    completeness: Completeness,
    payload: T,
}

enum Completeness {
    Complete,
    Incomplete(Vec<IncompleteReason>),
}
```

Coverage fields are required in v4; missing or invalid fields produce a warning and
a cache miss. Saturated contribution results may be stored only as incomplete and
must replay their warning. Capped history is not written as complete cache data.

Contribution invariants:

- a payload describes exactly its envelope range;
- open contribution data with changed bounds is fully refetched;
- aggregate payloads are never gap-added or left-edge-subtracted;
- completed fixed entries can be reused indefinitely if complete;
- saturated results never prove a missing repository inactive.

History invariants:

- complete cache coverage is `[requested_from, checked_until)`;
- refresh begins at `checked_until`, not at the latest commit timestamp;
- rolling refresh removes commits before the new start, fetches the right-edge gap,
  and deduplicates only non-empty OIDs;
- an empty successful gap advances `checked_until`;
- incompatible ranges, clock rollback, malformed data, or incomplete coverage cause
  a visible miss and full fetch;
- a page-capped result remains partial and is not written as complete.

For an open history window, both `ReadOnly` and `Refresh` may trim cached events
and fetch the uncovered right-edge gap so the command result is current. Only
`Refresh` writes the resulting envelope; `ReadOnly` leaves the cache unchanged.

### 3. Shared user identity resolution

`--exclude @user` and `--me github:user` share one per-command resolution index.
The expression is parsed before identity resolution so each distinct login is
resolved once. Noreply-address login matching remains local and requires no token.

```rust
struct IdentityResolutionReport {
    login: String,
    emails: BTreeSet<String>,
    repositories_examined: usize,
    logical_requests: usize,
    truncated_repositories: bool,
    truncated_commits: bool,
    failures: Vec<IdentityLookupFailure>,
}
```

Resolution remains deliberately bounded: one repository page, at most eight
non-fork repositories, and at most twenty commits per repository. A command resolves
at most eight distinct requested logins in deterministic sorted order; additional
logins are left unresolved and covered by the same partial warning. `hasNextPage`, a
REST `Link: rel="next"`, budget exhaustion, or request failure marks the report
partial. One actionable stderr warning states that known emails are used and other
emails may be missed. Partial exclude rules remove known matches only; partial
`--me` includes known and noreply matches only.

Email identity keys use trimmed ASCII-lowercase email. Name fallback is used only
when email is empty. `filter.rs`, user-resolution maps, and remote-dedup maps all use
the same canonical key.

No token or a failed resolver does not silently pretend the result is complete:
the command emits a warning and continues with local noreply matching. Existing
successful exit behavior is retained.

### 4. Multi-repository remote deduplication

Remote dedup stays separate from username-to-email discovery. Every selected
`RepoInput` is processed in deterministic repository-ID order:

1. inspect its `origin` only, preserving the current public contract;
2. parse GitHub owner/repository;
3. collect canonical author/co-author emails only from commits with that `repo_id`;
4. resolve unresolved emails against that repository;
5. merge lower-cased email-to-login mappings using deterministic first-wins;
6. warn on conflicting successful mappings without changing an earlier result.

A later selected origin is still attempted if an earlier origin could not resolve
an email. Failures preserve separate raw identities.

### 5. Author/co-author normalization

One shared helper defines a commit-local identity key. At extraction and aggregation
boundaries:

- duplicate co-author trailers collapse to the first displayed identity;
- a co-author identical to the primary author is removed from the co-author role;
- email comparison is case-insensitive;
- empty-email identities fall back to normalized name;
- same name with different non-empty emails remains distinct.

Primary identity wins over self-coauthor attribution. Distinct co-authors continue
to receive one overlapping author attribution, while root/ancestor/global commit
totals remain based on original commit partitions and count the commit once.

### 6. Platform matching and diagnostics

Repository labels, basenames, collision detection, selectors, and exclude matching
share one platform-aware comparison boundary: Windows is case-insensitive; other
platforms retain case-sensitive behavior. GitHub owner/repository identity is
case-insensitive on every platform, and every GitHub fetch/card/multi repository
exclude or repository-language lookup explicitly selects that provider mode rather
than inheriting the host filesystem mode.

Diagnostics distinguish:

- no commits in the requested period;
- commits exist but none match committer/language filters;
- future/reversed GitHub ranges;
- partial identity resolution;
- permission 403 versus rate-limit responses;
- saturated contribution and capped history data.

### 7. HTTP retry decisions

Retry classification is extracted into a pure decision function.

- 429 uses a valid `Retry-After` first, supporting delta seconds and HTTP-date.
- 429 with exhausted primary limit may use `X-RateLimit-Reset`; otherwise it falls
  back to the configured bounded delay.
- 403 retries only with valid `Retry-After`, or when
  `X-RateLimit-Remaining == 0` and reset is valid.
- ordinary permission 403 fails immediately.
- 408, 5xx, connect, request, and timeout failures retain bounded retry behavior.
- a server-requested delay beyond the accepted wait bound fails visibly rather than
  retrying early.

Production keeps the existing timeout and retry count. Tests use pure decision
assertions and zero-delay local TCP responses; no real GitHub request is needed.

### 8. Composite Action cache lifecycle

`DiskCache` accepts an internal environment override:

```text
LOGIT_GITHUB_CACHE_DIR=<exact cache directory>
```

When absent, all existing platform defaults remain unchanged. The composite Action
uses `${{ runner.temp }}/logit-github-cache` for both `actions/cache` and the
environment variable, so Windows and Unix runners agree.

The Action retains every current input and output. It adds `--refresh-cache`
internally exactly once for `card` and `multi`. The data-cache primary key includes
the v4 schema, runner OS, a configuration digest, run ID, and run attempt. The digest
contains the username and data-affecting request inputs, so raw user input never
appears in the cache key; a stable schema/OS/digest restore prefix selects the newest
compatible previous snapshot. Failed runs do not replace the last successful
snapshot.

The binary installation remains source-hash keyed and uses `cargo install --locked`.
Existing argv-array and input-validation safety remains mandatory.

### 9. CI

CI avoids an OS-by-feature Cartesian product:

- Ubuntu quality job:
  - `cargo fmt --all -- --check`;
  - `cargo clippy --locked --all-targets --all-features -- -D warnings`;
  - `cargo check --locked --no-default-features`;
  - `cargo test --locked --all-features`;
  - `cargo build --locked --release --all-features`.
- Windows job:
  - `cargo test --locked --all-features`.

This directly covers Windows selector/exclude and Action cache-path behavior. macOS
and MSRV remain future improvements until an explicit support contract is chosen.

## Error and Data Flow

Input validation and time resolution occur before cache/client/network work. Cache
errors remain warning-and-fresh-fetch except when the requested input itself is
invalid. Network permission and exhausted-rate-limit errors remain command errors.
Partial provider data remains usable but all partial reports, request failures, and
skipped-over-budget logins are summarized into one deduplicated command-level stderr
warning. JSON stdout never mixes warnings with data.

## Testing Strategy

All new behavior follows failing-first tests.

- Pure tests: fractional duration, platform comparison, identity normalization,
  cache scope/key, retry decisions, coverage validation.
- Aggregation tests: self co-author, duplicate trailer, mixed-case email, same name
  with different emails, hierarchical nodes, grand totals, excluded languages.
- Local CLI fixtures: Windows case variants, filter diagnostics, `--me` evaluator.
- GitHub TCP tests: future range rejection, identity partial reports, 429/403 headers,
  all selected origins, stable v4 keys, history gap coverage, contribution full
  refresh, saturation/cap behavior.
- Action tests: no expression interpolation or eval, literal argv safety, exactly one
  refresh flag, matching cache path/env, unique save key and stable restore prefix.
- Compatibility tests: CLI help snapshots/parse, Action input names, JSON shapes,
  group resolver behavior.
- Full gates: fmt, strict locked Clippy, debug all-feature tests, release all-feature
  build/tests, LSP diagnostics, direct release CLI checks.

## Stage and Commit Strategy

The user authorized autonomous progress and phase commits. Each commit is created
only after its targeted tests and integration checks pass; no push or tag is made.

1. Planning artifacts: this design and the reviewed implementation plan.
2. Local identity/statistics/platform repairs.
3. GitHub window/cache/retry/identity repairs.
4. Action, CI, and documentation adaptation.
5. Final review corrections, only if reviewers find blockers.

## Completion Criteria

- Every confirmed defect has a failing-first regression and passing implementation.
- A second rolling history refresh fetches only the right-edge gap and matches a
  fresh no-cache result.
- A rolling contribution refresh fetches the exact full new window and matches a
  fresh no-cache result without retaining expired aggregate data.
- `--no-cache --refresh-cache` performs zero cache initialization/read/write.
- v3 cache fixtures naturally miss under v4 without deletion.
- Partial identity, saturation, and history caps are never presented as complete.
- `--me github` uses the shared resolver once and mixed-case emails match.
- Remote dedup can resolve from a later selected GitHub origin.
- Self/duplicate co-authors count once per commit identity.
- Existing CLI flags, Action inputs/output, JSON shapes, and grouping semantics pass
  compatibility tests.
- Ubuntu and Windows CI definitions cover the agreed gates.
- Debug and release full suites, strict Clippy, formatting, LSP, real CLI surfaces,
  and final Oracle + Reviewer receipts all pass on one current artifact identity.
