# Follow-up Defect Remediation Implementation Plan

> **For agentic workers:** Use the subagent-driven-development skill to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 baseline `3fc3c904075c29272adbd09f1959421d29214e60` 上以失败优先回归修复已确认的本地身份、GitHub 时间/缓存/重试/身份、Action 持久化与 CI 缺陷，同时保持全部既有公共兼容契约。

**Architecture:** 保持现有模块边界并做外科式演进：`src/main.rs` 只负责一次性解析、时钟捕获和命令级编排，`src/github/api.rs` 拥有可测试的窗口、v4 envelope/state machine、重试和 provider 报告，local author/repository 规范化仍留在现有 `git`、`analyze`、`exclude`、`filter`、`stats`、`output` 模块。缓存以稳定语义 scope 作 key、以 coverage/completeness 作 value；所有网络行为都通过现有本地 TCP stub 验证，Action/CI/README 仅适配已批准行为，不引入新产品 surface。

**Tech Stack:** Rust edition 2024、Cargo、chrono 0.4、serde/serde_json 1、reqwest 0.13 blocking、git2 0.20、clap 4、assert_cmd 2、tempfile 3、GitHub composite Action Bash、GitHub Actions YAML、PowerShell 7 验证命令。

**Spec:** `docs/superpowers/specs/2026-08-30-follow-up-defect-remediation-design.md`

**Global Constraints:**
- Baseline is exactly HEAD `3fc3c904075c29272adbd09f1959421d29214e60`; implementation must not argue from unstated main-session history.
- All current CLI flags and aliases remain accepted with the same meanings; no flag is removed or renamed and no new public flag is added.
- All existing Action inputs and the `svg-path` output remain unchanged.
- Flat and hierarchical JSON keys, value types, and envelopes remain unchanged.
- `--group` remains an ordered fallback list; `--groups` remains subgroup levels.
- GitHub data still rejects author grouping because contribution records do not provide author identity.
- Existing cache files are never deleted; the v4 namespace naturally misses v3 entries.
- New incomplete-data and identity-resolution information is written to stderr; successful JSON stdout remains machine-compatible.
- `--no-cache` means no cache initialization, read, or write; when combined with `--refresh-cache`, `Disabled` wins and exactly one warning is emitted.
- Cache policy is fixed as default `ReadOnly` (read yes/write no), `--refresh-cache` `Refresh` (read yes/write yes), and `--no-cache` `Disabled` (read no/write no).
- Every request range is `[from, until_exclusive)` and one `DateTime<Utc>` is captured at each command boundary; internal request/key derivation must not independently call `Utc::now()`.
- Rolling contribution coverage changes cause a complete refetch of the new exact range; aggregate payloads are never gap-added or left-edge-subtracted.
- History refresh starts at `checked_until`, trims events older than the new rolling start, advances coverage after an empty successful gap, and deduplicates only non-empty OIDs.
- Missing/invalid v4 coverage is a warning miss; saturated contributions and capped history remain visibly incomplete and never prove completeness or inactivity.
- Identity email keys are trimmed ASCII-lowercase; empty-email identities fall back to normalized names.
- `--exclude @user` and `--me github:user` share one per-command, case-insensitive login resolution index; each distinct login is resolved at most once.
- Identity resolution remains bounded to one repository page, at most eight non-fork repositories, and at most twenty commits per repository; partial known emails remain usable with one actionable stderr warning.
- Remote dedup is independent from username-to-email discovery and inspects the `origin` of every selected repository in deterministic repository-ID order.
- Self/duplicate co-authors are removed commit-locally; primary identity wins, while same-name/different-nonempty-email identities remain distinct.
- Repository selector/exclude comparison is case-insensitive on Windows and case-sensitive elsewhere; GitHub owner/repository identity is case-insensitive on every platform.
- 429 honors valid `Retry-After` before reset/fallback; 403 retries only for valid `Retry-After` or exhausted primary limit with valid reset; ordinary permission 403 fails after one request.
- Production timeout and retry count remain unchanged; tests use only the existing TCP stub, explicit clock values, `TempDir`, and local Git fixtures—never the real network.
- The Action retains every current input/output, appends `--refresh-cache` internally exactly once for both `card` and `multi`, uses one identical cache path/env expression, and preserves argv-array/input-validation safety.
- The Action data key includes schema v4, runner OS, a configuration digest containing username/data-affecting request inputs, run ID, and run attempt; raw inputs do not appear in the key, its schema/OS/digest restore prefix is stable, and source installation uses `cargo install --locked`.
- Ubuntu CI runs fmt, strict locked all-target/all-feature Clippy, locked no-default check, locked all-feature tests, and locked release all-feature build; Windows runs locked all-feature tests.
- No macOS job, MSRV declaration, prebuilt distribution, dependency addition, package installation, version change, push, or tag is in scope.
- Do not broadly split `src/main.rs` or `src/github/api.rs`; do not rewrite Git diff, hierarchical aggregation, or GitHub language apportionment.
- Implementation executors may be recommended only as `coding` or `complex`; `quick`, planner, Reviewer, Oracle, and plan-critic are not implementation executors.
- This planning session performs no Git write. Phase commit messages below are permission-bounded handoff metadata only; an executor must not commit without explicit Git-write authorization and must never push or tag under this plan.

---

## Execution Waves and Commit Boundaries

| Wave | Tasks | Entry condition | Concurrency/file rule | Green boundary |
|---|---|---|---|---|
| 1 | Task 1 and Task 2 | plan handoff accepted | May run in parallel: Task 1 owns `main.rs`/`github/api.rs`; Task 2 owns author/analyze/aggregator/presentation | Both targeted suites green |
| 2 | Task 3 | Tasks 1–2 green | Serial because it reopens `main.rs` and `analyze.rs` | Foundation/local stage green |
| 3 | Task 4 | Task 1 green and Wave 2 boundary recorded | Serial ownership of `github/api.rs` begins | Contribution cache tests green |
| 4 | Task 5 | Task 4 green | Same `github/api.rs`; no parallel edit | History state-machine tests green |
| 5 | Task 6 | Task 5 green | Same `github/api.rs`; no parallel edit | Retry/report tests green |
| 6 | Task 7 | Tasks 3 and 6 green | Reopens `main.rs`, `filter.rs`, `exclude.rs`, and `github/api.rs` after both upstream contracts stabilize | GitHub identity integration stage green |
| 7 | Task 8 | Tasks 1 and 4 green; Task 7 preferred complete | Owns `github/cache.rs`, `action.yml`, `tests/action_test.rs` | Action/cache persistence green |
| 8 | Task 9 | Tasks 1–8 green | CI/docs/compat/final verification only; product edits are limited to correcting a demonstrated blocker | All final gates and review packet complete |

If workers cannot run concurrently, execute Task 1 → Task 2 → Task 3 → Task 4 → Task 5 → Task 6 → Task 7 → Task 8 → Task 9. A dependent task must not start while its prerequisite is RED.

Phase boundaries and the only allowed semantic messages (messages only, no Git command):

1. Planning artifacts (`spec` plus this plan): `docs: plan follow-up defect remediation`.
2. After Tasks 1–3 and their targeted tests pass: `fix: normalize query windows and local identities`.
3. After Tasks 4–7 and their targeted/integration tests pass: `fix: repair GitHub cache retry and identity flows`.
4. After Tasks 8–9 and every final gate pass: `fix: persist Action cache and harden CI`.
5. If formal review finds an eligible blocker and only after the blocker regression passes: `fix: address follow-up review blockers`.

Each boundary stages only files named by its completed tasks, excludes unrelated worktree changes, and requires fresh `git status --short` plus `git diff --check` evidence. No boundary authorizes push or tag.

## File Map

| File | Planned responsibility | Tasks |
|---|---|---|
| `src/main.rs` | Capture one command clock, derive `QueryWindow`/`CachePolicy`, reject future GitHub start before client work, distinguish filter diagnostics, share login resolution once, and iterate every selected origin | 1, 3, 7 |
| `src/filter.rs` | Collect GitHub logins from parsed `MeExpr` and match identity-map emails through the shared canonical key | 7 |
| `src/git/author.rs` | Define canonical email/commit identity and normalize self/duplicate co-author trailers | 2 |
| `src/analyze.rs` | Apply extraction-boundary co-author normalization and one Windows-aware repository comparison boundary | 2, 3 |
| `src/exclude.rs` | Resolve logins case-insensitively to canonical emails and use platform-aware repository matching | 3, 7 |
| `src/stats/aggregator.rs` | Enforce commit-local identity uniqueness in flat/tree aggregation without changing root/ancestor/global commit totals | 2 |
| `src/output/presentation.rs` | Preserve every primary/co-authored/net/language field when display-level dedup merges normalized author rows | 2 |
| `src/github/api.rs` | Own `CachePolicy`, `CacheWindowScope`, `QueryWindow`, v4 keys/envelopes/completeness, contribution/history state machines, retry decisions, bounded identity reports, and TCP-stub regressions | 1, 4, 5, 6, 7 |
| `src/github/cache.rs` | Honor `LOGIT_GITHUB_CACHE_DIR` as an exact internal override while preserving all platform defaults | 8 |
| `action.yml` | Persist the v4 data cache cross-platform, compute a safe configuration digest, install locked, and add one refresh flag without changing inputs/output or argv safety | 8 |
| `tests/action_test.rs` | Keep executable Bash argv safety and add Action input/output/cache/install/refresh plus CI static contract tests | 8, 9 |
| `.github/workflows/ci.yml` | Define the cost-bounded Ubuntu quality and Windows behavior jobs exactly as approved | 9 |
| `README.md` | Explain existing `--me github`, partial identity warnings, cache policy/persistence, and unchanged Action behavior | 9 |
| `tests/fixture_test.rs` | Exercise real CLI future-range validation, noreply `--me`, Windows matching, filter diagnostics, help/JSON/group compatibility, and stderr/stdout separation | 1, 3, 7, 9 |
| `src/cli.rs` | Keep declarations unchanged; add only an inline exact option/short-alias surface test so hidden/deprecated accepted flags are covered as well as visible help | 9 |
| `Cargo.toml`, `Cargo.lock` | Verification-only dependency/lock surfaces; no dependency or package metadata changes are planned | 9 |

No new Rust source module is planned. New tests remain inline beside private state-machine helpers or in the two existing integration test files; do not split large files for unrelated cleanup.

### Task 1: Establish exact time, cache-policy, and window-scope contracts

**Depends on:** none

**Files:**
- Modify: `src/main.rs` — `duration_for_days`, `resolve_time_range`, `fetch_github_data`, `cmd_github_multi`, and inline tests
- Modify: `src/github/api.rs` — introduce policy/window models and injectable cache initialization seam
- Modify: `tests/fixture_test.rs` — pre-I/O future-range regression
- Test: inline tests in `src/main.rs` and `src/github/api.rs`; `tests/fixture_test.rs`

**Interfaces:**
- Consumes: existing `days: Option<f64>`, `since: Option<&str>`, `until: Option<&str>`, `no_cache: bool`, `refresh_cache: bool`, and one command-boundary `observed_at: DateTime<Utc>`.
- Produces: `CachePolicy::{ReadOnly, Refresh, Disabled}`; `CachePolicy::from_flags(no_cache: bool, refresh_cache: bool) -> (CachePolicy, bool)` where the boolean requests the one conflict warning; `CachePolicy::{can_read, can_write}(self) -> bool`; `CacheWindowScope::{Fixed { from, until_exclusive }, Rolling { lookback_nanoseconds }, Anchored { from }}`; `QueryWindow { scope, requested_from, until_exclusive, observed_at, completed }`; `resolve_github_query_window(days, since, until, observed_at) -> anyhow::Result<QueryWindow>`; `initialize_cache_for_policy(policy, factory) -> Option<DiskCache>`.

**Recommended executor:** `complex`

- [ ] **Step 1: Write baseline-failing time/policy/window tests**

Add these concrete tests and keep the supplied fixed clock:

```rust
#[test]
fn positive_fractional_days_round_up_to_one_second() {
    assert_eq!(
        duration_for_days(0.000_001, "--days").unwrap(),
        chrono::Duration::seconds(1)
    );
    assert_eq!(
        duration_for_days(0.5, "--days").unwrap(),
        chrono::Duration::hours(12)
    );
}

#[cfg(feature = "github")]
#[test]
fn github_query_windows_use_one_clock_and_semantic_scopes() {
    use crate::github::api::{CachePolicy, CacheWindowScope};

    let now = fixed_now();
    let rolling = resolve_github_query_window(Some(0.5), None, None, now).unwrap();
    assert_eq!(rolling.requested_from, now - chrono::Duration::hours(12));
    assert_eq!(rolling.until_exclusive, now);
    assert_eq!(rolling.observed_at, now);
    assert!(!rolling.completed);
    assert_eq!(
        rolling.scope,
        CacheWindowScope::Rolling {
            lookback_nanoseconds: 43_200_000_000_000
        }
    );

    let anchored = resolve_github_query_window(None, Some("2025-02-01"), None, now).unwrap();
    assert!(matches!(anchored.scope, CacheWindowScope::Anchored { .. }));
    assert!(!anchored.completed);

    let fixed = resolve_github_query_window(
        None,
        Some("2025-01-01"),
        Some("2025-01-31"),
        now,
    )
    .unwrap();
    assert!(matches!(fixed.scope, CacheWindowScope::Fixed { .. }));
    assert!(fixed.completed);

    let (policy, warn) = CachePolicy::from_flags(true, true);
    assert_eq!(policy, CachePolicy::Disabled);
    assert!(warn);
    assert!(!policy.can_read());
    assert!(!policy.can_write());
}
```

In `src/github/api.rs`, add:

```rust
#[test]
fn disabled_cache_policy_never_invokes_cache_factory() {
    let calls = std::cell::Cell::new(0);
    let cache = initialize_cache_for_policy(CachePolicy::Disabled, || {
        calls.set(calls.get() + 1);
        anyhow::bail!("factory must stay unreachable")
    });

    assert!(cache.is_none());
    assert_eq!(calls.get(), 0);
}
```

In `tests/fixture_test.rs`, add:

```rust
#[cfg(feature = "github")]
#[test]
fn github_future_since_is_rejected_before_token_or_network() {
    let output = Command::cargo_bin("logit")
        .unwrap()
        .args(["github", "fetch", "octocat", "--since", "2999-01-01"])
        .env_remove("GITHUB_TOKEN")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("--since") && stderr.contains("future"), "{stderr}");
    assert!(!stderr.contains("GITHUB_TOKEN"), "{stderr}");
}

#[cfg(feature = "github")]
#[test]
fn github_no_cache_refresh_conflict_warns_exactly_once() {
    let output = Command::cargo_bin("logit")
        .unwrap()
        .args([
            "github", "fetch", "octocat", "--since", "2025-01-01",
            "--no-cache", "--refresh-cache",
        ])
        .env_remove("GITHUB_TOKEN")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert_eq!(
        stderr.matches("--no-cache overrides --refresh-cache; cache is disabled").count(),
        1,
        "{stderr}"
    );
}
```

- [ ] **Step 2: Run RED and identify the baseline defects**

Run: `cargo test --locked --all-features positive_fractional_days_round_up_to_one_second -- --nocapture`

Expected: FAIL because `seconds as i64` truncates the positive sub-second duration to zero.

Run: `cargo test --locked --all-features github_query_windows_use_one_clock_and_semantic_scopes -- --nocapture`

Expected: FAIL to compile because the policy/window models and resolver do not exist.

Run: `cargo test --locked --all-features --test fixture_test github_future_since_is_rejected_before_token_or_network -- --nocapture`

Expected: FAIL because baseline reaches the token error instead of rejecting the future start first.

Run: `cargo test --locked --all-features --test fixture_test github_no_cache_refresh_conflict_warns_exactly_once -- --nocapture`

Expected: FAIL because baseline silently combines the two booleans and emits no precedence warning.

- [ ] **Step 3: Implement checked duration and deterministic window resolution**

Use ceiling seconds for every positive accepted value, reject nanosecond-scope overflow, and derive all boundaries from the passed clock:

```rust
fn duration_for_days(days: f64, flag: &str) -> anyhow::Result<chrono::Duration> {
    if !days.is_finite() || days <= 0.0 {
        anyhow::bail!("{flag} must be a finite number greater than zero");
    }
    let seconds = (days * 86_400.0).ceil();
    if !seconds.is_finite() || seconds > i64::MAX as f64 {
        anyhow::bail!("{flag} duration is too large");
    }
    let duration = chrono::Duration::try_seconds(seconds as i64)
        .ok_or_else(|| anyhow::anyhow!("{flag} duration is too large"))?;
    duration
        .num_nanoseconds()
        .ok_or_else(|| anyhow::anyhow!("{flag} duration is too large for cache scope"))?;
    Ok(duration)
}
```

Implement the approved models exactly in `src/github/api.rs` with `Debug + Clone + PartialEq + Eq`; derive `Serialize + Deserialize` on `CacheWindowScope` because Task 4 uses its stable semantic representation. `resolve_github_query_window` applies these decisions in order:

1. Parse/validate before `GithubClient::new`, cache factory, or network.
2. An explicit past/equal `--until` produces an exact completed `Fixed` range; for date-only input, the existing next-midnight exclusive rule remains.
3. Without an elapsed end, `--days` and default 365 days produce `Rolling`; lone `--since` produces `Anchored`; provider end is `observed_at` (an explicit future end is not marked complete).
4. Reject `requested_from > observed_at` as a future `--since`; reject `requested_from >= until_exclusive` as reversed/empty.
5. `cmd_github_multi` builds every named/numeric period from the one `now` captured before iteration.

- [ ] **Step 4: Derive cache policy once and make Disabled structurally incapable of I/O**

Implement:

```rust
impl CachePolicy {
    pub(crate) fn from_flags(no_cache: bool, refresh_cache: bool) -> (Self, bool) {
        if no_cache {
            (Self::Disabled, refresh_cache)
        } else if refresh_cache {
            (Self::Refresh, false)
        } else {
            (Self::ReadOnly, false)
        }
    }

    pub(crate) fn can_read(self) -> bool { !matches!(self, Self::Disabled) }
    pub(crate) fn can_write(self) -> bool { matches!(self, Self::Refresh) }
}
```

`initialize_cache_for_policy` returns immediately for `Disabled`, invokes the factory once for the other policies, and retains existing warning-and-fresh-fetch behavior on factory failure. Emit exactly `Warning: --no-cache overrides --refresh-cache; cache is disabled.` once at the command boundary when `from_flags` returns `warn == true`; remove downstream `read_cache`/`write_cache` booleans as soon as their callers migrate to `CachePolicy`.

- [ ] **Step 5: Run GREEN and regression range tests**

Run: `cargo test --locked --all-features positive_fractional_days_round_up_to_one_second -- --nocapture`

Run: `cargo test --locked --all-features github_query_windows_use_one_clock_and_semantic_scopes -- --nocapture`

Run: `cargo test --locked --all-features disabled_cache_policy_never_invokes_cache_factory -- --nocapture`

Run: `cargo test --locked --all-features reversed_date_range_is_rejected_before_analysis -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test github_future_since_is_rejected_before_token_or_network -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test github_no_cache_refresh_conflict_warns_exactly_once -- --nocapture`

Expected: all PASS; the conflict policy reports `Disabled`, cache factory calls remain zero, fractional metadata still uses existing display-day ceiling, and no token/network/cache work precedes invalid-range errors.

- [ ] **Step 6: Record the phase contribution**

Record changed files and command outputs for the Tasks 1–3 boundary. Do not commit yet; the allowed boundary message after Task 3 is `fix: normalize query windows and local identities`.

### Task 2: Normalize commit-local author/co-author identity and downstream statistics

**Depends on:** none

**Files:**
- Modify: `src/git/author.rs` — canonical identity helpers, extraction normalization, inline tests
- Modify: `src/analyze.rs` — pass primary author through the extraction boundary
- Modify: `src/stats/aggregator.rs` — normalized flat/tree author attribution and tests
- Modify: `src/output/presentation.rs` — lossless author-row merge and tests
- Test: inline tests in all four modules

**Interfaces:**
- Consumes: `Author { name, email }`, commit message co-author trailers, `CommitStats.author/co_authors`, existing `AuthorStats` fields.
- Produces: `canonical_email_key(email: &str) -> String`; `commit_identity_key(author: &Author) -> String`; `normalize_co_authors(primary: &Author, co_authors: impl IntoIterator<Item = Author>) -> Vec<Author>`; changed `extract_co_authors(message: &str, primary: &Author) -> Vec<Author>`; `deduplicated_co_authors<'a>(primary: &Author, co_authors: &'a [Author]) -> Vec<&'a Author>` for defensive aggregation of hand-built/test data.

**Recommended executor:** `coding`

- [ ] **Step 1: Write baseline-failing identity and totals tests**

Add to `src/git/author.rs`:

```rust
#[test]
fn extraction_removes_primary_and_duplicate_coauthors_by_canonical_identity() {
    let primary = Author { name: "Alice".into(), email: " Alice@Example.com ".into() };
    let message = "subject\n\nCo-authored-by: Alias <alice@example.COM>\nCo-authored-by: Bob <BOB@example.com>\nCo-authored-by: Robert <bob@EXAMPLE.com>";
    let coauthors = extract_co_authors(message, &primary);

    assert_eq!(coauthors.len(), 1);
    assert_eq!(coauthors[0].name, "Bob");
    assert_eq!(coauthors[0].email, "BOB@example.com");
    assert_eq!(commit_identity_key(&coauthors[0]), "email:bob@example.com");
}

#[test]
fn empty_email_identity_falls_back_to_trimmed_lowercase_name() {
    let author = Author { name: "  Alice Smith  ".into(), email: "  ".into() };
    assert_eq!(commit_identity_key(&author), "name:alice smith");
}
```

Add to `src/stats/aggregator.rs` using existing `make_commit`/`rust_file` helpers:

```rust
#[test]
fn self_and_duplicate_coauthors_count_once_without_changing_commit_totals() {
    let commit = make_commit(
        "Alice",
        "Alice@Example.com",
        vec![
            Author { name: "Self".into(), email: "alice@example.COM".into() },
            Author { name: "Bob".into(), email: "BOB@example.com".into() },
            Author { name: "Robert".into(), email: "bob@EXAMPLE.com".into() },
        ],
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        vec![rust_file("src/lib.rs", 7, 2)],
    );

    let periods = aggregate_commits(&[commit.clone()], &Period::Month, None, None);
    let totals = aggregate_totals(&periods);
    assert_eq!(totals.total_commits, 1);
    assert_eq!(totals.by_author.len(), 2);
    assert_eq!(totals.by_author["Alice <Alice@Example.com>"].commits, 1);
    assert_eq!(totals.by_author["Bob <BOB@example.com>"].co_authored_commits, 1);

    let tree = build_group_tree(
        &[commit],
        &[GroupBy::Author, GroupBy::Language],
        &Period::Month,
        None,
        None,
    );
    assert_eq!(tree.len(), 2);
    assert!(tree.iter().all(|node| node.stats.total_commits == 1));
}

#[test]
fn same_name_with_different_nonempty_emails_remains_distinct() {
    let a = Author { name: "Alex".into(), email: "one@example.com".into() };
    let b = Author { name: "Alex".into(), email: "two@example.com".into() };
    assert_ne!(commit_identity_key(&a), commit_identity_key(&b));
}
```

Add a presentation regression that seeds nonzero `net_*`, `co_authored_*`, `languages`, and `co_authored_languages` in two same-dedup-key `AuthorStats`, invokes the existing author presentation path, and asserts the merged row has the exact sums. At minimum assert `commits`, `co_authored_commits`, `net_modifications`, `net_additions`, and `co_authored_languages["Rust"].additions`.

- [ ] **Step 2: Run RED**

Run: `cargo test --locked --all-features extraction_removes_primary_and_duplicate_coauthors_by_canonical_identity -- --nocapture`

Expected: FAIL to compile because extraction has no primary argument or canonical helper.

Run: `cargo test --locked --all-features self_and_duplicate_coauthors_count_once_without_changing_commit_totals -- --nocapture`

Expected: FAIL because self and mixed-case duplicate co-authors create duplicate attribution.

Run: `cargo test --locked --all-features presentation_author_merge_preserves_all_primary_and_coauthored_metrics -- --nocapture`

Expected: FAIL because `merge_author_stats` currently omits net and co-authored language fields.

- [ ] **Step 3: Implement one canonical commit identity boundary**

Use these exact semantics:

```rust
pub fn canonical_email_key(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

pub fn commit_identity_key(author: &Author) -> String {
    let email = canonical_email_key(&author.email);
    if email.is_empty() {
        format!("name:{}", author.name.trim().to_ascii_lowercase())
    } else {
        format!("email:{email}")
    }
}
```

`normalize_co_authors` inserts the primary key into a `HashSet`, keeps the first co-author display identity for each newly inserted key, and preserves input order. `extract_co_authors(message, primary)` maps regex captures then calls it. In `analyze_single_repo`, pass `&ci.author`; in aggregation, call `deduplicated_co_authors` again so synthetic/legacy `CommitStats` cannot bypass the invariant.

- [ ] **Step 4: Apply normalized attribution without changing partition totals**

In flat aggregation, increment `PeriodStats.total_commits` once, primary `commits` once, and each normalized distinct co-author's `co_authored_commits` once. In `group_keys`, emit primary first and normalized distinct co-author labels after it; never derive ancestor/global totals by summing overlapping author nodes. Keep same-name/different-email author strings distinct.

Complete `merge_author_stats` by summing every scalar field and merging both maps:

```rust
for (language, stats) in &source.co_authored_languages {
    let entry = target.co_authored_languages.entry(language.clone()).or_default();
    entry.additions += stats.additions;
    entry.deletions += stats.deletions;
    entry.files_changed += stats.files_changed;
    entry.net_modifications += stats.net_modifications;
    entry.net_additions += stats.net_additions;
}
```

Apply the same explicit sums to `target.net_modifications`, `target.co_authored_net_modifications`, `target.net_additions`, and `target.co_authored_net_additions`; do not alter JSON field names or display label formatting.

- [ ] **Step 5: Run GREEN and existing hierarchy regressions**

Run: `cargo test --locked --all-features git::author::tests -- --nocapture`

Run: `cargo test --locked --all-features self_and_duplicate_coauthors_count_once_without_changing_commit_totals -- --nocapture`

Run: `cargo test --locked --all-features same_name_different_email_identities_survive_raw_aggregation -- --nocapture`

Run: `cargo test --locked --all-features group_plan_author_tree_includes_coauthors_without_double_counting_ancestors -- --nocapture`

Run: `cargo test --locked --all-features presentation_author_merge_preserves_all_primary_and_coauthored_metrics -- --nocapture`

Expected: all PASS; one commit remains one root/global commit, distinct co-authors retain overlapping attribution, and presentation dedup loses no metric.

- [ ] **Step 6: Record the phase contribution**

Record `src/git/author.rs`, `src/analyze.rs`, `src/stats/aggregator.rs`, and `src/output/presentation.rs` plus targeted output for the Tasks 1–3 boundary. Do not commit yet.

### Task 3: Unify platform matching and make local filter diagnostics truthful

**Depends on:** Task 1, Task 2

**Files:**
- Modify: `src/analyze.rs` — explicit platform comparison helper and selector/label/collision use
- Modify: `src/exclude.rs` — repository matching through the same helper
- Modify: `src/main.rs` — pre/post-filter count diagnostics
- Modify: `tests/fixture_test.rs` — real CLI diagnostics and Windows regressions
- Test: inline tests in `src/analyze.rs`, `src/exclude.rs`, and `tests/fixture_test.rs`

**Interfaces:**
- Consumes: normalized slash-separated repository IDs/labels/selectors, `cfg!(windows)`, explicit GitHub-provider repository names, commits before/after committer/language filtering.
- Produces: `platform_repo_key(value: &str, windows: bool) -> String`; `platform_repo_eq(left: &str, right: &str, windows: bool) -> bool`; `RepoMatchMode::{LocalPlatform { windows: bool }, Github}`; local production callers use `cfg!(windows)`, while every GitHub exclude/language caller uses `Github`; distinct diagnostics for no commits in range versus commits removed by `--committer`/`--lang`.

**Recommended executor:** `coding`

- [ ] **Step 1: Write baseline-failing comparison and CLI diagnostic tests**

Add pure cross-platform assertions in `src/analyze.rs`:

```rust
#[test]
fn windows_repository_comparison_is_case_insensitive_but_unix_is_not() {
    assert!(platform_repo_eq("Team/Service", "team/service", true));
    assert!(!platform_repo_eq("Team/Service", "team/service", false));
    assert_eq!(
        platform_repo_key("Left/Service", true),
        platform_repo_key("left/service", true)
    );
}
```

Add to `src/exclude.rs`:

```rust
#[test]
fn repository_exclude_uses_explicit_platform_case_policy() {
    let rule = ExcludeRule::parse_many("Team/Service").unwrap().remove(0);
    assert!(rule.matches_repo_with_mode("team/service", RepoMatchMode::LocalPlatform { windows: true }));
    assert!(!rule.matches_repo_with_mode("team/service", RepoMatchMode::LocalPlatform { windows: false }));
    assert!(rule.matches_repo_with_mode("team/service", RepoMatchMode::Github));
}

#[test]
fn github_repository_language_and_repo_excludes_are_case_insensitive_on_every_host() {
    let repo_rules = ExcludeRule::parse_many("Team/Service").unwrap();
    assert!(is_repo_excluded_with_mode("team/service", &repo_rules, RepoMatchMode::Github));
    let lang_rules = ExcludeRule::parse_many("Team/Service:lang:Rust").unwrap();
    assert_eq!(
        excluded_langs_for_repo_with_mode("TEAM/SERVICE", &lang_rules, RepoMatchMode::Github),
        ["Rust"].into_iter().map(str::to_string).collect()
    );
}
```

Add to `tests/fixture_test.rs`:

```rust
#[test]
fn cli_filter_empty_diagnostic_is_not_no_commits_diagnostic() {
    let temp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(temp.path());
    let output = Command::cargo_bin("logit")
        .unwrap()
        .arg("stats")
        .arg(temp.path())
        .args(["--committer", "does-not-exist", "--format", "json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("commits exist") && stderr.contains("filters"), "{stderr}");
    assert!(!stderr.contains("No commits found in the given period"), "{stderr}");
    assert!(output.stdout.is_empty(), "diagnostic-only success must not emit malformed JSON");
}

#[cfg(windows)]
#[test]
fn windows_cli_selector_accepts_case_variant() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("Team").join("Service");
    fs::create_dir_all(&repo_path).unwrap();
    let _repo = common::create_test_repo(&repo_path);
    let json = successful_stats_json(&[temp.path()], &["team/service"]);
    assert_eq!(json["totals"]["total_commits"], 5);
}
```

- [ ] **Step 2: Run RED**

Run: `cargo test --locked --all-features windows_repository_comparison_is_case_insensitive_but_unix_is_not -- --nocapture`

Expected: FAIL to compile because the explicit comparison interface does not exist.

Run: `cargo test --locked --all-features --test fixture_test cli_filter_empty_diagnostic_is_not_no_commits_diagnostic -- --nocapture`

Expected: FAIL because baseline only checks emptiness before committer/language filtering and emits no truthful post-filter diagnostic.

On Windows run: `cargo test --locked --all-features --test fixture_test windows_cli_selector_accepts_case_variant -- --nocapture`

Expected on baseline Windows: FAIL because label/basename selector paths still compare case-sensitively.

- [ ] **Step 3: Route every local repository comparison through one policy**

Implement the pure helpers in `src/analyze.rs` and expose them as `pub(crate)` for `exclude.rs`. Normalize `\` to `/` before keying. Use the key/equality at all of these sites: ID sort/dedup, exact label/ID selector, basename selector, basename collision count, suffix collision, selected-ID set, and local `ExcludeRule` exact/prefix/suffix matching. Preserve original display casing in `RepoInput.label`; only comparison keys are folded. Keep `github/api.rs::repo_key` ASCII-lowercase on every OS.

Give `ExcludeRule` `matches_repo_with_mode(repo_name, mode)` plus mode-aware `is_repo_excluded_with_mode` and `excluded_langs_for_repo_with_mode`. Keep existing local wrappers selecting `LocalPlatform { windows: cfg!(windows) }`. Update every GitHub fetch/card/multi repository-exclude and repository-language call in `src/main.rs` to select `RepoMatchMode::Github`, so provider identity remains case-insensitive even on non-Windows hosts.

- [ ] **Step 4: Distinguish empty stages without changing successful output contracts**

In `cmd_stats`, retain the existing early message when analysis returns zero commits. After `filter_commits_for_stats`, add a second empty check that emits one stderr sentence containing both “commits exist” and “filters”, then returns success without writing partial JSON. Keep `--me`/exclude behavior downstream and do not relabel those outcomes as committer/language failures.

- [ ] **Step 5: Run GREEN and compatibility selectors/excludes**

Run: `cargo test --locked --all-features analyze::tests -- --nocapture`

Run: `cargo test --locked --all-features exclude::tests -- --nocapture`

Run: `cargo test --locked --all-features github_repository_language_and_repo_excludes_are_case_insensitive_on_every_host -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test cli_filter_empty_diagnostic_is_not_no_commits_diagnostic -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test cli_same_basename_repositories_have_distinct_shortest_labels -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test cli_repo_selector_accepts_exact_normalized_identity_and_unique_basename -- --nocapture`

On Windows run: `cargo test --locked --all-features --test fixture_test windows_cli_selector_accepts_case_variant -- --nocapture`

Expected: all applicable tests PASS; non-Windows case sensitivity and original labels remain unchanged.

- [ ] **Step 6: Close the foundation/local phase boundary**

Run: `cargo test --locked --all-features git::author::tests -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test -- --nocapture`

Expected: PASS. Record the exact Tasks 1–3 file set and permit only the message `fix: normalize query windows and local identities` if the orchestrator has explicit Git-write authorization. Do not push or tag.

### Task 4: Introduce v4 semantic keys/envelopes and correct rolling contribution refresh

**Depends on:** Task 1; start after the Tasks 1–3 boundary is recorded

**Files:**
- Modify: `src/github/api.rs` — v4 models, key generation, envelope validation, contribution decision/fetch path, inline tests
- Test: inline tests in `src/github/api.rs`

**Interfaces:**
- Consumes: Task 1 `CachePolicy`, `CacheWindowScope`, `QueryWindow`; user node ID; lower-cased login; `include_forks`, `include_contributed`, `include_private`; contribution payload/summary.
- Produces: `CacheEnvelope<T> { requested_from, checked_until, observed_at, completeness, payload }`; `Completeness::{Complete, Incomplete(Vec<IncompleteReason>)}`; `IncompleteReason::{ContributionRepositoryLimit { limit }, HistoryPageLimit { repository, pages }}`; `CachedContributionPayload { repos, summary }`; stable `contribution_cache_key(user_node_id, username, include_forks, include_contributed, include_private, scope) -> String`; `validate_envelope_bounds`; `ContributionCacheDecision::{Hit, FullFetch}`.

**Recommended executor:** `complex`

- [ ] **Step 1: Write baseline-failing v4 key/envelope/state tests**

Replace the v3-oriented assertions with these contracts:

```rust
#[test]
fn v4_rolling_keys_are_stable_across_observation_time_and_v3_files_naturally_miss() {
    let scope = CacheWindowScope::Rolling { lookback_nanoseconds: 86_400_000_000_000 };
    let first = contribution_cache_key("NODE", "OctoCat", false, true, false, &scope);
    let second = contribution_cache_key("NODE", "octocat", false, true, false, &scope);
    assert_eq!(first, second);
    assert!(first.starts_with("v4_contribution_"));

    let temp = tempfile::tempdir().unwrap();
    let cache = DiskCache::with_dir(temp.path()).unwrap();
    let v3 = first.replacen("v4_", "v3_", 1);
    cache.set(&v3, &serde_json::json!({"legacy": true})).unwrap();
    assert!(cache.get::<CacheEnvelope<CachedContributionPayload>>(&first).unwrap().is_none());
    assert!(temp.path().join(format!("{v3}.json")).exists());
}

#[test]
fn changed_open_contribution_bounds_require_full_refetch_not_gap_merge() {
    let old = QueryWindow::rolling_for_test(
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 1, 8, 0, 0, 0).unwrap(),
    );
    let new = QueryWindow::rolling_for_test(
        Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
    );
    let cached = CacheEnvelope {
        requested_from: old.requested_from,
        checked_until: old.until_exclusive,
        observed_at: old.observed_at,
        completeness: Completeness::Complete,
        payload: CachedContributionPayload::sample_for_test(99),
    };

    assert_eq!(
        contribution_cache_decision(Some(&cached), &new, CachePolicy::Refresh).unwrap(),
        ContributionCacheDecision::FullFetch
    );
}

#[test]
fn incomplete_contribution_never_claims_complete_and_replays_reason() {
    let envelope = CacheEnvelope {
        requested_from: fixed_time("2025-01-01T00:00:00Z"),
        checked_until: fixed_time("2025-01-02T00:00:00Z"),
        observed_at: fixed_time("2025-01-02T00:00:00Z"),
        completeness: Completeness::Incomplete(vec![
            IncompleteReason::ContributionRepositoryLimit { limit: 100 }
        ]),
        payload: CachedContributionPayload::sample_for_test(100),
    };
    assert!(!envelope.completeness.is_complete());
    assert!(envelope.incomplete_warning().unwrap().contains("100"));
}
```

`rolling_for_test`, `fixed_time`, and `sample_for_test` are `#[cfg(test)]` constructors with the exact values shown; they do not call the system clock.

- [ ] **Step 2: Add a baseline-failing TCP assertion for exact full refresh**

Extend the existing stub response to accept owned bodies/headers through a `json_response(status, body)` helper, then add:

```rust
#[test]
fn rolling_contribution_refresh_requests_the_exact_new_full_window() {
    let old = QueryWindow::rolling_for_test(fixed_time("2025-01-01T00:00:00Z"), fixed_time("2025-01-08T00:00:00Z"));
    let new = QueryWindow::rolling_for_test(fixed_time("2025-01-02T00:00:00Z"), fixed_time("2025-01-09T00:00:00Z"));
    let temp = tempfile::tempdir().unwrap();
    let cache = DiskCache::with_dir(temp.path()).unwrap();
    seed_contribution_envelope(&cache, "NODE", "octocat", &old, 99);
    let response = one_contribution_response("octocat", "new-repo", 3);
    let server = start_stub(vec![json_response(200, response.clone())]);
    let client = GithubClient::for_test(&server.base_url, vec![], Duration::from_secs(1));

    let refreshed = get_contribution_repos_cached(
        &client, &cache, "NODE", "octocat", false, false, false,
        &new, CachePolicy::Refresh, &mut CacheWarnings::default(),
    )
    .unwrap();
    assert_eq!(refreshed.0.len(), 1);
    assert_eq!(refreshed.0[0].0.name, "new-repo");
    assert_eq!(refreshed.0[0].1, 3, "expired cached aggregate must not survive");
    let request = server.finish().pop().unwrap();
    let variables = graphql_variables(&request);
    assert_eq!(variables["from"], new.requested_from.to_rfc3339());
    assert_eq!(
        DateTime::parse_from_rfc3339(variables["to"].as_str().unwrap()).unwrap()
            + chrono::Duration::nanoseconds(1),
        new.until_exclusive
    );

    let fresh_server = start_stub(vec![json_response(200, response)]);
    let fresh_client = GithubClient::for_test(&fresh_server.base_url, vec![], Duration::from_secs(1));
    let fresh = fetch_contributions_without_cache(
        &fresh_client, "octocat", false, false, &new,
    )
    .unwrap();
    assert_eq!(contribution_fingerprint(&refreshed), contribution_fingerprint(&fresh));
    assert_eq!(fresh_server.finish().len(), 1);
}
```

`one_contribution_response` returns a valid GraphQL payload with zero PR/review/issue summary counters and one `commitContributionsByRepository` node whose owner/name/count are the supplied values. `fetch_contributions_without_cache` calls the production provider full-fetch path directly with the same `QueryWindow`; `contribution_fingerprint` returns sorted `(lower_owner, lower_name, count, total_prs, total_reviews, total_issues)` tuples. The critical assertions prove both that `from` equals the new left edge—not old `checked_until`—and that the refreshed result equals a fresh no-cache fetch without the expired count `99`.

- [ ] **Step 3: Run RED**

Run: `cargo test --locked --all-features v4_rolling_keys_are_stable_across_observation_time_and_v3_files_naturally_miss -- --nocapture`

Expected: FAIL because baseline keys are v3 and encode moving timestamps.

Run: `cargo test --locked --all-features rolling_contribution_refresh_requests_the_exact_new_full_window -- --nocapture`

Expected: FAIL because the v4 envelope/state decision and new signature do not exist.

- [ ] **Step 4: Implement v4 serialization, semantic keys, and validation**

Set `CACHE_SCHEMA_VERSION` to `v4`. Encode every key component with the existing collision-free byte encoding. Lowercase login and owner/repository identity before encoding. Scope encoding is explicit and stable:

```rust
fn cache_scope_component(scope: &CacheWindowScope) -> String {
    match scope {
        CacheWindowScope::Rolling { lookback_nanoseconds } => format!("rolling_ns_{lookback_nanoseconds}"),
        CacheWindowScope::Anchored { from } => format!("anchored_{}_open", cache_string_component(&from.to_rfc3339())),
        CacheWindowScope::Fixed { from, until_exclusive } => format!(
            "fixed_{}_{}",
            cache_string_component(&from.to_rfc3339()),
            cache_string_component(&until_exclusive.to_rfc3339())
        ),
    }
}
```

All v4 coverage fields are required serde fields. A missing field therefore enters existing parse-warning/miss handling. `validate_envelope_bounds` rejects requested start after checked end, checked end after observation, clock rollback relative to the current open window, payload timestamps outside coverage, and incompatible fixed/open ranges.

- [ ] **Step 5: Replace contribution cache behavior with exact-envelope decisions**

Implement only `Hit` or `FullFetch`; no aggregate gap-merge variant exists. Rules:

- Complete fixed envelope with exact fixed scope is reusable indefinitely under `ReadOnly` or `Refresh`.
- Exact-bound open envelope may be read; an incomplete exact-bound envelope replays its warning and is never used to infer inactivity.
- Any rolling/anchored boundary change performs the complete provider query for `[new.requested_from, new.until_exclusive)`.
- `Refresh` writes the fresh envelope. A repository count of 100 writes `Incomplete(ContributionRepositoryLimit { limit: 100 })` and returns usable data with a deduplicated stderr warning.
- Multi-window provider splitting remains adjacent/non-overlapping; envelope describes the exact overall requested range, not an individual internal API shard.

- [ ] **Step 6: Run GREEN and contribution compatibility tests**

Run: `cargo test --locked --all-features v4_rolling_keys_are_stable_across_observation_time_and_v3_files_naturally_miss -- --nocapture`

Run: `cargo test --locked --all-features changed_open_contribution_bounds_require_full_refetch_not_gap_merge -- --nocapture`

Run: `cargo test --locked --all-features rolling_contribution_refresh_requests_the_exact_new_full_window -- --nocapture`

Run: `cargo test --locked --all-features contribution_queries_make_adjacent_half_open_windows_non_overlapping -- --nocapture`

Run: `cargo test --locked --all-features incomplete_contribution_never_claims_complete_and_replays_reason -- --nocapture`

Expected: all PASS; the v3 file still exists, stable rolling keys match, changed open bounds issue one full-range query, and saturation is visibly incomplete.

- [ ] **Step 7: Record the task boundary**

Record `src/github/api.rs`, tests, and targeted outputs. This is part of the later GitHub phase message `fix: repair GitHub cache retry and identity flows`; do not commit at this task boundary.

### Task 5: Implement the v4 rolling history coverage state machine

**Depends on:** Task 4

**Files:**
- Modify: `src/github/api.rs` — history semantic key, refresh plan/merge, envelope validation, batching integration, inline tests
- Test: inline tests in `src/github/api.rs`

**Interfaces:**
- Consumes: Task 4 `CacheEnvelope<Vec<CommitData>>`, `Completeness`, Task 1 `QueryWindow/CachePolicy`, lower-cased owner/repository, `include_private`, existing `RepoHistoryRequest` and batched history result/cap set.
- Produces: stable `history_cache_key(user_node_id, owner, repo, include_private, scope) -> String`; `HistoryFetchPlan::{Hit { commits }, Full { request }, Gap { retained, request }}`; `plan_history_refresh(cached, window, policy, owner, repo) -> anyhow::Result<HistoryFetchPlan>`; `finish_history_fetch(plan, fetched, window, completeness) -> CacheEnvelope<Vec<CommitData>>`.

**Recommended executor:** `complex`

- [ ] **Step 1: Write baseline-failing pure state-machine tests**

```rust
#[test]
fn rolling_history_refresh_starts_at_checked_until_and_trims_left_edge() {
    let window = QueryWindow::rolling_for_test(
        fixed_time("2025-01-02T00:00:00Z"),
        fixed_time("2025-01-11T00:00:00Z"),
    );
    let cached = CacheEnvelope {
        requested_from: fixed_time("2025-01-01T00:00:00Z"),
        checked_until: fixed_time("2025-01-10T00:00:00Z"),
        observed_at: fixed_time("2025-01-10T00:00:00Z"),
        completeness: Completeness::Complete,
        payload: vec![
            commit("expired", "2025-01-01T12:00:00Z", 1),
            commit("kept", "2025-01-05T12:00:00Z", 2),
        ],
    };

    let HistoryFetchPlan::Gap { retained, request } = plan_history_refresh(
        Some(cached), &window, CachePolicy::Refresh, "octocat", "repo"
    ).unwrap() else { panic!("expected a right-edge gap") };
    assert_eq!(retained.iter().map(|c| c.oid.as_deref()).collect::<Vec<_>>(), [Some("kept")]);
    assert_eq!(request.since.as_deref(), Some("2025-01-10T00:00:00+00:00"));
    assert_eq!(request.until_exclusive.as_deref(), Some("2025-01-11T00:00:00+00:00"));
}

#[test]
fn empty_successful_history_gap_advances_checked_until() {
    let window = QueryWindow::rolling_for_test(
        fixed_time("2025-01-02T00:00:00Z"),
        fixed_time("2025-01-11T00:00:00Z"),
    );
    let plan = HistoryFetchPlan::Gap {
        retained: vec![commit("kept", "2025-01-05T12:00:00Z", 2)],
        request: history_request("octocat", "repo", "2025-01-10T00:00:00Z", "2025-01-11T00:00:00Z"),
    };
    let envelope = finish_history_fetch(plan, vec![], &window, Completeness::Complete).unwrap();
    assert_eq!(envelope.checked_until, window.until_exclusive);
    assert_eq!(envelope.payload.len(), 1);
    assert_eq!(envelope.payload[0].oid.as_deref(), Some("kept"));
}

#[test]
fn readonly_incomplete_history_is_a_full_fetch_without_cache_write() {
    let window = QueryWindow::rolling_for_test(
        fixed_time("2025-01-02T00:00:00Z"),
        fixed_time("2025-01-11T00:00:00Z"),
    );
    let cached = CacheEnvelope {
        requested_from: window.requested_from,
        checked_until: window.until_exclusive,
        observed_at: window.observed_at,
        completeness: Completeness::Incomplete(vec![
            IncompleteReason::HistoryPageLimit { repository: "octocat/repo".into(), pages: 20 }
        ]),
        payload: vec![commit("partial", "2025-01-05T12:00:00Z", 1)],
    };
    let plan = plan_history_refresh(
        Some(cached), &window, CachePolicy::ReadOnly, "octocat", "repo"
    ).unwrap();
    assert!(matches!(plan, HistoryFetchPlan::Full { .. }));
    assert!(!CachePolicy::ReadOnly.can_write());
}

#[test]
fn history_dedup_only_collapses_nonempty_oids() {
    let commits = dedup_commits(vec![
        commit("same", "2025-01-05T00:00:00Z", 1),
        commit("same", "2025-01-05T00:00:00Z", 1),
        commit_with_oid(None, "2025-01-06T00:00:00Z", 2),
        commit_with_oid(None, "2025-01-06T00:00:00Z", 2),
    ]);
    assert_eq!(commits.len(), 3);
    assert_eq!(commits.iter().filter(|c| c.oid.is_none()).count(), 2);
}
```

Also add `malformed_incomplete_or_clock_rollback_history_plans_full_fetch` with three envelopes and assert each yields `HistoryFetchPlan::Full` plus one cache warning, and `capped_history_envelope_is_incomplete_not_complete` asserting `HistoryPageLimit` survives serialization round-trip.

- [ ] **Step 2: Add an end-to-end local-stub gap equivalence test**

Use `TempDir`, `DiskCache::with_dir`, explicit `QueryWindow`s, and the existing TCP history response format:

```rust
#[test]
fn second_rolling_history_refresh_fetches_only_gap_and_matches_fresh_result() {
    let first_window = QueryWindow::rolling_for_test(fixed_time("2025-01-01T00:00:00Z"), fixed_time("2025-01-08T00:00:00Z"));
    let second_window = QueryWindow::rolling_for_test(fixed_time("2025-01-02T00:00:00Z"), fixed_time("2025-01-09T00:00:00Z"));
    let temp = tempfile::tempdir().unwrap();
    let cache = DiskCache::with_dir(temp.path()).unwrap();
    seed_complete_history(&cache, "NODE", "octocat", "repo", &first_window,
        vec![commit("old", "2025-01-01T12:00:00Z", 1), commit("kept", "2025-01-05T12:00:00Z", 2)]);
    let server = start_stub(vec![json_response(200, history_response(vec![
        commit("new", "2025-01-08T12:00:00Z", 3)
    ]))]);
    let client = GithubClient::for_test(&server.base_url, vec![], Duration::from_secs(1));

    let refreshed = refresh_one_history(&client, &cache, "NODE", "octocat", "repo", &second_window).unwrap();
    let requests = server.finish();
    let variables = graphql_variables(&requests[0]);
    assert_eq!(variables["since0"], "2025-01-08T00:00:00+00:00");
    assert_eq!(variables["until0"], "2025-01-09T00:00:00+00:00");
    assert_eq!(refreshed.iter().map(|c| c.oid.as_deref()).collect::<Vec<_>>(), [Some("kept"), Some("new")]);

    let fresh_server = start_stub(vec![json_response(200, history_response(vec![
        commit("kept", "2025-01-05T12:00:00Z", 2),
        commit("new", "2025-01-08T12:00:00Z", 3),
    ]))]);
    let fresh_client = GithubClient::for_test(&fresh_server.base_url, vec![], Duration::from_secs(1));
    let fresh = fetch_one_history_without_cache(
        &fresh_client, "NODE", "octocat", "repo", &second_window,
    )
    .unwrap();
    assert_eq!(refreshed, fresh);
    let fresh_requests = fresh_server.finish();
    assert_eq!(fresh_requests.len(), 1);
    let fresh_variables = graphql_variables(&fresh_requests[0]);
    assert_eq!(fresh_variables["since0"], "2025-01-02T00:00:00+00:00");
    assert_eq!(fresh_variables["until0"], "2025-01-09T00:00:00+00:00");
}
```

Derive `PartialEq + Eq + Debug` for `CommitData` to support exact comparison. `refresh_one_history`, `fetch_one_history_without_cache`, `seed_complete_history`, `history_response`, and `graphql_variables` are test-only adapters around production state-machine/provider interfaces; the first records the right-edge gap, the second requests the exact complete no-cache window, and no network leaves loopback.

- [ ] **Step 3: Run RED**

Run: `cargo test --locked --all-features rolling_history_refresh_starts_at_checked_until_and_trims_left_edge -- --nocapture`

Expected: FAIL because baseline keys bind moving bounds and gap start still derives from the latest commit timestamp.

Run: `cargo test --locked --all-features second_rolling_history_refresh_fetches_only_gap_and_matches_fresh_result -- --nocapture`

Expected: FAIL because the stable envelope/state-machine helpers do not exist.

- [ ] **Step 4: Implement stable history scope and refresh planning**

History keys include v4, user node ID, lower-cased owner/name, `include_private`, and Task 4 scope component—never moving rolling timestamps. Replace `CachedCommitHistory { since, until, checked_until, commits }` with `CacheEnvelope<Vec<CommitData>>`.

`plan_history_refresh` applies this order:

1. Disabled has no cache object; a miss, malformed envelope, incomplete coverage under either readable policy, incompatible scope/range, or clock rollback yields `Full` with a visible warning where applicable. `ReadOnly` performs the network fetch but does not write its result.
2. A complete envelope covering the exact fixed range yields `Hit`.
3. For rolling/anchored `ReadOnly` or `Refresh`, retain only commits in the new range. If `checked_until < window.until_exclusive`, yield `Gap` whose `request.since` is exactly `checked_until`; do not use latest commit date. Both policies fetch the gap for a current result, but only `Refresh` writes the completed envelope.
4. If checked coverage already reaches the end, yield `Hit` with trimmed payload.
5. `ReadOnly` may replay only an exact-range complete payload. Incomplete payloads are never returned as cache hits and never prove inactivity.

- [ ] **Step 5: Merge gaps and preserve partial state**

`finish_history_fetch` filters fetched commits to the requested half-open gap, merges them with retained payload, deduplicates only non-empty OIDs, trims to the current full window, and sets `checked_until = window.until_exclusive` even for an empty successful gap. When `batch_commit_history` reports a cap, return usable data with `Completeness::Incomplete(HistoryPageLimit { repository, pages: 20 })`; never serialize it as `Complete`. Preserve batching in chunks of five and the saturation guard that prevents an incomplete contribution list from proving a repository inactive.

- [ ] **Step 6: Run GREEN and legacy validation regressions**

Run: `cargo test --locked --all-features rolling_history_refresh_starts_at_checked_until_and_trims_left_edge -- --nocapture`

Run: `cargo test --locked --all-features empty_successful_history_gap_advances_checked_until -- --nocapture`

Run: `cargo test --locked --all-features readonly_incomplete_history_is_a_full_fetch_without_cache_write -- --nocapture`

Run: `cargo test --locked --all-features second_rolling_history_refresh_fetches_only_gap_and_matches_fresh_result -- --nocapture`

Run: `cargo test --locked --all-features malformed_incomplete_or_clock_rollback_history_plans_full_fetch -- --nocapture`

Run: `cargo test --locked --all-features capped_history_envelope_is_incomplete_not_complete -- --nocapture`

Run: `cargo test --locked --all-features batch_history_uses_and_filters_each_repository_window -- --nocapture`

Expected: all PASS; second refresh records one right-edge request, fresh/refreshed payloads match, empty gaps advance, and caps cannot masquerade as complete.

- [ ] **Step 7: Record the task boundary**

Record the current `src/github/api.rs` diff and all targeted outputs. Keep the GitHub phase uncommitted until Task 7.

### Task 6: Make retry classification pure and identity discovery bounded/visible

**Depends on:** Task 5

**Files:**
- Modify: `src/github/api.rs` — pure retry decision, header-aware TCP helper, bounded identity report/resolver, inline tests
- Test: inline tests in `src/github/api.rs`

**Interfaces:**
- Consumes: HTTP status/headers, explicit `now`, configured fallback delay, existing retry attempt count/timeout, one GraphQL repository page, REST commit responses and `Link` headers.
- Produces: `RetryDecision::{Return, RetryAfter(Duration), Fail(String)}`; `retry_decision(status, headers, fallback, now, max_server_wait) -> RetryDecision`; `IdentityLookupFailure { repository: Option<String>, message: String }`; approved `IdentityResolutionReport`; `IdentityResolutionReport::{is_partial, warning}`; `GithubClient::resolve_user_identity(login: &str) -> IdentityResolutionReport`.

**Recommended executor:** `complex`

- [ ] **Step 1: Write baseline-failing pure retry tests**

```rust
#[test]
fn retry_decision_honors_retry_after_seconds_and_http_date_before_reset() {
    let now = fixed_time("2025-01-01T00:00:00Z");
    let mut seconds = HeaderMap::new();
    seconds.insert("retry-after", HeaderValue::from_static("7"));
    seconds.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
    seconds.insert("x-ratelimit-reset", HeaderValue::from_static("1735689660"));
    assert_eq!(
        retry_decision(StatusCode::TOO_MANY_REQUESTS, &seconds, Duration::from_secs(2), now, Duration::from_secs(120)),
        RetryDecision::RetryAfter(Duration::from_secs(7))
    );

    let mut date = HeaderMap::new();
    date.insert("retry-after", HeaderValue::from_static("Wed, 01 Jan 2025 00:00:09 GMT"));
    assert_eq!(
        retry_decision(StatusCode::TOO_MANY_REQUESTS, &date, Duration::ZERO, now, Duration::from_secs(120)),
        RetryDecision::RetryAfter(Duration::from_secs(9))
    );
}

#[test]
fn permission_403_fails_once_while_exhausted_403_uses_reset() {
    let now = fixed_time("2025-01-01T00:00:00Z");
    assert!(matches!(
        retry_decision(StatusCode::FORBIDDEN, &HeaderMap::new(), Duration::ZERO, now, Duration::from_secs(120)),
        RetryDecision::Fail(message) if message.contains("permission")
    ));

    let mut exhausted = HeaderMap::new();
    exhausted.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
    exhausted.insert("x-ratelimit-reset", HeaderValue::from_static("1735689605"));
    assert_eq!(
        retry_decision(StatusCode::FORBIDDEN, &exhausted, Duration::ZERO, now, Duration::from_secs(120)),
        RetryDecision::RetryAfter(Duration::from_secs(5))
    );
}

#[test]
fn server_delay_beyond_bound_fails_visibly_instead_of_retrying_early() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("121"));
    assert!(matches!(
        retry_decision(StatusCode::TOO_MANY_REQUESTS, &headers, Duration::ZERO, fixed_time("2025-01-01T00:00:00Z"), Duration::from_secs(120)),
        RetryDecision::Fail(message) if message.contains("121") && message.contains("120")
    ));
}
```

- [ ] **Step 2: Write bounded partial identity tests with the TCP stub**

Change test-only `StubResponse::Json` to own `body: String` and `headers: Vec<(String, String)>`; make `json_response` provide empty headers so existing tests retain behavior. Add:

```rust
#[test]
fn identity_report_is_bounded_canonical_and_partial_when_provider_has_more() {
    let repos = serde_json::json!({
        "data": {
            "rateLimit": {"cost": 1, "remaining": 100, "resetAt": "2025-01-02T00:00:00Z"},
            "user": {"repositories": {
                "pageInfo": {"hasNextPage": true, "endCursor": "next"},
                "nodes": [{
                    "name": "repo", "owner": {"login": "OctoCat"}, "isFork": false,
                    "languages": {"edges": []}
                }]
            }}
        }
    });
    let commits = serde_json::Value::Array((0..20).map(|_| serde_json::json!({
        "commit": {"author": {"email": " Mixed@Example.COM "}}
    })).collect());
    let server = start_stub(vec![
        json_response(200, repos.to_string()),
        json_response_with_headers(200, commits.to_string(), vec![(
            "Link".into(), "<http://example.invalid/page=2>; rel=\"next\"".into()
        )]),
    ]);
    let client = GithubClient::for_test(&server.base_url, vec![], Duration::from_secs(1));
    let report = client.resolve_user_identity("OctoCat");

    assert_eq!(report.login, "octocat");
    assert_eq!(report.emails, std::collections::BTreeSet::from(["mixed@example.com".into()]));
    assert_eq!(report.repositories_examined, 1);
    assert_eq!(report.logical_requests, 2);
    assert!(report.truncated_repositories);
    assert!(report.truncated_commits);
    assert!(report.failures.is_empty());
    assert!(report.warning().unwrap().contains("known emails"));
    assert_eq!(server.finish().len(), 2);
}

#[test]
fn identity_request_failure_returns_known_partial_report_not_false_complete() {
    let server = start_stub(vec![
        json_response(200, one_identity_repo_response()),
        json_response(403, "{}"),
    ]);
    let client = GithubClient::for_test(&server.base_url, vec![Duration::ZERO; 2], Duration::from_secs(1));
    let report = client.resolve_user_identity("octocat");
    assert!(report.is_partial());
    assert_eq!(report.failures.len(), 1);
    assert_eq!(server.finish().len(), 2, "permission 403 must not retry");
}
```

- [ ] **Step 3: Run RED**

Run: `cargo test --locked --all-features retry_decision_honors_retry_after_seconds_and_http_date_before_reset -- --nocapture`

Expected: FAIL because retry classification is embedded in `send_with_retry` and HTTP-date is unsupported.

Run: `cargo test --locked --all-features permission_403_fails_once_while_exhausted_403_uses_reset -- --nocapture`

Expected: FAIL because baseline treats reset-bearing 403 without checking remaining budget.

Run: `cargo test --locked --all-features identity_report_is_bounded_canonical_and_partial_when_provider_has_more -- --nocapture`

Expected: FAIL because `resolve_user_emails` returns only `Vec<String>` and paginates the repository listing instead of reporting the one-page bound.

- [ ] **Step 4: Implement pure retry precedence without changing attempt/timeout limits**

Parse `Retry-After` as unsigned delta seconds first, then RFC 2822/HTTP-date with `DateTime::parse_from_rfc2822` and the explicit `now`. Parse reset epoch only when `X-RateLimit-Remaining == 0`. Apply:

- 429: valid `Retry-After`; else exhausted+valid reset; else configured fallback.
- 403: valid `Retry-After`; else exhausted+valid reset; else `Fail` containing “permission”.
- 408 and 5xx: configured fallback.
- Other success/client statuses: existing return/fail behavior.
- Any server-derived wait above 120 seconds: visible `Fail`; never clamp and retry early.

`send_with_retry` obtains the decision before consuming the response, sleeps only for `RetryAfter`, and retains `retry_delays.len() + 1`, transport retry categories, timeout, and final-status diagnostics.

- [ ] **Step 5: Implement bounded identity reporting**

Define the spec model exactly:

```rust
pub struct IdentityResolutionReport {
    pub login: String,
    pub emails: BTreeSet<String>,
    pub repositories_examined: usize,
    pub logical_requests: usize,
    pub truncated_repositories: bool,
    pub truncated_commits: bool,
    pub failures: Vec<IdentityLookupFailure>,
}
```

Fetch exactly one owned-repository GraphQL page. Mark repository truncation for `hasNextPage` or more than eight eligible non-fork repos; inspect only the first eight in deterministic provider order. Request at most 20 commits per repository, canonicalize nonempty emails with `canonical_email_key`, and mark commit truncation for REST `Link` containing `rel="next"`. Increment `logical_requests` once per attempted GraphQL/REST operation; capture parse/request/status failures with repository context and continue. `is_partial` is true for either truncation flag or any failure. `warning()` returns one actionable sentence saying known emails are used and other emails may be missed; no internal request prints a second per-login partial warning. Task 7 applies a command-level maximum of eight distinct sorted logins; later logins are not requested and contribute to one command-level partial warning.

Keep the existing `resolve_user_emails` signature as a temporary thin adapter that
calls `resolve_user_identity` and returns the report's known emails until Task 7
migrates every `main.rs` caller. The adapter preserves compilation between Tasks 6
and 7 and is removed only after those call sites consume reports directly.

- [ ] **Step 6: Run GREEN and existing bounded retry tests**

Run: `cargo test --locked --all-features retry_decision_ -- --nocapture`

Run: `cargo test --locked --all-features permission_403_fails_once_while_exhausted_403_uses_reset -- --nocapture`

Run: `cargo test --locked --all-features identity_report_ -- --nocapture`

Run: `cargo test --locked --all-features graphql_retries_408_429_and_5xx_then_succeeds_with_a_bound -- --nocapture`

Run: `cargo test --locked --all-features graphql_transient_transport_error_retries_then_succeeds -- --nocapture`

Run: `cargo test --locked --all-features rest_author_query_is_url_encoded -- --nocapture`

Expected: all PASS; HTTP-date and seconds work, permission 403 records one request, provider partial state is visible, and existing transport/timeout bounds remain.

- [ ] **Step 7: Record the task boundary**

Record `src/github/api.rs` and targeted output. Keep the GitHub phase open for command-level identity integration in Task 7.

### Task 7: Share local GitHub-login resolution once and deduplicate across all selected origins

**Depends on:** Task 3, Task 6

**Files:**
- Modify: `src/filter.rs` — collect login atoms and canonical identity-map lookup
- Modify: `src/exclude.rs` — canonical/case-insensitive report application
- Modify: `src/main.rs` — pre-resolution parse, shared index, no-token/partial warnings, all-origin remote dedup
- Modify: `src/github/api.rs` — expose report methods/client identity call at crate boundary
- Modify: `tests/fixture_test.rs` — real local noreply `--me github` behavior
- Test: inline tests in `src/filter.rs`, `src/exclude.rs`, `src/main.rs`; `tests/fixture_test.rs`

**Interfaces:**
- Consumes: Task 6 `IdentityResolutionReport`, parsed `MeExpr`, parsed `ExcludeRule`s, optional token-backed `GithubClient`, selected `RepoInput`s, commits keyed by `repo_id`, each repo's `origin`.
- Produces: `MeExpr::github_logins(&self) -> BTreeSet<String>`; `collect_requested_github_logins(me, rules) -> BTreeSet<String>`; `CommandIdentityResolution { reports: BTreeMap<String, IdentityResolutionReport>, skipped_logins: usize }`; `CommandIdentityResolution::warning() -> Option<String>`; `resolve_identity_reports(logins, resolver) -> CommandIdentityResolution`; canonical email→login map; `RemoteIdentityReport { mappings: HashMap<String, String>, warnings: Vec<String> }`; `build_remote_identity_map_with(repos, commits, resolver) -> RemoteIdentityReport` test seam.

**Recommended executor:** `complex`

- [ ] **Step 1: Write baseline-failing shared-index and mixed-case tests**

Add to `src/filter.rs`:

```rust
#[test]
fn github_identity_map_lookup_canonicalizes_mixed_case_email() {
    let expr = parse_me_expr("github:OctoCat").unwrap();
    let commit = make_commit("Octo", " Mixed@Example.COM ");
    let map = HashMap::from([("mixed@example.com".into(), "octocat".into())]);
    assert!(expr.matches_commit(&commit, &map));
}

#[test]
fn github_logins_are_collected_case_insensitively() {
    let expr = parse_me_expr("github:OctoCat|github:octocat&name:Octo").unwrap();
    assert_eq!(expr.github_logins(), BTreeSet::from(["octocat".into()]));
}
```

Add to `src/main.rs`:

```rust
#[test]
fn shared_me_and_exclude_login_is_resolved_once_per_command() {
    let me = filter::parse_me_expr("github:OctoCat").unwrap();
    let rules = exclude::ExcludeRule::parse_many(":author:@octocat").unwrap();
    let logins = collect_requested_github_logins(Some(&me), &rules);
    let calls = std::cell::Cell::new(0);
    let resolution = resolve_identity_reports(logins, |login| {
        calls.set(calls.get() + 1);
        IdentityResolutionReport::complete_for_test(login, [" Mixed@Example.COM "])
    });

    assert_eq!(calls.get(), 1);
    assert_eq!(resolution.reports.len(), 1);
    assert_eq!(resolution.reports["octocat"].emails, BTreeSet::from(["mixed@example.com".into()]));
    assert!(resolution.warning().is_none());
}
```

Add `requested_login_budget_is_bounded_and_visible`: provide ten distinct mixed-case
logins, assert only the first eight sorted canonical logins reach the resolver, and
assert one command-level warning names the skipped count without printing to stdout.
Add `multiple_partial_identity_reports_emit_one_command_warning`: return two partial
reports plus two skipped-over-budget logins, feed them through the command warning
collector, and assert stderr contains the single warning prefix exactly once while
captured stdout remains empty.

- [ ] **Step 2: Write an all-selected-origins fake-resolver regression**

In `src/main.rs` tests, define this local fixture and test:

```rust
fn repo_with_origin(root: &Path, id: &str, origin: &str) -> analyze::RepoInput {
    let path = root.join(id);
    std::fs::create_dir_all(&path).unwrap();
    let repo = git2::Repository::init(&path).unwrap();
    repo.remote("origin", origin).unwrap();
    analyze::RepoInput { path, id: id.into(), label: id.into() }
}

#[test]
fn remote_dedup_attempts_all_selected_origins_and_uses_later_success() {
    let temp = tempfile::tempdir().unwrap();
    let repos = vec![
        repo_with_origin(temp.path(), "a", "https://github.com/one/first.git"),
        repo_with_origin(temp.path(), "b", "https://github.com/two/second.git"),
    ];
    let commits = vec![
        commit_for_repo("a", "Same@Example.com"),
        commit_for_repo("b", "same@example.COM"),
    ];
    let calls = std::cell::RefCell::new(Vec::new());
    let report = build_remote_identity_map_with(&repos, &commits, |owner, repo, emails| {
        calls.borrow_mut().push((owner.to_string(), repo.to_string(), emails.to_vec()));
        if owner == "two" {
            HashMap::from([("same@example.com".into(), "octocat".into())])
        } else {
            HashMap::new()
        }
    });

    assert_eq!(calls.borrow().iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["one", "two"]);
    assert_eq!(report.mappings["same@example.com"], "octocat");
}
```

`commit_for_repo` constructs a complete `CommitStats` with the given `repo_id`, primary email, empty co-authors/files, fixed timestamp, and nonempty OID. Add a companion `remote_dedup_first_success_wins_and_warns_on_conflicting_later_login` asserting the first mapping remains and exactly one warning names both logins.

- [ ] **Step 3: Add a real local noreply CLI regression without network**

In `tests/fixture_test.rs`, create a one-commit local Git repo whose author email is `123+OctoCat@users.noreply.github.com`, invoke:

```rust
#[test]
fn cli_me_github_noreply_matches_without_token_or_network() {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().join("identity-repo");
    create_single_commit_repo(
        &repo_path,
        "Octo Cat",
        "123+OctoCat@users.noreply.github.com",
    );
    let output = Command::cargo_bin("logit")
        .unwrap()
        .arg("stats")
        .arg(&repo_path)
        .args(["--me", "github:octocat", "--format", "json"])
        .env_remove("GITHUB_TOKEN")
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(json["totals"]["total_commits"], 1);
}
```

`create_single_commit_repo(path, name, email)` uses `git2::Repository::init`, one fixed `Signature`, one blob/tree, and `repo.commit(Some("HEAD"), ...)`, matching the existing fixture construction style. This path must not make a provider request: noreply matching is local even when the same command has no token.

- [ ] **Step 4: Run RED**

Run: `cargo test --locked --all-features github_identity_map_lookup_canonicalizes_mixed_case_email -- --nocapture`

Expected: FAIL because baseline looks up the unnormalized raw email.

Run: `cargo test --locked --all-features shared_me_and_exclude_login_is_resolved_once_per_command -- --nocapture`

Expected: FAIL because `--me` does not trigger resolver discovery and exclude resolution owns a separate loop.

Run: `cargo test --locked --all-features remote_dedup_attempts_all_selected_origins_and_uses_later_success -- --nocapture`

Expected: FAIL because baseline stops after the first selected GitHub origin.

- [ ] **Step 5: Parse first, resolve once, and apply known partial results**

Move `parse_me_expr` and exclude parsing before any client/cache/network creation. Collect lower-cased distinct logins from both structures into a sorted `BTreeSet`. Resolve only the first eight sorted logins, record the skipped count as partial, create at most one optional `GithubClient` for command-level identity needs, then call the injected/real resolver once per admitted login and store reports by lower-cased login.

Apply every report without printing per-report warnings:

- Insert each canonical email→lower-cased login into the `--me` identity map.
- Apply canonical emails to all matching exclude author/committer groups using case-insensitive login comparison.
- Aggregate every partial/truncation/failure reason plus the skipped-login count into
  `CommandIdentityResolution::warning()` and emit that command-level warning exactly
  once after all reports are applied.
- On no token/client failure, emit one actionable warning and continue with local noreply matching; do not claim completeness or fail an otherwise successful local command.

Update `is_github_match` to call `canonical_email_key(email)` before map lookup. Keep successful JSON stdout warning-free.

- [ ] **Step 6: Iterate every selected origin independently**

Replace first-origin selection with deterministic `repos` iteration sorted by the Task 3 comparison key. For each selected GitHub `origin`:

1. Collect canonical primary/co-author emails only from commits whose `commit.repo_id == repo.id`.
2. Skip already successfully mapped emails for request efficiency, but attempt unresolved emails on later origins.
3. Resolve against that origin only and merge successful mappings first-wins.
4. If a later success conflicts with an earlier login, retain the earlier mapping and append one deterministic warning.
5. A failed origin leaves raw identities separate and does not stop later origins.

Keep GitHub owner/name comparison case-insensitive on every OS; do not share this process with username-to-email discovery.

- [ ] **Step 7: Run GREEN and real local surface tests**

Run: `cargo test --locked --all-features filter::tests -- --nocapture`

Run: `cargo test --locked --all-features shared_me_and_exclude_login_is_resolved_once_per_command -- --nocapture`

Run: `cargo test --locked --all-features remote_dedup_ -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test cli_me_github_noreply_matches_without_token_or_network -- --nocapture`

Run: `cargo test --locked --all-features identity_report_ -- --nocapture`

Expected: all PASS; shared login call count is one, mixed-case email matches, later origin resolves, first successful conflict mapping wins, and no real network is used.

- [ ] **Step 8: Close the GitHub phase boundary**

Run: `cargo test --locked --all-features github::api::tests -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test -- --nocapture`

Expected: PASS. Record Tasks 4–7 exact files and permit only `fix: repair GitHub cache retry and identity flows` if explicit Git-write authorization exists. Do not push or tag.

### Task 8: Align DiskCache and composite Action persistence safely across runners

**Depends on:** Task 1, Task 4; Task 7 preferred complete

**Files:**
- Modify: `src/github/cache.rs` — exact environment override and inline tests
- Modify: `action.yml` — cache lifecycle, digest, locked install, one refresh flag
- Modify: `tests/action_test.rs` — static and Bash-stub assertions
- Test: inline cache tests and `tests/action_test.rs`

**Interfaces:**
- Consumes: internal `LOGIT_GITHUB_CACHE_DIR`, current 16 Action inputs, `svg-path`, existing `cmd` argv array and retry loop, runner temp/OS/run identity.
- Produces: `DiskCache::new()` honoring the override exactly; `dirs_or_fallback_with(get_env) -> PathBuf` test seam; one Action expression `${{ runner.temp }}/logit-github-cache` shared by cache `path` and command env; v4 unique key plus stable restore prefix; exactly one `--refresh-cache` argv element; locked source install.

**Recommended executor:** `coding`

- [ ] **Step 1: Write baseline-failing DiskCache override test without mutating process env**

```rust
#[test]
fn github_cache_dir_override_is_exact_and_not_extended() {
    let expected = PathBuf::from("C:/runner-temp/logit-github-cache");
    let actual = dirs_or_fallback_with(|name| {
        (name == "LOGIT_GITHUB_CACHE_DIR").then(|| expected.clone().into_os_string())
    });
    assert_eq!(actual, expected);
}

#[test]
fn github_cache_defaults_remain_platform_compatible_without_override() {
    let actual = dirs_or_fallback_with(|name| match name {
        "LOCALAPPDATA" => Some(PathBuf::from("C:/Local").into_os_string()),
        _ => None,
    });
    assert_eq!(actual, PathBuf::from("C:/Local/logit/cache/github"));
}
```

- [ ] **Step 2: Write baseline-failing Action cache/refresh/install contracts**

Add helpers that extract declared input names, output lines, named steps, and raw blocks without a YAML dependency. Add:

```rust
#[test]
fn action_preserves_inputs_output_and_uses_one_cross_platform_cache_directory() {
    let source = action_source();
    assert_eq!(declared_input_names(&source), INPUT_NAMES.into_iter().map(str::to_owned).collect::<Vec<_>>());
    assert!(source.contains("outputs:\n  svg-path:"));

    let cache_dir = "${{ runner.temp }}/logit-github-cache";
    assert_eq!(source.matches(&format!("path: {cache_dir}")).count(), 1);
    assert_eq!(source.matches(&format!("LOGIT_GITHUB_CACHE_DIR: {cache_dir}")).count(), 1);
    assert!(source.contains("logit-data-v4-${{ runner.os }}-${{ steps.datakey.outputs.hash }}-${{ github.run_id }}-${{ github.run_attempt }}"));
    assert!(source.contains("logit-data-v4-${{ runner.os }}-${{ steps.datakey.outputs.hash }}-"));
    assert!(!data_cache_key_line(&source).contains("inputs.username"));
}

#[test]
fn action_adds_refresh_once_for_card_and_multi_and_installs_locked() {
    let source = action_source();
    assert!(source.contains("cargo install --locked --path"));
    for command_name in ["card", "multi"] {
        let temp = TempDir::new().unwrap();
        let mut inputs = valid_inputs();
        inputs.insert("command".into(), command_name.into());
        let (output, argv_log, _) = run_action(&inputs, &temp, &[0]);
        assert!(output.status.success());
        let argv = logged_argv(&argv_log);
        assert_eq!(argv.iter().filter(|arg| arg.as_str() == "--refresh-cache").count(), 1);
    }
}
```

Extend the existing literal argv expected array with one `--refresh-cache` and keep every unsafe title/exclude/token assertion unchanged.

- [ ] **Step 3: Run RED**

Run: `cargo test --locked --all-features github_cache_dir_override_is_exact_and_not_extended -- --nocapture`

Expected: FAIL because no override exists.

Run: `cargo test --locked --all-features --test action_test action_preserves_inputs_output_and_uses_one_cross_platform_cache_directory -- --nocapture`

Expected: FAIL because Action cache path is Unix-home-specific, lacks env alignment, and uses the old nonunique key.

Run: `cargo test --locked --all-features --test action_test action_adds_refresh_once_for_card_and_multi_and_installs_locked -- --nocapture`

Expected: FAIL because install is unlocked and argv has no refresh flag.

- [ ] **Step 4: Implement exact cache directory override**

`dirs_or_fallback_with` checks `LOGIT_GITHUB_CACHE_DIR` first and returns its `PathBuf` without appending components. An absent/empty override falls through to the exact existing `LOCALAPPDATA`, `HOME`, and `.logit-cache/github` defaults. `DiskCache::new()` calls the seam with `std::env::var_os`; `with_dir` remains test-only and unchanged.

- [ ] **Step 5: Implement Action persistence without changing public inputs/output**

Add a `Compute GitHub data cache key` step with `id: datakey`. Map the data-affecting values (`username`, command, days, periods, include flags) through `env:` and hash NUL-delimited `printf` output with the already-used `sha256sum`; never interpolate user input into the shell program text. Presentation-only title/short/lang rows/output and post-fetch exclusions do not enter the data-cache digest. Use:

```yaml
path: ${{ runner.temp }}/logit-github-cache
key: logit-data-v4-${{ runner.os }}-${{ steps.datakey.outputs.hash }}-${{ github.run_id }}-${{ github.run_attempt }}
restore-keys: |
  logit-data-v4-${{ runner.os }}-${{ steps.datakey.outputs.hash }}-
```

Set `LOGIT_GITHUB_CACHE_DIR: ${{ runner.temp }}/logit-github-cache` in `Generate SVG`. Initialize `cmd` with exactly one `--refresh-cache` after the username for both commands. Change install to `cargo install --locked --path "${{ github.action_path }}" --features github --force`. Preserve all 16 inputs, `svg-path`, validation, literal arrays, retry count semantics, source hash cache, and token secrecy. The unique run ID/attempt save key prevents a failed run from replacing the prior successful snapshot while the stable prefix restores the newest compatible prior snapshot.

- [ ] **Step 6: Run GREEN and all Action safety regressions**

Run: `cargo test --locked --all-features github::cache::tests -- --nocapture`

Run: `cargo test --locked --all-features --test action_test -- --nocapture`

Expected: PASS; cache override is exact, Action path/env match, key is unique with a stable prefix, card/multi each carry one refresh flag, install is locked, and existing malicious-looking argv values remain literal.

- [ ] **Step 7: Record the task boundary**

Record `src/github/cache.rs`, `action.yml`, `tests/action_test.rs`, and outputs. Keep the Action/CI/docs phase open until Task 9.

### Task 9: Lock CI, documentation, compatibility, final gates, and review packet

**Depends on:** Tasks 1–8

**Files:**
- Modify: `.github/workflows/ci.yml` — exact Ubuntu/Windows jobs
- Modify: `README.md` — approved behavior documentation only
- Modify: `tests/action_test.rs` — CI and Action public-surface contracts
- Modify: `tests/fixture_test.rs` — CLI help/JSON/group/stderr compatibility packet
- Modify test-only section: `src/cli.rs` — exact long-option/short-alias contract; declarations remain byte-for-byte unchanged
- Verify only: `Cargo.toml`, `Cargo.lock`, all production/test files changed by Tasks 1–8
- Test: `tests/action_test.rs`, `tests/fixture_test.rs`, complete debug/release suites and real release CLI

**Interfaces:**
- Consumes: final CLI/Action/cache behavior, current public help, current JSON/group outputs, exact CI command contract.
- Produces: cost-bounded CI YAML; README explanations for partial identity/cache/Action behavior; executable compatibility tests; one artifact-identity review packet with formatter/Clippy/check/debug/release/LSP/CLI evidence.

**Recommended executor:** `complex`

- [ ] **Step 1: Write baseline-failing CI and public-surface tests**

Add to `tests/action_test.rs`:

```rust
#[test]
fn ci_has_exact_locked_ubuntu_quality_and_windows_test_contract() {
    let ci = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/ci.yml")).unwrap();
    for command in [
        "cargo fmt --all -- --check",
        "cargo clippy --locked --all-targets --all-features -- -D warnings",
        "cargo check --locked --no-default-features",
        "cargo build --locked --release --all-features",
    ] {
        assert_eq!(ci.matches(command).count(), 1, "missing/duplicate: {command}");
    }
    assert!(ci.contains("runs-on: windows-latest"));
    assert_eq!(ci.matches("cargo test --locked --all-features").count(), 2);
    assert!(!ci.contains("macos-latest"));
    assert!(!ci.to_ascii_lowercase().contains("msrv"));
}

#[test]
fn action_public_input_and_output_names_are_exactly_preserved() {
    let source = action_source();
    assert_eq!(declared_input_names(&source), INPUT_NAMES.into_iter().map(str::to_owned).collect::<Vec<_>>());
    assert_eq!(declared_output_names(&source), vec!["svg-path"]);
}
```

Add to `tests/fixture_test.rs`:

```rust
#[test]
fn cli_help_json_and_group_compatibility_contract_remains_intact() {
    let stats_help = Command::cargo_bin("logit").unwrap().args(["stats", "--help"]).output().unwrap();
    let help = String::from_utf8(stats_help.stdout).unwrap();
    for flag in ["--author", "--committer", "--since", "--until", "--exclude", "--repo", "--group", "--groups", "--days", "--me", "--dedup"] {
        assert!(help.contains(flag), "missing {flag}: {help}");
    }

    let temp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(temp.path());
    let flat = successful_stats_json(&[temp.path()], &[]);
    assert!(flat["periods"].is_array());
    for field in ["total_commits", "total_additions", "total_deletions", "total_net_modifications", "total_net_additions", "by_language", "by_author"] {
        assert!(flat["totals"].get(field).is_some(), "missing totals.{field}");
    }
}
```

Add this exhaustive declaration-surface test to the existing `src/cli.rs` test module so hidden `--compact` and every short alias are covered even though help omits hidden flags:

```rust
fn option_surface(command: &clap::Command) -> BTreeSet<(String, Option<char>)> {
    command
        .get_arguments()
        .filter_map(|arg| {
            let long = arg.get_long()?;
            (!matches!(long, "help" | "version"))
                .then(|| (long.to_string(), arg.get_short()))
        })
        .collect()
}

fn expected_surface(values: &[(&str, Option<char>)]) -> BTreeSet<(String, Option<char>)> {
    values.iter().map(|(long, short)| ((*long).to_string(), *short)).collect()
}

#[cfg(feature = "github")]
#[test]
fn cli_public_option_and_short_alias_surface_is_exact() {
    let command = Cli::command();
    let scan = command.get_subcommands().find(|sub| sub.get_name() == "scan").unwrap();
    let stats = command.get_subcommands().find(|sub| sub.get_name() == "stats").unwrap();
    let github = command.get_subcommands().find(|sub| sub.get_name() == "github").unwrap();
    let fetch = github.get_subcommands().find(|sub| sub.get_name() == "fetch").unwrap();
    let card = github.get_subcommands().find(|sub| sub.get_name() == "card").unwrap();
    let multi = github.get_subcommands().find(|sub| sub.get_name() == "multi").unwrap();

    assert_eq!(option_surface(scan), expected_surface(&[
        ("format", Some('f')), ("output", Some('o')),
    ]));
    assert_eq!(option_surface(stats), expected_surface(&[
        ("author", None), ("committer", None), ("since", None), ("until", None),
        ("period", None), ("lang", None), ("exclude-lang", None), ("exclude", None),
        ("format", Some('f')), ("output", Some('o')), ("repo", None), ("group", None),
        ("groups", None), ("days", Some('d')), ("show-email", None), ("dedup", None),
        ("me", None), ("sort", None), ("number-format", None), ("short", None),
        ("inline-tree", None), ("no-compact", None), ("compact", None),
        ("columns", None), ("exclude-columns", None),
    ]));
    assert_eq!(option_surface(fetch), expected_surface(&[
        ("since", None), ("until", None), ("days", Some('d')), ("include-forks", None),
        ("include-contributed", None), ("include-private", None), ("no-cache", None),
        ("refresh-cache", None), ("format", Some('f')), ("output", Some('o')),
        ("period", None), ("group", None), ("groups", None), ("number-format", None),
        ("short", None), ("no-compact", None), ("compact", None), ("inline-tree", None),
        ("columns", None), ("exclude-columns", None), ("exclude-lang", None),
        ("exclude", None), ("sort", None),
    ]));
    assert_eq!(option_surface(card), expected_surface(&[
        ("input", Some('i')), ("since", None), ("until", None), ("days", Some('d')),
        ("include-forks", None), ("include-contributed", None), ("include-private", None),
        ("no-cache", None), ("refresh-cache", None), ("title", None), ("short", None),
        ("number-format", None), ("number-format-lines", None), ("lang-rows", None),
        ("exclude-lang", None), ("exclude", None), ("output", Some('o')),
    ]));
    assert_eq!(option_surface(multi), expected_surface(&[
        ("periods", Some('p')), ("include-forks", None), ("include-contributed", None),
        ("include-private", None), ("no-cache", None), ("refresh-cache", None),
        ("exclude-lang", None), ("exclude", None), ("output", Some('o')),
        ("number-format", None), ("number-format-lines", None),
    ]));
}
```

Import `std::collections::BTreeSet` in the test module. The existing `cli_group_and_groups_historical_semantics` remains the authoritative ordered-fallback/subgroup and GitHub-author-rejection regression; do not replace or weaken it.

- [ ] **Step 2: Run RED**

Run: `cargo test --locked --all-features --test action_test ci_has_exact_locked_ubuntu_quality_and_windows_test_contract -- --nocapture`

Expected: FAIL because baseline CI lacks fmt, strict/all-target/locked Clippy, no-default check, release build, and Windows.

Run: `cargo test --locked --all-features --test action_test action_public_input_and_output_names_are_exactly_preserved -- --nocapture`

Expected before helper implementation: FAIL to compile; after helper-only code, it must PASS against both baseline and repaired Action, proving no public input/output drift.

Run: `cargo test --locked --all-features cli_public_option_and_short_alias_surface_is_exact -- --nocapture`

Expected after adding the contract test: PASS against the baseline declarations and remain PASS after CI/docs work; the independently failing CI contract remains the Task 9 RED gate.

- [ ] **Step 3: Implement the exact cost-bounded CI matrix**

Replace the Ubuntu job body with the five exact commands from Step 1. Install both `rustfmt` and `clippy` components. Add one independent `windows-latest` job with checkout, stable Rust, rust-cache, and only `cargo test --locked --all-features`. Do not add a matrix, macOS, MSRV, coverage service, package install, or prebuilt binary step.

- [ ] **Step 4: Update README without changing public syntax**

Document these existing/repaired behaviors next to their current sections:

- `--me github:<login>` and `--exclude @login` use known public commit emails plus local noreply matching; provider bounds/failures produce stderr warnings and may miss unknown emails.
- Email matching is case-insensitive; remote dedup checks each selected repository's GitHub `origin` independently.
- Default cache is read-only, `--refresh-cache` reads/updates, and `--no-cache` performs no cache I/O and wins over refresh.
- The composite Action persists `${{ runner.temp }}/logit-github-cache`, automatically refreshes once per invocation, and retains all listed inputs plus `svg-path`.
- Warnings go to stderr and do not alter successful JSON stdout.

Keep the existing group/groups explanation and Action input table names/defaults. Do not advertise macOS/MSRV support or a new flag.

- [ ] **Step 5: Run focused compatibility GREEN**

Run: `cargo test --locked --all-features --test action_test -- --nocapture`

Run: `cargo test --locked --all-features --test fixture_test -- --nocapture`

Run: `cargo test --locked --all-features cli::tests -- --nocapture`

Run: `cargo test --locked --all-features group_plan_keeps_group_as_fallback_and_groups_as_sublevels -- --nocapture`

Run: `cargo test --locked --all-features github_explicit_author_group_is_actionable_error -- --nocapture`

Expected: PASS; Action names/output, CLI flags, JSON fields/types, and group contracts are unchanged.

- [ ] **Step 6: Run the full debug/release quality gates**

Run each command separately in PowerShell and stop at the first nonzero exit:

Run: `cargo fmt --all -- --check`

Expected: exit 0 with no formatting diff.

Run: `cargo clippy --locked --all-targets --all-features -- -D warnings`

Expected: exit 0 with zero warnings.

Run: `cargo check --locked --no-default-features`

Expected: exit 0.

Run: `cargo test --locked --all-features`

Expected: exit 0; all unit/integration/doc tests pass without real network.

Run: `cargo test --locked --release --all-features`

Expected: exit 0; release-mode full suite passes.

Run: `cargo build --locked --release --all-features`

Expected: exit 0 and `target\release\logit.exe` exists on Windows (`target/release/logit` on Unix CI).

- [ ] **Step 7: Run LSP and real release CLI surfaces**

Call `lsp_diagnostics` with severity `all` for every changed Rust file: `src/main.rs`, `src/filter.rs`, `src/git/author.rs`, `src/analyze.rs`, `src/exclude.rs`, `src/stats/aggregator.rs`, `src/output/presentation.rs`, `src/github/api.rs`, `src/github/cache.rs`, `src/cli.rs`, `tests/action_test.rs`, and `tests/fixture_test.rs`.

Expected: no error or warning diagnostic on any file.

Run this PowerShell-compatible release surface check from the repository root:

```powershell
$binaryPath = Resolve-Path -LiteralPath "target\release\logit.exe"
& $binaryPath --help *> $null
if ($LASTEXITCODE -ne 0) { throw "release root help failed" }
& $binaryPath stats --help *> $null
if ($LASTEXITCODE -ne 0) { throw "release stats help failed" }
& $binaryPath github fetch --help *> $null
if ($LASTEXITCODE -ne 0) { throw "release github fetch help failed" }
$jsonText = & $binaryPath stats . --format json --since 2020-01-01
if ($LASTEXITCODE -ne 0) { throw "release stats JSON failed" }
$jsonValue = $jsonText | ConvertFrom-Json
if ($null -eq $jsonValue.periods -or $null -eq $jsonValue.totals.total_commits) {
    throw "release JSON compatibility fields missing"
}
```

Expected: every help command exits 0 and release JSON parses with `periods` plus numeric `totals.total_commits`. Do not invoke a live GitHub endpoint.

- [ ] **Step 8: Produce cleanup and formal-review packet**

Run: `git diff --check`

Run: `git status --short`

Run: `git diff --stat`

Run: `git rev-parse HEAD`

Expected: diff check exits 0; status/stat name only intended plan/spec and Tasks 1–9 files; HEAD identifies the artifact base/current committed identity. Confirm no temporary cache fixture, generated SVG, captured argv log, token, or TCP artifact is tracked or left outside test-owned `TempDir`.

Give the orchestrator one packet containing: HEAD, `git status --short`, changed-file list, every command/exit result from Steps 5–7, Windows CI expectation, no-network statement, and residual risks. The orchestrator—not an implementation worker—owns final Reviewer and Oracle dispatch against this one artifact identity; any post-review edit invalidates those receipts and requires the affected tests plus final gates again.

- [ ] **Step 9: Close the Action/CI/docs phase boundary**

Only after Steps 5–8 are green, permit `fix: persist Action cache and harden CI` when explicit Git-write authorization exists. If a formal blocker correction is required, use only `fix: address follow-up review blockers` after its failing regression and all affected gates pass. Never push or tag.

## Coverage Matrix

| Spec scope / completion criterion | Implementation task | Baseline-failing evidence | Final proof |
|---|---|---|---|
| Fractional duration, one clock, future GitHub start, cache policy | Task 1 | `positive_fractional_days_round_up_to_one_second`, `github_query_windows_use_one_clock_and_semantic_scopes`, `github_future_since_is_rejected_before_token_or_network` | Task 1 GREEN plus full suites |
| `--no-cache --refresh-cache` zero initialization/read/write and one warning | Task 1 | `disabled_cache_policy_never_invokes_cache_factory` and policy assertions | API tests plus fixture stderr assertion and full suites |
| v4 stable semantic key; v3 natural miss without deletion | Task 4 | `v4_rolling_keys_are_stable_across_observation_time_and_v3_files_naturally_miss` | v4 key/envelope tests |
| Rolling contribution boundary change performs exact full refetch, no gap merge | Task 4 | `changed_open_contribution_bounds_require_full_refetch_not_gap_merge`, TCP exact-window test | New-window request variables and no-cache-equivalent payload |
| Saturation incomplete/replay warning; cannot prove inactivity | Tasks 4–5 | incomplete envelope and saturation tests | completeness serialization plus existing saturation guard |
| History gap starts at `checked_until`, trims left, empty gap advances, nonempty OID dedup | Task 5 | four state-machine/TCP tests | refreshed payload equals fresh payload |
| Malformed/incompatible/rollback/capped history never claims complete | Task 5 | `malformed_incomplete_or_clock_rollback_history_plans_full_fetch`, capped envelope test | warnings, full fetch plan, incomplete round-trip |
| 429 seconds/HTTP-date precedence; exhausted 403 only; permission 403 one request; wait bound | Task 6 | three pure retry tests plus identity 403 TCP test | retry tests and existing transport suite |
| Bounded visible partial identity report | Task 6 | `identity_report_is_bounded_canonical_and_partial_when_provider_has_more` | one page, ≤8 repos, ≤20 commits/repo, request counts/warning |
| `--me github` and exclude share one lower-cased login resolution; command login budget and partial warning are singular; no-token noreply works | Task 7 | shared-index, mixed-case map, budget/multi-partial warning, real CLI noreply tests | call count one per admitted login, one warning, canonical map, CLI JSON total 1 |
| Every selected origin tried independently; later success; deterministic first-wins conflict | Task 7 | two remote-dedup fake-resolver tests | call order, mapping, warning assertions |
| Self/duplicate co-author commit-local normalization and intact totals | Task 2 | extraction/aggregation/tree tests | flat/tree/presentation tests |
| Windows selector/label/exclude case policy; other platforms unchanged; GitHub repository excludes always folded | Task 3 | pure explicit-platform/provider-mode tests and Windows CLI test | unit plus Windows CI locked suite |
| Distinct no-commits versus filtered-empty diagnostics | Task 3 | `cli_filter_empty_diagnostic_is_not_no_commits_diagnostic` | stderr wording and empty stdout assertions |
| Exact DiskCache override and cross-platform Action persistence | Task 8 | cache override and Action static tests | path/env equality, unique key/stable prefix |
| Action public surface, one refresh, locked install, argv safety | Tasks 8–9 | Action refresh/install/public-name tests | complete `action_test` suite |
| Ubuntu/Windows cost-bounded CI; no macOS/MSRV | Task 9 | `ci_has_exact_locked_ubuntu_quality_and_windows_test_contract` | static test plus workflow review |
| CLI flags, Action names/output, JSON shape, group contract unchanged | Task 9 | public-surface compile/static test and existing compatibility regressions | fixture/action/CLI suites and release CLI |
| README explains existing behavior and partial guarantees | Task 9 | documentation review against exact required bullets | diff review plus public-surface tests |
| Debug/release/full gates, LSP, release CLI, cleanup, review packet | Task 9 | final gate is blocked until Tasks 1–8 regressions are green | exact Step 5–8 evidence on one artifact identity |

## Spec Self-Review Results

- **Scope coverage:** Every item in spec Scope 1–12, Compatibility Contract, Architecture 1–9, Error/Data Flow, Testing Strategy, Stage Strategy, and Completion Criteria maps to at least one task and named assertion in the coverage matrix; no product scope was added.
- **Forbidden-marker scan:** Zero prohibited placeholder markers or deferred implementation phrases remain in this plan; every behavior-changing task includes concrete test names, core Rust/YAML/PowerShell content, RED reason, implementation interface, and GREEN command.
- **Type/interface consistency:** `CachePolicy`, `CacheWindowScope`, `QueryWindow`, `CacheEnvelope<T>`, `Completeness`, `IncompleteReason`, `IdentityResolutionReport`, `IdentityLookupFailure`, `RetryDecision`, `HistoryFetchPlan`, `canonical_email_key`, and all key/resolver signatures are introduced once and consumed under the same names in downstream tasks.
- **Dependency consistency:** Task 1 supplies Task 4/5 policy/window types; Task 4 supplies Task 5 envelopes; Task 6 supplies Task 7 reports; Task 2 supplies Task 3/7 canonical identity behavior; Task 8 consumes stable v4/path contracts; Task 9 starts only after Tasks 1–8 are green.
- **Compatibility consistency:** No plan step changes CLI declarations, Action input/output names, JSON/group envelopes, production retry count/timeout, dependency manifests, package version, macOS/MSRV support, or real-network test policy.
- **Task sizing:** Nine tasks each own one independently rejectable TDD outcome. Shared `src/github/api.rs` work is serialized; unrelated broad file splitting is explicitly excluded.

## Plan-Critic Receipt

- Status: `waiting for receipt`.
- Receipt owner: orchestrator; this planner role does not dispatch plan-critic, Reviewer, or Oracle profiles.
- Receipt scope: exactly the complete current revision of `docs/superpowers/plans/2026-08-30-follow-up-defect-remediation.md`.
- Invalidation rule: any edit to this plan requires a fresh plan-critic review of the complete revised file; dispatch acknowledgement, timeout, partial response, or an older-revision verdict is not a receipt.
- Execution handoff remains gated on the orchestrator's current-revision receipt/approval policy; no implementation or Git write was performed while producing this plan.
