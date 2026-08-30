# Project-wide Defect Remediation Design

Date: 2026-08-28
Status: approved by delegated user authority

## Goal

Repair confirmed security, correctness, reliability, and incomplete-feature defects across logit's public CLI, local Git analysis, GitHub data path, Action, SVG output, and TUI/table presentation. The work must preserve compatible behavior where a historical contract exists and must add regression coverage for every repaired defect.

## Completion Boundary

This remediation is complete when:

1. Every confirmed P0/P1 issue in this document has a failing-first regression test and passes after the fix.
2. The directly related P2 network timeout/retry and error-visibility issues are fixed.
3. Local and GitHub group plans produce the same group keys, ordering, metrics, and totals in table and TUI output.
4. `cargo test --all-features`, formatting, clippy, diagnostics, and real CLI/TUI/SVG surfaces pass.
5. No unrelated refactor, speculative optimization, or new query language is introduced.

Potential issues that cannot be reproduced, statically proven, or tied to a public contract are reported separately rather than changed.

## Evidence Baseline

- HEAD before remediation: `ecb0e3c3281e098ac0d0bedb2f788c7e3f85b0cf`.
- Rust/Cargo: 1.95.0, edition 2024.
- Baseline: 171 unit tests and 3 integration tests pass with `--all-features`.
- Runtime-confirmed defects:
  - `stats --group repo,author -f json` produces a flat `{periods,totals}` result despite stale README tree examples.
  - `--groups` help claims it overrides `--group`, while implementation prepends the selected primary group.
  - Multi-group TUI emits a warning, no data, and exit code 0.
  - `--show-email full` omits an author email present in Git history.
- Historical contract: commit `d088fb5` defines `--group` as fallback primary-group candidates and `--groups` as subgroup levels beneath the selected primary. That intent takes precedence over the contradictory help line and stale README.

## Approaches Considered

### A. Independent patches

Lowest initial change count, but leaves group, sort, columns, and totals duplicated between renderers. It would fix symptoms while preserving the main source of drift. Rejected.

### B. Controlled presentation unification plus surgical fixes

Normalize grouping once, build one structured presentation model for table and TUI, and repair all unrelated confirmed defects at their existing boundaries. This preserves most modules and limits architectural change to the area that requires it. Selected.

### C. Unified local/GitHub domain rewrite

Would make all data sources share a new end-to-end model, but changes JSON schemas, caching, and aggregation simultaneously. The compatibility and verification cost is disproportionate. Rejected.

## Architecture

### 1. Group plan normalization

Introduce a single group-plan resolver used by local `stats` and `github fetch`.

The plan contains:

- selected primary group;
- ordered subgroup levels;
- supported dimensions for the current data source;
- whether the output is flat or hierarchical.

Contract:

- `--group` remains an ordered fallback list. The first supported dimension with more than one value is selected; language remains the final fallback.
- `--groups` supplies subgroup levels beneath that selected primary. It does not override the primary candidates.
- Duplicate levels are removed only when the duplicate is the selected primary; other duplicates are explicit errors.
- Language may only be the final level because one commit can span languages.
- Unique grouping levels are skipped consistently for flat and hierarchical output.
- Local data supports repo, author, period, and language.
- GitHub data supports repo, period, and language. Explicit author grouping fails with an actionable nonzero error because GitHub contribution records have no author identity.
- `github fetch` gains the same subgroup input where its supported dimensions permit it.

CLI help and README will document this exact behavior and use `--groups` in multi-level examples.

### 2. Shared table/TUI presentation model

Add a renderer-neutral model under `src/output/` with:

- visible columns in requested order;
- flattened rows with depth, label, row kind, metrics, and optional language detail;
- one totals row derived from the same aggregate result;
- sort order resolved before either renderer sees the rows;
- number values retained as numbers until final formatting.

Table output converts the model to text and color spans. TUI converts the same model to Ratatui rows and preserves navigation/view toggling. TUI no longer assumes a fixed Period column or five fixed metrics.

Parity contract:

- group labels, hierarchy depth, row ordering, selected columns, sort, number format, inline language rows, and totals are equal between table and TUI;
- visual borders, colors, navigation, and truncation may differ;
- JSON remains a complete machine-readable data format and is not trimmed by presentation-only `--columns` or number formatting;
- hierarchical JSON continues to serialize `GroupNode` data, while non-hierarchical JSON retains the existing `{periods, totals}` envelope.

### 3. Local query and repository identity

Normalize repository inputs before analysis:

- canonicalize paths when possible;
- deduplicate repeated and overlapping discoveries;
- apply `--repo` selection before expensive analysis;
- retain a stable repository identity separate from the basename display label;
- use the basename only when unique, and add the shortest distinguishing parent path when names collide.

Apply commit-level filters before aggregation:

- `--committer` matches committer name or email;
- language filtering excludes commits with no matching file language, matching the CLI's “Filter commits” contract;
- invalid `since > until`, negative/non-finite days, and unsupported ranges fail before analysis.

Date-only `--until YYYY-MM-DD` is inclusive for the whole UTC calendar day by converting it to an exclusive next-day boundary. Existing date-only `--since` remains inclusive from midnight.

### 4. Author identity and deduplication

Preserve the full `Name <email>` identity in raw author aggregation. Presentation-time dedup then has enough information to implement:

- `none`: distinct email identities remain separate;
- `name`: identities with the same display name merge;
- `remote`: resolved GitHub identities merge when available;
- `show-email`: none/simple/full changes labels without changing totals.

Co-author language and metric accounting remains intact.

### 5. Git diff correctness

- Merge commits remain counted as commits but do not add a second copy of changes already counted from reachable parent commits.
- Enable rename similarity detection so a pure rename does not become artificial delete/add churn.
- Retain binary deltas as file changes with zero line additions/deletions so files and path exclusions remain truthful.
- Invalid Git timestamps become explicit analysis errors rather than silently mapping to Unix epoch.

### 6. Exclusion and partial failures

Change exclude parsing to return validation errors. Unknown qualifiers, malformed expressions, and empty qualified groups fail with a clear diagnostic and nonzero exit rather than becoming whole-repository exclusions.

Scanner and analysis behavior:

- every skipped filesystem error is visible;
- duplicate error printing is removed;
- partial success may still produce data with warnings;
- if every requested/discovered repository fails, the command returns nonzero rather than “No commits found”.

### 7. Action security and retry behavior

The composite Action will:

- map `${{ inputs.* }}` into environment variables rather than interpolating them into shell source;
- validate command, booleans, retry count, delay, days, and period lists;
- build a Bash argv array and invoke `"${cmd[@]}"` without `eval`;
- append each non-empty exclusion as one argument without `xargs` re-parsing;
- treat `retry-count` as retries after the initial attempt, matching its documentation;
- execute at least once when retry count is zero;
- preserve and return the actual child exit code after the final failure;
- avoid printing the token or an executable reconstructed command line containing unsafe values.

Arbitrary output paths remain a deliberate CLI capability; this remediation secures argument transport without adding an unrelated sandbox policy.

### 8. SVG/XML safety

Register Tera templates under autoescaped XML/SVG names or apply an equivalent mandatory XML-escape boundary to every dynamic text/attribute value. Username, title, generated period labels, language names, and JSON-loaded strings containing `<>&"'` must render as text, never markup.

SVG tests must parse the result as well-formed XML and assert that injected elements/attributes are absent.

### 9. GitHub cache and exact time data

Version cache keys and include all identity dimensions that affect content:

- authenticated/user node identity;
- repository owner/name with collision-free component encoding;
- time range and cache schema version;
- request modes that change included repositories.

Contribution-window cache values store both repository rows and `ContributionSummary`; old entries naturally miss rather than receiving a complex migration.

Retain exact commit timestamps through period aggregation. Weekly buckets may be derived for week output, but day and month output must bucket original timestamps, never a normalized Monday. Fetched batch data is filtered against each repository's own range before merge/cache.

RFC3339 comparisons parse timestamps into instants. Distinct commits must use an available stable identity; if the API response lacks an OID, deduplication must avoid collapsing records solely because date/additions/deletions happen to match.

### 10. GitHub network reliability

- Add a finite default HTTP client timeout without adding a new CLI flag.
- Retry bounded transient transport errors plus 408, 429, and 5xx with existing rate-limit handling.
- Apply equivalent transient handling to REST identity lookups where practical.
- URL-encode REST query parameters through reqwest's query builder.
- Treat `hasNextPage=true` without a cursor as an incomplete-response error.
- Warn when API-imposed repository/history limits make the result incomplete.
- Surface cache initialization/write/parse failures as warnings while still allowing fresh network results.

## Confirmed Defect Set

### P0

- Action shell injection and incorrect failure propagation.
- SVG/XML injection.
- GitHub commit-history cache reuse across different users.

### P1

- Group contract/help/README mismatch and renderer drift.
- Multi-group TUI silent success with no output.
- GitHub unsupported author grouping silently returning empty author data.
- `--repo` and `--committer` not wired.
- Author email discarded before dedup/display.
- Same-basename repositories merged; repeated paths double-counted.
- Date-only until excludes most of the named day; reversed/invalid ranges accepted.
- Invalid exclude qualifier can exclude an entire repository.
- Merge churn double-counting, missing rename detection, and binary files disappearing from file counts.
- Language filter counts commits that contain no requested language.
- GitHub cached contribution summaries become zero.
- Day/month output uses normalized Monday timestamps.
- Batched repositories leak commits outside their individual fetch window.
- Equivalent RFC3339 instants are compared lexically.

### Directly related P2

- Missing HTTP timeout and incomplete transient retry coverage.
- Missing pagination/cap incompleteness diagnostics.
- Silent cache write/parse failures.
- All-repository analysis failure returning success.

## Error Handling

Input contract violations fail before expensive work and identify the offending flag/value. Unsupported data-source group dimensions fail explicitly. Partial filesystem/network/cache degradation is either retried or warned; no confirmed incomplete result is presented as unqualified success. Security boundary failures never fall back to shell evaluation or unescaped markup.

## Verification Scenarios

### Local CLI

1. Two repositories with the same basename remain separate; duplicate input paths do not change totals.
2. `--repo`, `--committer`, language, date, email, and dedup filters alter only their intended records.
3. Invalid days/date ranges/exclude rules exit nonzero with stable diagnostics.
4. Merge, rename, and binary fixtures produce expected commit, line, and file totals.

### Group and output parity

1. Zero subgroup, one subgroup, and multiple subgroups are tested across repo/author/period/language constraints.
2. Table and TUI snapshots contain identical semantic rows for columns, sort, number format, inline language details, and totals.
3. GitHub rejects author grouping and supports valid repo/period/language plans.
4. JSON envelopes remain compatible and hierarchical JSON contains the same group tree.

### Security

1. Action inputs containing spaces, quotes, semicolons, command substitutions, and multiline excludes remain single literal argv values.
2. Action child failure is retried exactly as documented and the last nonzero status is returned.
3. SVG inputs containing XML metacharacters produce well-formed XML with no injected element.

### GitHub data/cache

1. Two users querying the same repository never share history cache content.
2. Contribution summary survives a cache hit.
3. Commits at day/month/week boundaries land in exact expected buckets.
4. Mixed per-repository windows exclude out-of-range commits before merge.
5. Timeout, 429/transient retry, missing cursor, and cache-write failure paths are observable and bounded.

### Regression and surface

- Run targeted tests in failing and passing states.
- Run `cargo fmt --check`.
- Run `cargo clippy --all-targets --all-features -- -D warnings`.
- Run `cargo test --all-features`.
- Run representative CLI commands against temporary Git fixtures.
- Render TUI via `ratatui::backend::TestBackend` and inspect the buffer.
- Parse generated SVG as XML.
- Verify the Action script with a stub executable in an isolated temporary environment where available; retain static assertions otherwise.

## Non-goals

- Removing or renaming existing CLI flags.
- Trimming JSON fields based on table column preferences.
- Adding a general query language or configurable timeout surface.
- Replacing the entire cache framework or TUI interaction architecture.
- Broad performance tuning, style cleanup, or module rewrites unrelated to confirmed defects.
- Fixing speculative edge cases without reproducible or direct static evidence.
