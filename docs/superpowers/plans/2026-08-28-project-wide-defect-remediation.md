# Project-wide Defect Remediation Implementation Plan

> **For agentic workers:** Use the subagent-driven-development skill to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复批准设计中确认的全部 P0/P1 与直接相关 P2 缺陷，并用失败优先回归测试证明 CLI、本地 Git、GitHub、Action、SVG、table 和 TUI 的公共契约。

**Architecture:** 在现有模块边界内做外科式修复：输入与仓库先规范化，local/GitHub 共用 group plan，table/TUI 共用 renderer-neutral presentation model，GitHub 缓存、精确时间和网络可靠性仍在 `src/github/` 内演进。JSON 保持机器可读完整数据，不经过 presentation columns/number formatting；Action 与 SVG 在各自安全边界消除 shell/XML 重新解释。

**Tech Stack:** Rust 1.95.0、edition 2024、Cargo、git2 0.20、clap 4、chrono 0.4、rayon、reqwest 0.13 blocking、Tera 1、ratatui 0.30、crossterm 0.29、assert_cmd 2、predicates 3、tempfile 3、GitHub composite Action Bash。

**Spec:** `docs/superpowers/specs/2026-08-28-project-wide-defect-remediation-design.md`

**Global Constraints:**
- 初始行为基线是 HEAD `ecb0e3c3281e098ac0d0bedb2f788c7e3f85b0cf` 上 `cargo test --all-features` 的 171 个单元测试与 3 个集成测试全绿。
- `--group` 保持有序 fallback primary 候选；`--groups` 只表示选中 primary 下方的 subgroup levels，绝不反转两者语义。
- Local 支持 repo、author、period、language；GitHub 仅支持 repo、period、language，显式 author grouping 必须非零失败。
- Language 只能是最终层级；只有与已选 primary 重复的 subgroup 会被移除，其他重复层级必须报错。
- Table 与 TUI 必须共享语义行、排序、列、number format、inline language rows 和 totals；边框、颜色、导航与截断允许不同。
- JSON 不按 `--columns` 或 number formatting 裁剪；flat JSON 保留现有 envelope，hierarchical JSON 序列化 `GroupNode` 数据。
- Date-only `--until YYYY-MM-DD` 表示完整 UTC 日并转成下一日零点的 exclusive boundary；`--since` 仍从该日 UTC 零点 inclusive。
- Partial success 可以带 warning 输出数据；全部请求/发现的 repo 分析失败必须返回非零。
- Action 不使用 `eval` 或 `xargs` 重解析输入，不打印 token 或可执行的重建命令；`retry-count` 是首次尝试后的重试次数。
- SVG 所有动态文本/属性值必须经强制 XML escaping；自动化测试断言精确 XML entities，`pwsh` 可用时通过 .NET `[xml]` 做真实 parser 验证，当前 Windows 验收不得跳过 parser 测试。
- GitHub cache key 必须版本化并包含用户 node identity、无碰撞 owner/name、time range 与影响内容的 request modes；旧 cache 自然 miss。
- GitHub HTTP client 使用有限默认 timeout；重试 transient transport、408、429、5xx，且 pagination/cap/cache degradation 可见。
- 不删除或重命名现有 CLI flags，不增加通用 query language、timeout flag、全域重写、无证据重构、性能优化或 UI 润色。
- 不增加任何 production、dev 或 test dependency，不安装软件/包；复用现有 Rust dependencies、PowerShell 7 与 .NET XML parser。
- Windows 执行命令使用 PowerShell 7 语法；不得写 Bash-only 的工作区命令。
- 不执行或要求 `git add`、`git commit`、`git push`、`git tag` 或其他 Git 写命令；每个任务只记录待审文件。
- 后续 subagent 共用同一工作区；同一文件的修改必须按下述依赖顺序串行。

---

## Execution Waves and Dependency Order

| Wave | 可并行任务 | 进入条件 | 文件冲突规则 |
|---|---|---|---|
| 1 | Task 1（Action）、Task 2（SVG）、Task 3（input/exclude/failure）、Task 4（GitHub cache） | 基线全绿 | 四项文件集合互不重叠；允许独立 subagent 并行 |
| 2A | Task 5 → Task 6 → Task 7 | Task 3 完成；严格串行 | 依次共享 `src/main.rs`、`src/analyze.rs`、`src/stats/models.rs` |
| 2B | Task 8 → Task 9 | Task 4 完成；严格串行 | 两项共享 `src/github/api.rs` |
| 2 并行关系 | 2A 链与 2B 链并行 | Wave 1 对应依赖完成 | 两条链不得同时改 `src/github/api.rs` 之外的同一文件 |
| 3 | Task 10 | Tasks 7、9 完成 | 集中修改 `src/main.rs`、`src/cli.rs`、`src/stats/aggregator.rs`、`src/github/api.rs` |
| 4 | Task 11 | Task 10 完成 | presentation、table、TUI 与主分发串行收口 |
| 5 | Task 12 | Tasks 1–11 全绿 | 只做综合验证、debug journal 清理与范围核对 |

若执行器不支持并行，按 Task 1 → 12 的编号顺序执行仍满足全部依赖。任何 Task 的 GREEN 未通过时，不得启动依赖它的 Task。

## File Map

| File | Responsibility after remediation |
|---|---|
| `action.yml` | 安全地把 Action inputs 映射为环境变量、构造 argv array、验证输入、执行有限重试并传播最后状态 |
| `tests/action_test.rs` | Action 静态安全断言与在 Bash 可用时的 stub argv/retry/status 行为测试 |
| `src/github/svg.rs`, `src/templates/*.svg` | 在强制 XML autoescape 边界渲染 profile/multi SVG |
| `tests/github_svg_test.rs` | 通过真实 CLI JSON-input surface 断言精确 XML entities；条件探测 `pwsh` 并用 .NET `[xml]` 解析 SVG |
| `src/main.rs` | 输入验证、repo/filter/group wiring、输出分发与 nonzero error policy |
| `src/cli.rs` | 精确的 group/groups help 与 GitHub subgroup surface |
| `src/scanner.rs` | 返回 repo 与所有 skipped filesystem/repository warnings |
| `src/exclude.rs` | 以 `Result` 返回严格 exclude grammar validation |
| `src/analyze.rs` | 使用稳定 repo identity/label 分析，收集一次性 repo errors，merge commit 保留 commit 但不重复 churn |
| `src/git/repo.rs` | exclusive-until、invalid timestamp error 与 repo metadata |
| `src/git/diff.rs` | merge/rename/binary 正确 diff accounting |
| `src/git/author.rs`, `src/filter.rs` | 随 `CommitStats.repo_id` 更新既有测试 fixtures，不改变 author/filter 产品行为 |
| `src/stats/models.rs` | 保留 `repo_id` 与完整 `Name <email>` 聚合身份所需数据 |
| `src/stats/aggregator.rs` | commit-level language semantics、author identity、shared `GroupPlan` 与 trees/totals |
| `src/output/presentation.rs` | renderer-neutral numeric rows、depth/kind/labels、columns、sort 与 totals |
| `src/output/{column,table,tui,json}.rs` | table/TUI 消费 shared model；JSON 继续绕过 presentation trimming |
| `src/github/cache.rs` | 可区分 miss 与 read/parse failure 的 disk cache API |
| `src/github/api.rs` | versioned identity cache、exact commit data、per-repo windows、instant comparison、retry/pagination/cap diagnostics |
| `tests/common/mod.rs`, `tests/fixture_test.rs` | deterministic Git fixtures 与真实 CLI regressions |
| `README.md` | 在相应实现任务内同步 group/groups 与 Action retry 契约 |

### Task 1: Secure the composite Action argument and retry boundary

**Depends on:** none

**Files:**
- Create: `tests/action_test.rs`
- Modify: `action.yml:100-151`
- Modify: `README.md:149-168`
- Test: `tests/action_test.rs`

**Interfaces:**
- Consumes: composite Action inputs `username`, `token`, `command`, `days`, `periods`, `include-*`, `exclude-lang`, multiline `exclude`, `short`, `lang-rows`, `title`, `output`, `retry-count`, `retry-delay`.
- Produces: Bash array `cmd=(logit github ...)`; one literal argv element per user value; `total_attempts = INPUT_RETRY_COUNT + 1`; final failed child status returned unchanged.

- [ ] **Step 1: Add failing static and executable-stub tests**

Create tests with these exact names and core assertions:

```rust
#[test]
fn action_run_block_has_no_expression_interpolation_eval_or_xargs() {
    let yaml = std::fs::read_to_string("action.yml").unwrap();
    let run = generate_svg_run_block(&yaml);
    assert!(!run.contains("${{ inputs."), "inputs must enter through env");
    assert!(!run.contains("eval"));
    assert!(!run.contains("xargs"));
    assert!(run.contains("\"${cmd[@]}\""));
}

#[test]
fn action_stub_preserves_literal_argv_and_final_status_when_bash_exists() {
    let Some(bash) = find_bash() else { return };
    let result = run_generate_step_with_stub(
        &bash,
        ActionInputs {
            title: "x'; echo PWNED; $(touch never)".into(),
            excludes: "repo one\nrepo;two".into(),
            retry_count: "2".into(),
            stub_statuses: vec![23, 23, 23],
            ..ActionInputs::safe_defaults()
        },
    );
    assert_eq!(result.status.code(), Some(23));
    assert_eq!(result.invocations, 3);
    assert!(result.argv.iter().any(|arg| arg == "x'; echo PWNED; $(touch never)"));
    assert!(result.argv.iter().any(|arg| arg == "repo one"));
    assert!(result.argv.iter().any(|arg| arg == "repo;two"));
    assert!(!result.combined_output.contains(&result.token));
}
```

`generate_svg_run_block` 只提取 `- name: Generate SVG` 的 `run: |` 缩进行；stub 用 `printf '%s\0' "$@"` 写 argv 日志，并从计数文件依次返回状态。`find_bash` 用 `std::process::Command::new("bash").arg("--version")` 探测，Windows 缺 Bash 时仅跳过行为测试，静态测试仍执行。

同一文件再加入 `action_invalid_inputs_fail_before_stub`（逐一覆盖 command、四个 boolean、retry-count、retry-delay、days、lang-rows、periods）、`action_retry_count_zero_runs_once`、`action_retry_count_is_retries_after_initial_and_preserves_last_status`、`action_token_and_unsafe_command_are_not_logged`。每项分别断言 stub invocation count、exit code 与 stdout/stderr；非法输入必须是 invocation count `0` 和 exit code `2`。

- [ ] **Step 2: Run RED and verify the security assertions expose current behavior**

Run: `cargo test --all-features --test action_test -- --nocapture`

Expected: FAIL in `action_run_block_has_no_expression_interpolation_eval_or_xargs` because the run block contains `${{ inputs.* }}`, `eval $CMD`, and `xargs`; where Bash exists, the stub test also fails because injection-shaped values are re-parsed and the final status is not reliably `23`.

- [ ] **Step 3: Replace string command construction with validated argv transport**

Map every input in the Action step `env:` block and implement this control shape without echoing the reconstructed command:

```bash
case "$INPUT_COMMAND" in card|multi) ;; *) echo "Invalid command" >&2; exit 2 ;; esac
for value in "$INPUT_INCLUDE_FORKS" "$INPUT_INCLUDE_CONTRIBUTED" "$INPUT_INCLUDE_PRIVATE" "$INPUT_SHORT"; do
  case "$value" in true|false) ;; *) echo "Invalid boolean: $value" >&2; exit 2 ;; esac
[[ "$INPUT_RETRY_COUNT" =~ ^[0-9]+$ ]] || { echo "Invalid retry-count" >&2; exit 2; }
[[ "$INPUT_RETRY_DELAY" =~ ^[0-9]+([.][0-9]+)?$ ]] || { echo "Invalid retry-delay" >&2; exit 2; }
[[ "$INPUT_DAYS" =~ ^([0-9]+([.][0-9]*)?|[.][0-9]+)$ && "$INPUT_DAYS" =~ [1-9] ]] || { echo "Invalid days" >&2; exit 2; }

cmd=(logit github "$INPUT_COMMAND" "$INPUT_USERNAME")
if [[ "$INPUT_COMMAND" == multi ]]; then
  IFS=',' read -r -a periods <<< "$INPUT_PERIODS"
  ((${#periods[@]} > 0)) || { echo "Invalid periods" >&2; exit 2; }
  for period in "${periods[@]}"; do
    [[ "$period" =~ ^(week|month|quarter|half|year|[0-9]+([.][0-9]+)?[dD]?)$ ]] || exit 2
  done
  cmd+=(-p "$INPUT_PERIODS")
else
  cmd+=(-d "$INPUT_DAYS" --lang-rows "$INPUT_LANG_ROWS")
  [[ "$INPUT_SHORT" == true ]] && cmd+=(--short)
  [[ -n "$INPUT_TITLE" ]] && cmd+=(--title "$INPUT_TITLE")
fi
while IFS= read -r line; do
  [[ -n "$line" ]] && cmd+=(--exclude "$line")
done <<< "$INPUT_EXCLUDE"
cmd+=(-o "$INPUT_OUTPUT")

attempt=0
while (( attempt <= INPUT_RETRY_COUNT )); do
  set +e
  "${cmd[@]}"
  exit_code=$?
  set -e
  (( exit_code == 0 )) && exit 0
  (( attempt == INPUT_RETRY_COUNT )) && exit "$exit_code"
  sleep "$INPUT_RETRY_DELAY"
  ((attempt += 1))
done
```

Add explicit `lang-rows` nonnegative-integer validation, append each true `include-*` flag and nonempty `exclude-lang` exactly once, and keep `GITHUB_TOKEN` only in the environment. Update README `retry-count` wording to “Retries after the initial attempt”.

- [ ] **Step 4: Run GREEN and inspect the literal argv contract**

Run: `cargo test --all-features --test action_test -- --nocapture`

Expected: PASS; when Bash exists, the captured argv contains each malicious-looking value as one literal entry, retry-count `0` records one invocation, retry-count `2` records three, and the process exits with the stub's last nonzero status.

- [ ] **Step 5: Record the task boundary without Git writes**

Record for review: `action.yml`, `README.md`, `tests/action_test.rs`. Do not stage or commit them.

### Task 2: Enforce XML escaping for profile and multi SVG

**Depends on:** none

**Files:**
- Create: `tests/github_svg_test.rs`
- Modify: `src/github/svg.rs:105-302,600-878`
- Modify only if an assertion exposes an unescaped attribute: `src/templates/profile_card.svg`, `src/templates/multi_card.svg`
- Test: `src/github/svg.rs` inline tests
- Test: `tests/github_svg_test.rs`

**Interfaces:**
- Consumes: `render_profile_card(...) -> anyhow::Result<String>`, `render_multi_card(...) -> anyhow::Result<String>`, and `logit github card --input <json> --output <svg>`.
- Produces: Tera templates registered/rendered under mandatory XML-autoescaped names (`profile_card.xml`, `multi_card.xml`); exact entity sequence `&lt;`, `&gt;`, `&amp;`, `&quot;`, `&#x27;`; optional-platform test helper `find_pwsh() -> Option<PathBuf>` and mandatory-on-Windows .NET `[xml]` parse evidence.

- [ ] **Step 1: Add failing exact-entity tests without changing dependencies**

Add these exact unit tests using only existing dependencies:

```rust
#[test]
fn profile_svg_escapes_all_dynamic_xml_metacharacters() {
    let payload = r#"<script id="owned">&"'</script>"#;
    let escaped = "&lt;script id=&quot;owned&quot;&gt;&amp;&quot;&#x27;&lt;/script&gt;";
    let mut stats = make_stats();
    stats.by_language.insert(payload.into(), LangStats { additions: 1, ..Default::default() });
    let svg = render_profile_card(payload, &make_user(1), Some(&stats), 1,
        &ContributionSummary::default(), 30, false, NumberFormat::Plain,
        None, 2, Some(payload)).unwrap();
    assert!(svg.matches(escaped).count() >= 3, "username/title/language payloads must be entity encoded");
    assert!(!svg.contains("<script id=\"owned\">"));
    assert!(!svg.contains("id=\"owned\""));
    assert!(!svg.contains("</script>"));
}

#[test]
fn multi_svg_escapes_json_derived_language_names() {
    let mut stats = make_stats();
    let payload = r#"x" onload="alert(1)&<tag>'"#;
    let escaped = "x&quot; onload=&quot;alert(1)&amp;&lt;tag&gt;&#x27;";
    stats.by_language.insert(payload.into(), LangStats { additions: 1, ..Default::default() });
    let svg = render_multi_card(&[MultiColumnData { days: 7, stats, active_repos: 1 }],
        NumberFormat::Plain, None).unwrap();
    assert!(svg.contains(escaped));
    assert!(!svg.contains("onload=\"alert(1)\""));
    assert!(!svg.contains("<tag>"));
}
```

- [ ] **Step 2: Add the real CLI and conditional .NET XML parser test**

In `tests/github_svg_test.rs`, write a complete existing GitHub JSON envelope whose metadata username, user strings, `created_at`, and language key contain `<>&"'`; invoke the binary with `assert_cmd`; assert the same exact entity strings and absence of payload-created element/attribute. Add:

```rust
fn find_pwsh() -> Option<std::path::PathBuf> {
    std::process::Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| std::path::PathBuf::from("pwsh"))
}

#[test]
fn cli_generated_svg_is_well_formed_xml_when_pwsh_available() {
    let Some(pwsh) = find_pwsh() else {
        eprintln!("SKIP: pwsh unavailable; exact entity assertions remain active");
        return;
    };
    let (_temp, svg_path) = generate_cli_svg_with_metacharacter_json();
    let script = "$ErrorActionPreference='Stop'; $doc=[xml](Get-Content -Raw -LiteralPath $args[0]); if ($doc.DocumentElement.LocalName -ne 'svg') { throw 'root is not svg' }; 'DOTNET_XML_PARSED'";
    let output = std::process::Command::new(pwsh)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(&svg_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("DOTNET_XML_PARSED"));
}
```

`generate_cli_svg_with_metacharacter_json() -> (tempfile::TempDir, PathBuf)` uses `tempfile` and `assert_cmd`, both already declared; retaining `_temp` keeps the SVG alive for the whole parser invocation. It must not invoke or install any package manager.

- [ ] **Step 3: Run RED and verify raw Tera interpolation is exposed**

Run: `cargo test --all-features profile_svg_escapes_all_dynamic_xml_metacharacters -- --nocapture`

Expected: FAIL because `tera.add_raw_template("card", ...)` does not activate the XML suffix autoescape boundary, so the exact escaped sequence is absent and raw payload markup remains.

Run: `cargo test --all-features --test github_svg_test -- --nocapture`

Expected: FAIL because JSON-loaded metacharacters do not match the exact entity sequence; when `pwsh` is present, .NET `[xml]` also rejects malformed output.

- [ ] **Step 4: Register and render both raw templates as XML**

Use exact names at both registration and rendering sites:

```rust
tera.add_raw_template("profile_card.xml", TEMPLATE)?;
Ok(tera.render("profile_card.xml", &ctx)?)

tera.add_raw_template("multi_card.xml", MULTI_TEMPLATE)?;
Ok(tera.render("multi_card.xml", &ctx)?)
```

Keep fixed palette colors and numeric attributes unchanged. If a template assertion still finds an unsafe attribute, apply Tera's explicit XML escape filter at that interpolation; do not add a second ad-hoc escaping function.

- [ ] **Step 5: Run GREEN for unit and real CLI surfaces**

Run: `cargo test --all-features profile_svg_escapes_all_dynamic_xml_metacharacters -- --nocapture`

Run: `cargo test --all-features multi_svg_escapes_json_derived_language_names -- --nocapture`

Run: `cargo test --all-features --test github_svg_test -- --nocapture`

Expected: all PASS; exact entity assertions succeed and no payload-created element/attribute appears; where `pwsh` exists, stdout contains `DOTNET_XML_PARSED` after .NET reaches a valid `svg` root.

- [ ] **Step 6: Record the task boundary without Git writes**

Record for review: `src/github/svg.rs`, any changed template, `tests/github_svg_test.rs`. Confirm dependency manifests are untouched. Do not stage or commit them.

### Task 3: Validate ranges/excludes and make scan/analysis failure visible

**Depends on:** none

**Files:**
- Modify: `src/main.rs:52-147,262-268,600-688,691-725`
- Modify: `src/exclude.rs:86-298,362-575`
- Modify: `src/scanner.rs:5-58,60-155`
- Modify: `src/analyze.rs:18-47`
- Modify: `tests/fixture_test.rs`
- Test: inline tests in all four Rust modules and `tests/fixture_test.rs`

**Interfaces:**
- Consumes: `StatsArgs.days/since/until`, GitHub date/period inputs, repeated `--exclude`, scan roots, `(commits, errors)` from `analyze_repos`.
- Produces: `TimeRange { since: Option<DateTime<Utc>>, until_exclusive: Option<DateTime<Utc>> }`; `ExcludeRule::parse_many(value: &str) -> anyhow::Result<Vec<ExcludeRule>>`; `ScanReport { repos: Vec<PathBuf>, warnings: Vec<String> }`; scanner-invalid markers remain scanner warnings with no analyzed repo, while repositories that open successfully but fail `walk_commits` produce one caller-owned analyze error and all-repo nonzero status.

- [ ] **Step 1: Add failing parser/range/scanner tests**

Add exact unit tests:

```rust
#[test]
fn reversed_date_range_is_rejected_before_analysis() {
    let err = resolve_time_range(None, Some("2025-02-02"), Some("2025-02-01"), fixed_now()).unwrap_err();
    assert!(err.to_string().contains("--since must not be after --until"));
}

#[test]
fn date_only_until_becomes_exclusive_next_midnight() {
    let range = resolve_time_range(None, None, Some("2025-02-01"), fixed_now()).unwrap();
    assert_eq!(range.until_exclusive.unwrap().to_rfc3339(), "2025-02-02T00:00:00+00:00");
}

#[test]
fn invalid_days_are_rejected() {
    for days in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(resolve_time_range(Some(days), None, None, fixed_now()).is_err());
    }
}

#[test]
fn unknown_or_empty_exclude_qualifiers_are_errors() {
    for value in ["repo:nope:value", "repo:lang:", "repo:author:", "repo:lang:rust+"] {
        assert!(ExcludeRule::parse_many(value).is_err(), "{value}");
    }
}

#[test]
fn invalid_git_marker_is_reported_once() {
    let report = scan_for_repos(root_with_invalid_git_marker()).unwrap();
    assert!(report.repos.is_empty());
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains(".git"));
}
```

Keep `invalid_git_marker_is_reported_once` strictly as a scanner test: it expects `report.repos.is_empty()` and one scanner warning, so it does not exercise all-repo analysis failure.

For integration test `cli_all_repositories_failed_returns_nonzero_once`, build a distinct corrupted repository that still passes `git2::Repository::open`:

```rust
fn create_repo_with_missing_head_object(path: &Path) {
    let repo = git2::Repository::init(path).unwrap();
    let signature = git2::Signature::now("Broken", "broken@example.com").unwrap();
    let blob = repo.blob(b"content\n").unwrap();
    let tree_oid = {
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("file.txt", blob, 0o100644).unwrap();
        builder.write().unwrap()
    };
    let commit_oid = {
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "broken head", &tree, &[]).unwrap()
    };
    let hex = commit_oid.to_string();
    drop(repo);
    let object = path.join(".git").join("objects").join(&hex[..2]).join(&hex[2..]);
    std::fs::remove_file(object).unwrap();
    assert!(git2::Repository::open(path).is_ok());
}

#[test]
fn cli_all_repositories_failed_returns_nonzero_once() {
    let temp = tempfile::tempdir().unwrap();
    create_repo_with_missing_head_object(temp.path());
    let output = run_logit_stats(temp.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("failed to analyze").count(), 1);
    assert!(!stderr.contains("No commits found"));
}
```

The deleted loose HEAD object makes `Repository::open` succeed while `RepoAnalyzer::walk_commits` fails at `revwalk.push(head_oid)` or commit lookup. The fixture therefore reaches `analyze_repos`; it must not be replaced by an invalid marker or unborn repository.

- [ ] **Step 2: Run RED for each contract**

Run: `cargo test --all-features reversed_date_range_is_rejected_before_analysis`

Expected: FAIL because no `resolve_time_range` exists and current code accepts reversed/non-finite ranges.

Run: `cargo test --all-features unknown_or_empty_exclude_qualifiers_are_errors`

Expected: FAIL because `parse_many` returns `Vec`, silently drops unknown/empty qualifiers, and can create a repo-wide exclusion.

Run: `cargo test --all-features --test fixture_test cli_all_repositories_failed_returns_nonzero_once`

Expected: FAIL because the corrupted repository reaches analysis, the current callee and caller both print its error, and empty commits return success with `No commits found`.

- [ ] **Step 3: Implement deterministic input contracts before expensive work**

Introduce:

```rust
#[derive(Debug, Clone, Copy)]
struct TimeRange {
    since: Option<DateTime<Utc>>,
    until_exclusive: Option<DateTime<Utc>>,
}

fn resolve_time_range(
    days: Option<f64>,
    since: Option<&str>,
    until: Option<&str>,
    now: DateTime<Utc>,
) -> anyhow::Result<TimeRange>;
```

Reject `days <= 0`, non-finite days, duration overflow, invalid dates, and `since >= until_exclusive`. Use the same helper from local stats and GitHub fetch/card; make `parse_period` reject non-finite or nonpositive numeric periods before any request.

Change exclude parsing to propagate a diagnostic containing the offending value and qualifier. `parse_all_qualifiers`, `set_path`, and `parse_author` return `Result`; an empty repo-less rule, empty qualifier value, unknown key, missing `:`, or empty `+` component is an error. Adapt all four call sites in `src/main.rs` with `collect::<anyhow::Result<Vec<_>>>()?` and flatten only successful vectors.

- [ ] **Step 4: Return and print scanner warnings exactly once**

Implement:

```rust
pub struct ScanReport {
    pub repos: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn scan_for_repos(root: &Path) -> anyhow::Result<ScanReport>;
```

Push messages for `read_dir`, directory-entry, metadata, and invalid repository-open failures rather than discarding them. An invalid `.git` marker yields one `ScanReport.warning` and no `ScanReport.repo`. `cmd_scan` and `cmd_stats` print each collected scanner warning once. Remove `eprintln!` from `analyze_repos`; its caller alone prints each `RepoError`. After actual analysis, if the analyzed `repos` slice is nonempty and `errors.len() == repos.len()`, return `anyhow::bail!("failed to analyze all {} repositories", repos.len())`; partial data continues with warnings.

- [ ] **Step 5: Run GREEN and the unaffected valid syntax tests**

Run: `cargo test --all-features exclude::tests -- --nocapture`

Run: `cargo test --all-features scanner::tests -- --nocapture`

Run: `cargo test --all-features reversed_date_range_is_rejected_before_analysis`

Run: `cargo test --all-features --test fixture_test cli_all_repositories_failed_returns_nonzero_once`

Expected: all PASS; valid existing exclusion grammar still parses, every malformed example returns a stable error, the invalid marker is one scanner warning with zero repos, and the missing-object repository is one analyze error with a nonzero final status.

- [ ] **Step 6: Record the task boundary without Git writes**

Record for review: `src/main.rs`, `src/exclude.rs`, `src/scanner.rs`, `src/analyze.rs`, `tests/fixture_test.rs`. Do not stage or commit them.

### Task 4: Version and isolate GitHub cache data while preserving summaries

**Depends on:** none

**Files:**
- Modify: `src/github/cache.rs:3-85`
- Modify: `src/github/api.rs:1140-1551,1778-2270`
- Test: inline tests in both files

**Interfaces:**
- Consumes: authenticated `user_node_id`, username, owner/name, `since/until`, `include_forks`, `include_contributed`, `include_private`, cached contribution/history JSON.
- Produces: `DiskCache::get<T>(&self, key: &str) -> anyhow::Result<Option<T>>`; `CachedContributionWindow { repos, summary }`; schema-v2 collision-free cache keys; warning-and-fresh-fetch behavior for cache init/read/parse/write failures.

- [ ] **Step 1: Add failing key, summary, and cache-error tests**

Add exact tests:

```rust
#[test]
fn history_cache_key_is_user_scoped_and_component_collision_free() {
    let a = history_cache_key("NODE_A", "a/b", "repo", "2025-01-01", "2025-02-01", false);
    let b = history_cache_key("NODE_B", "a/b", "repo", "2025-01-01", "2025-02-01", false);
    let c = history_cache_key("NODE_A", "a_b", "repo", "2025-01-01", "2025-02-01", false);
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert!(a.starts_with("v2_history_"));
}

#[test]
fn completed_contribution_cache_hit_restores_summary() {
    let cached = CachedContributionWindow {
        repos: vec![(repo("alice", "one"), 4)],
        summary: ContributionSummary { total_prs: 3, total_reviews: 2, total_issues: 1 },
    };
    let restored = cache_round_trip_completed_window(cached);
    assert_eq!(restored.1.total_prs, 3);
    assert_eq!(restored.1.total_reviews, 2);
    assert_eq!(restored.1.total_issues, 1);
}

#[test]
fn malformed_cache_is_distinct_from_miss() {
    std::fs::write(cache_path_for_test("bad"), "{").unwrap();
    let err = cache.get::<Vec<u64>>("bad").unwrap_err();
    assert!(err.to_string().contains("bad.json"));
}

#[test]
fn cache_write_failure_keeps_fresh_result_and_returns_visible_warning() {
    let mut warnings = Vec::new();
    let fresh = repo_rows_fixture();
    cache_set_or_warn(&unwritable_cache(), "v2_test", &fresh, &mut warnings);
    assert_eq!(fresh.len(), 1);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("failed to write GitHub cache"));
    assert!(warnings[0].contains("v2_test"));
}
```

- [ ] **Step 2: Run RED and identify each current cache defect**

Run: `cargo test --all-features history_cache_key_is_user_scoped_and_component_collision_free`

Expected: FAIL because current history keys contain only sanitized owner/name and `a/b` collides with `a_b`.

Run: `cargo test --all-features completed_contribution_cache_hit_restores_summary`

Expected: FAIL with restored summary fields equal to zero because only repo rows are cached.

Run: `cargo test --all-features malformed_cache_is_distinct_from_miss`

Expected: FAIL because `DiskCache::get` maps parse/read failure to `None`.

- [ ] **Step 3: Implement schema-v2 keys and cache values**

Add:

```rust
const CACHE_SCHEMA_VERSION: u8 = 2;

#[derive(Serialize, Deserialize)]
struct CachedContributionWindow {
    repos: Vec<(RepoWithLangs, u64)>,
    summary: ContributionSummary,
}

fn encode_cache_component(value: &str) -> String {
    value.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
```

Define `contribution_cache_key(user_node_id, username, from, to, include_forks, include_contributed)` and `history_cache_key(user_node_id, owner, name, since, until, include_private)`; encode every string component, prefix `v2_`, and include exact range/modes. Pass `user_node_id` into contribution-cache lookup. Old unversioned files are never read and therefore naturally miss.

- [ ] **Step 4: Surface cache degradation without blocking fresh data**

Change `DiskCache::get` to return `Result<Option<T>>` with path context. Add internal `cache_get_or_warn<T>(cache, key, warnings) -> Option<T>` and `cache_set_or_warn<T>(cache, key, value, warnings)` helpers so tests can assert diagnostics; `fetch_user_stats` prints each accumulated warning once. At each API cache read, catch `Err`, add `Warning: failed to read/parse GitHub cache '<key>': ...`, and proceed as a miss. On `DiskCache::new` or `set` failure, add one contextual warning and keep the network result. Do not convert a network error into cache success.

- [ ] **Step 5: Run GREEN and prove two-user isolation through the cache flow**

Run: `cargo test --all-features github::cache::tests -- --nocapture`

Run: `cargo test --all-features history_cache_key_is_user_scoped_and_component_collision_free`

Run: `cargo test --all-features completed_contribution_cache_hit_restores_summary`

Add `history_cache_same_repo_two_users_never_share_commits` using the current pre-Task-8 `CommitData { additions, deletions, committed_date }`: seed Alice's schema-v2 key with `(11, 1, "2025-01-01T00:00:00Z")` and Bob's key for the same owner/repo/range with `(22, 2, "2025-01-02T00:00:00Z")`; read both keys and assert Alice receives only additions `11` while Bob receives only additions `22`. Do not reference `CommitData.oid`, which is introduced by Task 8.

Expected: all PASS; malformed cache is observable, summary survives cache hit, and identity/range/mode changes produce different keys.

- [ ] **Step 6: Record the task boundary without Git writes**

Record for review: `src/github/cache.rs`, `src/github/api.rs`. Do not stage or commit them.

### Task 5: Normalize repository identity, deduplicate discovery, and wire `--repo`

**Depends on:** Task 3

**Files:**
- Modify: `src/main.rs:61-101,271-328`
- Modify: `src/analyze.rs:12-99,101-242`
- Modify: `src/stats/models.rs:7-17,195-238`
- Modify fixtures only: `src/stats/aggregator.rs:552-610`
- Modify fixtures only: `src/git/author.rs:60-135`
- Modify fixtures only: `src/filter.rs:235-285`
- Modify: `tests/common/mod.rs`
- Modify: `tests/fixture_test.rs`
- Test: inline model/aggregator/author/filter/analyze tests and CLI integration tests

**Interfaces:**
- Consumes: direct repo paths plus `ScanReport.repos`, optional `StatsArgs.repo` selectors.
- Produces: `RepoInput { path: PathBuf, id: String, label: String }`; `normalize_repo_inputs(paths, selectors) -> anyhow::Result<Vec<RepoInput>>`; `CommitStats.repo_id` stable canonical identity and `CommitStats.repo` collision-free display label.

- [ ] **Step 1: Add failing duplicate, overlap, collision, and selector tests**

Add exact integration tests using `git2` fixtures, not shell Git commands:

```rust
#[test]
fn cli_duplicate_and_overlapping_paths_do_not_change_totals() {
    let single = run_json(&[repo_path]);
    let repeated = run_json(&[repo_path, repo_path, parent_scan_root]);
    assert_eq!(single["totals"], repeated["totals"]);
}

#[test]
fn cli_same_basename_repositories_have_distinct_shortest_labels() {
    create_repo(root.join("left/service"), 1);
    create_repo(root.join("right/service"), 2);
    let json = run_json_grouped_by_repo(root);
    let labels = period_labels(&json);
    assert_eq!(labels, ["left/service", "right/service"]);
    assert_ne!(json["periods"][0]["total_commits"], json["periods"][1]["total_commits"]);
}

#[test]
fn cli_repo_selector_is_applied_before_analysis() {
    let json = run_stats(&[root], &["--repo", "left/service", "--group", "repo", "-f", "json"]);
    assert_eq!(period_labels(&json), ["left/service"]);
}
```

Also assert an ambiguous bare selector `service` fails and names both distinguishing labels; an exact label or canonical path selects one repo.

- [ ] **Step 2: Run RED and verify collision/double-count behavior**

Run: `cargo test --all-features --test fixture_test cli_duplicate_and_overlapping_paths_do_not_change_totals`

Expected: FAIL because the same canonical repository is analyzed multiple times and totals increase.

Run: `cargo test --all-features --test fixture_test cli_same_basename_repositories_have_distinct_shortest_labels`

Expected: FAIL because both repositories use basename `service` and aggregate into one row.

Run: `cargo test --all-features --test fixture_test cli_repo_selector_is_applied_before_analysis`

Expected: FAIL because `StatsArgs.repo` is not read.

- [ ] **Step 3: Introduce stable identity and shortest distinguishing labels**

Implement:

```rust
#[derive(Debug, Clone)]
pub struct RepoInput {
    pub path: PathBuf,
    pub id: String,
    pub label: String,
}

pub fn normalize_repo_inputs(
    paths: Vec<PathBuf>,
    selectors: Option<&[String]>,
) -> anyhow::Result<Vec<RepoInput>>;
```

Canonicalize each path when `std::fs::canonicalize` succeeds; otherwise retain the original absolute/relative path. Normalize separators to `/` for `id`, deduplicate by case-normalized canonical identity on Windows and exact canonical identity elsewhere, then derive labels: unique basenames stay basename-only; collisions prepend the minimum parent suffix that distinguishes every member. Sort by `id` for deterministic analysis.

Selectors match exact label or exact normalized identity. A basename selector is accepted only when unique; an ambiguous basename returns a nonzero error listing valid labels. Apply selection before `analyze_repos` and before remote identity resolution.

- [ ] **Step 4: Carry identity separately through analysis**

Change:

```rust
pub fn analyze_repos(
    repos: &[RepoInput],
    since: Option<DateTime<Utc>>,
    until_exclusive: Option<DateTime<Utc>>,
) -> (Vec<CommitStats>, Vec<RepoError>);
```

Add `#[serde(default)] pub repo_id: String` to `CommitStats`; set `repo_id = input.id` and `repo = input.label`. Active-repo/skipped counts use `repo_id`; presentation grouping continues to use `repo`. Update every existing `CommitStats` literal with a deterministic `repo_id`: `src/stats/models.rs`, `src/stats/aggregator.rs`, `src/git/author.rs`, `src/filter.rs`, and production construction in `src/analyze.rs`. `#[serde(default)]` is only backward-compatible deserialization; it does not make the field optional in Rust struct literals.

- [ ] **Step 5: Run GREEN and verify analysis is performed once**

Run: `cargo test --all-features analyze::tests -- --nocapture`

Run: `cargo test --all-features --bin logit -- --nocapture`

Run: `cargo test --all-features --test fixture_test cli_duplicate_and_overlapping_paths_do_not_change_totals`

Run: `cargo test --all-features --test fixture_test cli_same_basename_repositories_have_distinct_shortest_labels`

Run: `cargo test --all-features --test fixture_test cli_repo_selector_is_applied_before_analysis`

Expected: all PASS; duplicate/overlap totals equal the single-input run, colliding names remain separate, and selector filtering occurs before analysis.

- [ ] **Step 6: Record the task boundary without Git writes**

Record for review: `src/main.rs`, `src/analyze.rs`, `src/stats/models.rs`, `src/stats/aggregator.rs`, `src/git/author.rs`, `src/filter.rs`, `tests/common/mod.rs`, `tests/fixture_test.rs`. Do not stage or commit them.

### Task 6: Preserve author identity and apply committer/language commit filters

**Depends on:** Task 5

**Files:**
- Modify: `src/main.rs:83-152,271-328`
- Modify: `src/stats/aggregator.rs:23-173,281-354,552-948`
- Modify: `src/output/table.rs:47-83,185-308,366-534,978-1452`
- Modify: `tests/common/mod.rs`
- Modify: `tests/fixture_test.rs`
- Test: inline aggregator/table tests and CLI integration tests

**Interfaces:**
- Consumes: full `Author { name, email }`, `StatsArgs.committer`, `StatsArgs.lang`, `DedupMode`, `EmailDisplay`, remote `identity_map`.
- Produces: raw `PeriodStats.by_author` keys in exact `Name <email>` form; commit-level `filter_commits_for_stats(commits, committer, language) -> Vec<CommitStats>`; presentation-time none/name/remote dedup with label-only email display changes.

- [ ] **Step 1: Add failing author/filter regressions**

Add exact tests:

```rust
#[test]
fn same_name_different_email_identities_survive_raw_aggregation() {
    let stats = aggregate_commits(&[
        commit("Alex", "a@example.com", rust_file()),
        commit("Alex", "b@example.com", rust_file()),
    ], &Period::Month, None, None);
    assert!(stats[0].by_author.contains_key("Alex <a@example.com>"));
    assert!(stats[0].by_author.contains_key("Alex <b@example.com>"));
}

#[test]
fn language_filter_drops_commit_without_matching_file_language() {
    let stats = aggregate_commits(&[commit("A", "a@x", python_file())],
        &Period::Month, None, Some("Rust"));
    assert!(stats.is_empty());
}

#[test]
fn author_table_none_and_name_dedup_preserve_expected_totals_and_email_labels() {
    let none = render_author_fixture(DedupMode::None, EmailDisplay::Full);
    assert!(none.contains("Alex <a@example.com>"));
    assert!(none.contains("Alex <b@example.com>"));
    let by_name = render_author_fixture(DedupMode::Name, EmailDisplay::None);
    assert_eq!(occurrences(&by_name, "Alex"), 1);
    assert!(by_name.contains("Total"));
}
```

Add CLI tests `cli_committer_matches_name_and_email` using distinct author/committer signatures and `cli_show_email_full_displays_history_email_without_changing_totals`.

- [ ] **Step 2: Run RED and identify discarded/wrongly counted identities**

Run: `cargo test --all-features same_name_different_email_identities_survive_raw_aggregation`

Expected: FAIL because aggregation keys only by `commit.author.name`.

Run: `cargo test --all-features language_filter_drops_commit_without_matching_file_language`

Expected: FAIL because `total_commits` increments before file-language filtering and returns a zero-metric commit bucket.

Run: `cargo test --all-features --test fixture_test cli_committer_matches_name_and_email`

Expected: FAIL because `args.committer` is never applied.

- [ ] **Step 3: Preserve full identities and filter commits before aggregation**

Use `Author::to_string()` for author and co-author aggregation keys and for `GroupBy::Author` partitions. Before building `identity_map` or aggregates, retain commits whose committer name or email matches `--committer` case-insensitively and whose file changes contain the requested language. Continue passing `lang_filter` to aggregation so nonmatching files inside retained commits do not contribute metrics.

Implement the explicit helper:

```rust
fn filter_commits_for_stats(
    commits: Vec<CommitStats>,
    committer: Option<&str>,
    language: Option<&str>,
) -> Vec<CommitStats> {
    commits.into_iter().filter(|commit| {
        committer.is_none_or(|p| commit.committer.matches(p))
            && language.is_none_or(|lang| commit.file_changes.iter()
                .any(|fc| fc.language.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(lang))))
    }).collect()
}
```

Make remote identity lookup keys lowercase email and make presentation lookup lowercase too. `DedupMode::None` keeps each full identity; `Name` merges extracted names; `Remote` merges only when a resolved login exists. `EmailDisplay::{None,Simple,Full}` changes only labels, never metrics.

- [ ] **Step 4: Run GREEN across raw aggregation and CLI surfaces**

Run: `cargo test --all-features stats::aggregator::tests -- --nocapture`

Run: `cargo test --all-features output::table::tests -- --nocapture`

Run: `cargo test --all-features --test fixture_test cli_committer_matches_name_and_email`

Run: `cargo test --all-features --test fixture_test cli_show_email_full_displays_history_email_without_changing_totals`

Expected: all PASS; same-name identities separate/merge only under the selected mode, committer name/email both match, language-only commits determine commit count, and email display leaves totals unchanged.

- [ ] **Step 5: Record the task boundary without Git writes**

Record for review: `src/main.rs`, `src/stats/aggregator.rs`, `src/output/table.rs`, `tests/common/mod.rs`, `tests/fixture_test.rs`. Do not stage or commit them.

### Task 7: Correct merge, rename, binary, timestamp, and exclusive-until Git behavior

**Depends on:** Task 6

**Files:**
- Modify: `src/git/diff.rs:8-67,69-183`
- Modify: `src/git/repo.rs:55-168,170-320`
- Modify: `src/analyze.rs:49-99,101-242`
- Test: inline tests in all three files

**Interfaces:**
- Consumes: `git2::Commit`, `RepoAnalyzer::walk_commits(since, until_exclusive)`, `CommitInfo.parent_oids`.
- Produces: unchanged public `analyze_commit_diff(repo: &Repository, commit: &Commit) -> anyhow::Result<Vec<FileChange>>`; merge commits with empty change vectors, rename-detected paths, binary `FileChange` with zero lines, explicit invalid timestamp error.

- [ ] **Step 1: Add failing Git fixtures for every confirmed defect**

Add exact tests:

```rust
#[test]
fn pure_rename_is_one_zero_churn_file_change() {
    let commit = commit_pure_rename("old.rs", "new.rs");
    let changes = analyze_commit_diff(&repo, &commit).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "new.rs");
    assert_eq!((changes[0].additions, changes[0].deletions), (0, 0));
}

#[test]
fn binary_delta_is_retained_with_zero_lines() {
    let commit = commit_binary_blob("asset.bin", &[0, 159, 146, 150]);
    let changes = analyze_commit_diff(&repo, &commit).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "asset.bin");
    assert_eq!((changes[0].additions, changes[0].deletions), (0, 0));
}

#[test]
fn merge_commit_counts_once_without_replaying_parent_churn() {
    let (commits, errors) = analyze_repos(&[merge_repo_input()], None, None);
    assert!(errors.is_empty());
    let merge = commits.iter().find(|c| c.message_subject == "merge feature").unwrap();
    assert!(merge.file_changes.is_empty());
    assert_eq!(aggregate_commits(&commits, &Period::Month, None, None)[0].total_commits,
        expected_reachable_commit_count());
}
```

Add `invalid_git_timestamp_is_analysis_error`, `until_includes_235959_of_named_day`, and `until_excludes_next_day_midnight`.

- [ ] **Step 2: Run RED and verify concrete wrong metrics**

Run: `cargo test --all-features pure_rename_is_one_zero_churn_file_change`

Expected: FAIL because rename detection is disabled and old/new appear as delete/add churn.

Run: `cargo test --all-features binary_delta_is_retained_with_zero_lines`

Expected: FAIL because binary deltas are skipped.

Run: `cargo test --all-features merge_commit_counts_once_without_replaying_parent_churn`

Expected: FAIL because the merge receives a first-parent diff and repeats reachable changes.

Run: `cargo test --all-features invalid_git_timestamp_is_analysis_error`

Expected: FAIL because invalid timestamps become Unix epoch/default.

- [ ] **Step 3: Implement minimal diff and timestamp corrections**

For merge commits (`parent_count() > 1`), return an empty `Vec<FileChange>` from `analyze_commit_diff`; `RepoAnalyzer::walk_commits` still returns the commit, so commit count remains intact. For non-merge commits, call rename similarity detection before reading deltas:

```rust
let mut find = git2::DiffFindOptions::new();
find.renames(true);
diff.find_similar(Some(&mut find))?;
```

Seed `file_stats` from every delta before `DiffFormat::Patch`, choosing `new_file().path()` then old path, so binary deltas survive as `(0, 0)`. Patch callbacks only increment text line counts.

Replace timestamp fallback with:

```rust
let timestamp = DateTime::from_timestamp(time.seconds(), 0)
    .ok_or_else(|| anyhow::anyhow!("invalid Git timestamp {} for commit {oid}", time.seconds()))?;
```

Interpret `until_exclusive` with `timestamp >= boundary` as excluded; retain `timestamp >= since`.

- [ ] **Step 4: Run GREEN and aggregate file totals**

Run: `cargo test --all-features git::diff::tests -- --nocapture`

Run: `cargo test --all-features git::repo::tests -- --nocapture`

Run: `cargo test --all-features analyze::tests -- --nocapture`

Expected: all PASS; pure rename is one zero-churn file, binary contributes one file and zero lines, merge contributes one commit and zero repeated churn, invalid timestamp is an error, and named-day until is inclusive only through 23:59:59.999… UTC.

- [ ] **Step 5: Record the task boundary without Git writes**

Record for review: `src/git/diff.rs`, `src/git/repo.rs`, `src/analyze.rs`. Do not stage or commit them.

### Task 8: Retain exact GitHub commits, per-repo windows, and stable dedup identity

**Depends on:** Task 4

**Files:**
- Modify: `src/github/api.rs:31-154,350-432,797-841,977-1171,1340-1732,1778-2270`
- Test: inline `src/github/api.rs` tests

**Interfaces:**
- Consumes: GraphQL history nodes, per-repo inclusive `since` and exclusive `until_exclusive`, schema-v2 cache entries from Task 4.
- Produces: `CommitData { oid: Option<String>, additions, deletions, committed_date }`; `RepoHistoryRequest { owner, name, since, until_exclusive }`; `PageRequest` carrying those boundaries plus cursor; `build_batch_history_variables(user_node_id: &str, active: &[PageRequest]) -> serde_json::Value`; exact `RepoContribution.commits`; instant-based `since <= commit < until_exclusive` filtering and OID-based dedup.

- [ ] **Step 1: Add failing timestamp/window/identity tests**

Add exact tests:

```rust
#[test]
fn equivalent_rfc3339_instants_compare_equal_for_range_filtering() {
    let commits = vec![
        CommitData {
            oid: Some("A".into()), additions: 1, deletions: 0,
            committed_date: "2025-01-01T01:00:00+01:00".into(),
        }, // 2025-01-01T00:00:00Z
        CommitData {
            oid: Some("B".into()), additions: 1, deletions: 0,
            committed_date: "2025-01-01T00:00:01Z".into(),
        }, // exactly until_exclusive
    ];
    let filtered = filter_commits_to_range(&commits,
        Some("2025-01-01T00:00:00Z"),
        Some("2025-01-01T00:00:01Z")).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].oid.as_deref(), Some("A"));
    assert!(!filtered.iter().any(|commit| commit.oid.as_deref() == Some("B")));
}

#[test]
fn identical_metrics_without_oid_are_not_collapsed() {
    let one = CommitData {
        oid: None, additions: 1, deletions: 2,
        committed_date: "2025-01-01T00:00:00Z".into(),
    };
    let commits = vec![one.clone(), one];
    assert_eq!(dedup_commits(commits).len(), 2);
}

#[test]
fn day_and_month_buckets_use_original_commit_instants_not_monday() {
    let commits = vec![
        CommitData { oid: Some("A".into()), additions: 2, deletions: 1,
            committed_date: "2025-01-31T23:59:59Z".into() },
        CommitData { oid: Some("B".into()), additions: 3, deletions: 1,
            committed_date: "2025-02-01T00:00:00Z".into() },
    ];
    let contrib = RepoContribution {
        repo_name: "o/r".into(), total_commits: 2,
        total_additions: 5, total_deletions: 2,
        weeks: vec![], commits,
        languages: HashMap::from([("Rust".into(), 1)]),
    };
    let day = contributions_to_period_stats(std::slice::from_ref(&contrib), &Period::Day);
    assert_eq!(day.iter().map(|row| row.period_label.as_str()).collect::<Vec<_>>(),
        vec!["2025-01-31", "2025-02-01"]);
    let month = contributions_to_period_stats(std::slice::from_ref(&contrib), &Period::Month);
    assert_eq!(month.iter().map(|row| row.period_label.as_str()).collect::<Vec<_>>(),
        vec!["2025-01", "2025-02"]);
}

#[test]
fn batch_history_uses_and_filters_each_repository_window() {
    let active = vec![
        PageRequest { batch_index: 0, owner: "o".into(), name: "old".into(),
            since: Some("2025-01-01T00:00:00Z".into()),
            until_exclusive: Some("2025-03-01T00:00:00Z".into()), after: None },
        PageRequest { batch_index: 1, owner: "o".into(), name: "new".into(),
            since: Some("2025-02-01T00:00:00Z".into()),
            until_exclusive: Some("2025-03-01T00:00:00Z".into()), after: None },
    ];
    let query = build_batch_history_query(&active);
    assert!(query.contains("$since0"));
    assert!(query.contains("$since1"));
    let variables = build_batch_history_variables("NODE", &active);
    assert_eq!(variables["since0"], "2025-01-01T00:00:00Z");
    assert_eq!(variables["since1"], "2025-02-01T00:00:00Z");
    let mixed = vec![
        CommitData { oid: Some("old".into()), additions: 1, deletions: 0,
            committed_date: "2025-01-31T23:59:59Z".into() },
        CommitData { oid: Some("new".into()), additions: 1, deletions: 0,
            committed_date: "2025-02-01T00:00:00Z".into() },
    ];
    let filtered = filter_commits_to_range(
        &mixed, active[1].since.as_deref(), active[1].until_exclusive.as_deref()).unwrap();
    assert_eq!(filtered.iter().map(|commit| commit.oid.as_deref()).collect::<Vec<_>>(),
        vec![Some("new")]);
}
```

- [ ] **Step 2: Run RED and expose lexical/Monday/shared-window behavior**

Run: `cargo test --all-features equivalent_rfc3339_instants_compare_equal_for_range_filtering`

Expected: FAIL because range filtering compares RFC3339 strings lexically.

Run: `cargo test --all-features identical_metrics_without_oid_are_not_collapsed`

Expected: FAIL because dedup uses date/additions/deletions only.

Run: `cargo test --all-features day_and_month_buckets_use_original_commit_instants_not_monday`

Expected: FAIL because GitHub data is reduced to Monday timestamps before day/month bucketing.

Run: `cargo test --all-features batch_history_uses_and_filters_each_repository_window`

Expected: FAIL because each batch uses one minimum `since` and does not filter each repo before merge/cache.

- [ ] **Step 3: Request stable OIDs and retain exact commit data**

Add `oid` to GraphQL history node selection and deserialize it as `Option<String>` with `#[serde(default)]`. Add exact commits to `RepoContribution` while retaining weekly derivation only for week/card compatibility:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitData {
    #[serde(default)]
    pub oid: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub committed_date: String,
}

pub struct RepoHistoryRequest {
    owner: String,
    name: String,
    since: Option<String>,
    until_exclusive: Option<String>,
}
```

Deduplicate only equal nonempty OIDs. If either record lacks OID, retain both; never collapse solely by date/additions/deletions.

- [ ] **Step 4: Make request variables and filtering per repository**

Change `batch_commit_history` to consume `&[RepoHistoryRequest]`; generate `$sinceN`, `$untilN`, `$afterN` per alias, with `$untilN` carrying the exclusive boundary. Parse all range endpoints with `DateTime::parse_from_rfc3339` and compare instants using `since <= committed_at && committed_at < until_exclusive`. Immediately filter each fetched repo against its own request range before overlap merge and cache write. Determine cached `data_until` by parsed instant, not lexical max.

Convert GitHub period/repo stats from `RepoContribution.commits`: bucket every original instant with `bucket_timestamp`; apportion that commit's metrics across repo languages; sum numeric metrics after bucketing. Week output may derive Monday labels, but day/month never see normalized Monday timestamps.

- [ ] **Step 5: Run GREEN and boundary totals**

Run: `cargo test --all-features equivalent_rfc3339_instants_compare_equal_for_range_filtering`

Run: `cargo test --all-features identical_metrics_without_oid_are_not_collapsed`

Run: `cargo test --all-features day_and_month_buckets_use_original_commit_instants_not_monday`

Run: `cargo test --all-features batch_history_uses_and_filters_each_repository_window`

Run: `cargo test --all-features github::api::tests -- --nocapture`

Expected: all PASS; the offset timestamp equal to `since` is retained, the commit exactly equal to `until_exclusive` is excluded, missing-OID records remain distinct, day/month boundaries are exact, and mixed repo windows cannot leak commits.

- [ ] **Step 6: Record the task boundary without Git writes**

Record for review: `src/github/api.rs`. Do not stage or commit it.

### Task 9: Bound GitHub HTTP retries and expose incomplete pagination

**Depends on:** Task 8

**Files:**
- Modify: `src/github/api.rs:156-612,721-761,812-841,977-1058,1778-2270`
- Test: inline `src/github/api.rs` tests using a standard-library TCP stub

**Interfaces:**
- Consumes: GraphQL and REST request factories, response statuses/headers/pageInfo, retry policy.
- Produces: `GithubClient::for_test(base_url, retry_delays, timeout)`; `resolve_single_email_result(...) -> anyhow::Result<Option<String>>`; 30-second production timeout; bounded GraphQL and REST retry for connect/timeout plus 408/429/5xx; query-builder URL encoding; missing-cursor error; testable pagination cap warning.

- [ ] **Step 1: Add a dependency-free HTTP stub and failing reliability tests**

Use `std::net::TcpListener` on `127.0.0.1:0`, a thread that returns a configured status/body sequence or delays a response, and a test-only client constructor with injected retry delays and timeout. Add exact tests:

```rust
#[test]
fn graphql_retries_408_429_and_5xx_then_succeeds_with_a_bound() {
    let server = StubServer::statuses([408, 429, 503, 200], graphql_ok_body());
    let client = GithubClient::for_test(
        server.base_url(), vec![Duration::ZERO; 6], Duration::from_secs(1));
    assert!(client.graphql_query("query { viewer { login } }", &json!({})).is_ok());
    assert_eq!(server.request_count(), 4);
}

#[test]
fn retry_exhaustion_returns_last_http_status() {
    let server = StubServer::statuses([503; 7], "{}");
    let err = GithubClient::for_test(
        server.base_url(), vec![Duration::ZERO; 6], Duration::from_secs(1))
        .graphql_query("query { viewer { login } }", &json!({})).unwrap_err();
    assert!(err.to_string().contains("503"));
    assert_eq!(server.request_count(), 7);
}

#[test]
fn rest_author_query_is_url_encoded() {
    let server = StubServer::ok_json("[]");
    GithubClient::for_test(server.base_url(), vec![], Duration::from_secs(1))
        .resolve_single_email("o", "r", "a+b @example.com");
    let target = server.last_target();
    let encoded = query_component(&target, "author").unwrap();
    assert!(encoded.contains("%2B"), "literal plus must be percent encoded: {encoded}");
    assert!(encoded.contains("%40"), "@ must be percent encoded: {encoded}");
    assert!(encoded.contains("%20") || encoded.contains('+'),
        "space may be %20 or form-style +: {encoded}");
    assert_eq!(decode_form_query_component(encoded), "a+b @example.com");
}

#[test]
fn has_next_page_without_cursor_is_an_incomplete_response_error() {
    let err = pagination_decision(true, None, 1, 20, "o/r").unwrap_err();
    assert!(err.to_string().contains("hasNextPage=true"));
    assert!(err.to_string().contains("missing cursor"));
}

#[test]
fn graphql_transient_transport_error_retries_then_succeeds() {
    let server = StubServer::drop_connections_then_respond(1, 200, graphql_ok_body());
    let client = GithubClient::for_test(
        server.base_url(), vec![Duration::ZERO; 6], Duration::from_secs(1));
    assert!(client.graphql_query("query { viewer { login } }", &json!({})).is_ok());
    assert_eq!(server.accepted_connection_count(), 2);
}

#[test]
fn graphql_timeout_retries_and_exhausts_with_a_wall_clock_bound() {
    let server = StubServer::delayed_responses(
        3, Duration::from_millis(100), 200, graphql_ok_body());
    let started = Instant::now();
    let err = GithubClient::for_test(
        server.base_url(), vec![Duration::ZERO; 2], Duration::from_millis(10))
        .graphql_query("query { viewer { login } }", &json!({})).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("timed out"));
    assert_eq!(server.request_count(), 3);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn rest_identity_retries_transient_status_then_succeeds() {
    let body = r#"[{"author":{"login":"alice"}}]"#;
    let server = StubServer::status_bodies([(503, "{}"), (200, body)]);
    let login = GithubClient::for_test(
        server.base_url(), vec![Duration::ZERO], Duration::from_secs(1))
        .resolve_single_email_result("o", "r", "alice@example.com").unwrap();
    assert_eq!(login.as_deref(), Some("alice"));
    assert_eq!(server.request_count(), 2);
}

#[test]
fn rest_identity_retry_exhaustion_is_bounded_and_visible() {
    let server = StubServer::statuses([503, 503, 503], "{}");
    let err = GithubClient::for_test(
        server.base_url(), vec![Duration::ZERO; 2], Duration::from_secs(1))
        .resolve_single_email_result("o", "r", "alice@example.com").unwrap_err();
    assert!(err.to_string().contains("503"));
    assert_eq!(server.request_count(), 3);
}
```

Define the two test-only helpers in the same `src/github/api.rs` test module; they use only `str`/byte operations:

```rust
fn query_component<'a>(target: &'a str, key: &str) -> Option<&'a str> {
    target.split_once('?')?.1.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn decode_form_query_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => { decoded.push(b' '); index += 1; }
            b'%' => {
                assert!(index + 2 < bytes.len(), "truncated percent escape");
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap();
                decoded.push(u8::from_str_radix(hex, 16).unwrap());
                index += 3;
            }
            byte => { decoded.push(byte); index += 1; }
        }
    }
    String::from_utf8(decoded).unwrap()
}
```

Add cap tests for history 20 pages/2000 commits, owned/contributed 300 repos, and private 10 pages/1000 repos; assert returned `PaginationDecision::Capped(message)` contains the scope and limit.

- [ ] **Step 2: Run RED and verify current unbounded/incomplete paths**

Run: `cargo test --all-features graphql_retries_408_429_and_5xx_then_succeeds_with_a_bound`

Expected: FAIL because 408/429 are not retried and endpoints/backoff are not injectable.

Run: `cargo test --all-features graphql_transient_transport_error_retries_then_succeeds`

Expected: FAIL because current transport errors return immediately instead of rebuilding and retrying the request.

Run: `cargo test --all-features graphql_timeout_retries_and_exhausts_with_a_wall_clock_bound`

Expected: FAIL because the current client has no timeout and the injected delayed server cannot be bounded or retried.

Run: `cargo test --all-features rest_identity_retries_transient_status_then_succeeds`

Expected: FAIL because `resolve_single_email` maps the first REST 503 directly to `None` instead of using the transient retry policy.

Run: `cargo test --all-features rest_identity_retry_exhaustion_is_bounded_and_visible`

Expected: FAIL because current REST failures are silently converted to `None` and expose neither the last 503 nor a bounded request count.

Run: `cargo test --all-features rest_author_query_is_url_encoded`

Expected: FAIL because the email is interpolated directly into the URL.

Run: `cargo test --all-features has_next_page_without_cursor_is_an_incomplete_response_error`

Expected: FAIL because pagination silently stops.

- [ ] **Step 3: Add a finite client configuration and one retry loop**

Build the production client with `.timeout(std::time::Duration::from_secs(30))`. Store GraphQL/REST base URLs and retry delays in `GithubClient`; `new()` uses GitHub URLs, a 30-second timeout, and `[1,2,5,15,30,60]` seconds, while `for_test(base_url, retry_delays, timeout)` injects all three values. The delayed-response test must prove the same client builder honors the injected 10ms timeout; production differs only in the duration value.

Create one internal request loop whose factory rebuilds the request each attempt. Retry only `reqwest::Error::is_timeout()`, `is_connect()`, status 408, 429, and 500–599; retain 403 rate-limit header handling; fail immediately for authentication and other 4xx. Maximum remains initial attempt plus six retries. Both `resolve_single_email_result` and each REST commit request inside `resolve_user_emails` must use this same loop; the existing optional wrapper may convert a final identity miss to `None`, but retry exhaustion/cache-resolution callers must emit one contextual warning rather than silently swallowing the error.

- [ ] **Step 4: Encode REST queries and centralize pagination decisions**

Replace interpolated query text with:

```rust
self.client
    .get(format!("{}/repos/{owner}/{repo}/commits", self.rest_base_url))
    .query(&[("author", email), ("per_page", "1")]);
```

Implement:

```rust
enum PaginationDecision {
    Stop,
    Continue(String),
    Capped(String),
}

fn pagination_decision(
    has_next_page: bool,
    end_cursor: Option<String>,
    fetched: usize,
    cap: usize,
    scope: &str,
) -> anyhow::Result<PaginationDecision>;
```

`hasNextPage=true` plus no cursor is always an error. Caps return a warning message that the caller prints exactly once before returning partial data. Apply it to history, owned, contributed, and private pagination.

- [ ] **Step 5: Run GREEN and ensure retries are bounded**

Run: `cargo test --all-features graphql_retries_408_429_and_5xx_then_succeeds_with_a_bound -- --nocapture`

Run: `cargo test --all-features retry_exhaustion_returns_last_http_status -- --nocapture`

Run: `cargo test --all-features graphql_timeout_retries_and_exhausts_with_a_wall_clock_bound -- --nocapture`

Run: `cargo test --all-features rest_identity_retries_transient_status_then_succeeds -- --nocapture`

Run: `cargo test --all-features rest_identity_retry_exhaustion_is_bounded_and_visible -- --nocapture`

Run: `cargo test --all-features rest_author_query_is_url_encoded`

Run: `cargo test --all-features has_next_page_without_cursor_is_an_incomplete_response_error`

Run: `cargo test --all-features github::api::tests -- --nocapture`

Expected: all PASS; delayed responses time out/retry within the asserted wall-clock bound, GraphQL and REST request counts never exceed initial attempt plus configured retries, REST 503 can recover and exhausted status is visible, the captured author component contains `%2B` and `%40`, accepts either `%20` or `+` for the space, decodes exactly to `a+b @example.com`, missing cursor errors, cap messages identify incomplete results, and existing 403 handling remains green.

- [ ] **Step 6: Record the task boundary without Git writes**

Record for review: `src/github/api.rs`. Do not stage or commit it.

### Task 10: Resolve one shared local/GitHub group plan and update its public contract

**Depends on:** Tasks 7 and 9

**Files:**
- Modify: `src/cli.rs:94-109,186-192,305-379,543-552`
- Modify: `src/stats/aggregator.rs:347-504,506-948`
- Modify: `src/github/api.rs:1594-1732,1778-2270`
- Modify: `src/main.rs:149-257,331-438`
- Modify: `src/output/json.rs:1-106`
- Modify: `README.md:27-59,83-115`
- Modify: `tests/fixture_test.rs`
- Test: inline CLI/aggregator/GitHub/JSON tests and CLI integration tests

**Interfaces:**
- Consumes: primary candidates, subgroup list, source capabilities, observed cardinalities, local commits or exact GitHub contributions.
- Produces: `GroupSource`, `GroupCardinality`, `GroupPlan`, `resolve_group_plan(...) -> Result<GroupPlan, String>`; existing local `build_group_tree(...) -> Vec<GroupNode>` aligned with `GroupPlan`; `github::api::contributions_to_group_tree(contributions: &[RepoContribution], levels: &[GroupBy], period: &Period) -> Vec<GroupNode>`; GitHub `--groups`; explicit unsupported-author error; compatible flat/hierarchical JSON.

- [ ] **Step 1: Add failing resolver and CLI contract tests**

Add the exact types to the tests first and write:

```rust
#[test]
fn group_plan_keeps_group_as_fallback_and_groups_as_sublevels() {
    let counts = GroupCardinality { repo: 1, author: 2, period: 3, language: 4 };
    let plan = resolve_group_plan(
        &[GroupBy::Repo, GroupBy::Author, GroupBy::Language],
        &[GroupBy::Author, GroupBy::Period],
        &counts,
        GroupSource::Local,
    ).unwrap();
    assert_eq!(plan.primary, GroupBy::Author);
    assert_eq!(plan.levels, vec![GroupBy::Author, GroupBy::Period]);
    assert!(plan.hierarchical);
}

#[test]
fn duplicate_selected_primary_is_removed_but_other_duplicate_errors() {
    let counts = GroupCardinality { repo: 2, author: 2, period: 2, language: 2 };
    let selected_duplicate = resolve_group_plan(
        &[GroupBy::Repo],
        &[GroupBy::Repo, GroupBy::Period],
        &counts,
        GroupSource::Local,
    ).unwrap();
    assert_eq!(selected_duplicate.levels, vec![GroupBy::Repo, GroupBy::Period]);

    let other_duplicate = resolve_group_plan(
        &[GroupBy::Repo],
        &[GroupBy::Period, GroupBy::Period],
        &counts,
        GroupSource::Local,
    ).unwrap_err();
    assert!(other_duplicate.contains("duplicate"));
    assert!(other_duplicate.contains("Period"));
}

#[test]
fn github_explicit_author_group_is_actionable_error() {
    let counts = GroupCardinality { repo: 2, author: 0, period: 2, language: 3 };
    let err = resolve_group_plan(
        &[GroupBy::Author],
        &[],
        &counts,
        GroupSource::Github,
    ).unwrap_err();
    assert!(err.contains("author"));
    assert!(err.contains("GitHub contribution records have no author identity"));
}

#[test]
fn unique_subgroups_are_skipped_for_flat_and_hierarchical_paths() {
    let counts = GroupCardinality { repo: 2, author: 1, period: 2, language: 3 };
    let plan = resolve_group_plan(
        &[GroupBy::Repo],
        &[GroupBy::Author, GroupBy::Period],
        &counts,
        GroupSource::Local,
    ).unwrap();
    assert_eq!(plan.levels, vec![GroupBy::Repo, GroupBy::Period]);
    assert!(plan.hierarchical);
}

#[test]
fn group_node_tree_follows_plan_level_order_and_preserves_totals() {
    let counts = GroupCardinality { repo: 2, author: 2, period: 2, language: 1 };
    let plan = resolve_group_plan(
        &[GroupBy::Repo],
        &[GroupBy::Period],
        &counts,
        GroupSource::Local,
    ).unwrap();
    let commits = vec![
        make_commit_in_repo("repo-b", "Alice", "a@x", vec![],
            Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap(),
            vec![rust_file("b.rs", 7, 2)]),
        make_commit_in_repo("repo-a", "Bob", "b@x", vec![],
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            vec![rust_file("a.rs", 5, 1)]),
    ];
    let nodes = build_group_tree(&commits, &plan.levels, &Period::Month, None, None);
    assert_eq!(nodes.iter().map(|node| node.label.as_str()).collect::<Vec<_>>(),
        vec!["repo-a", "repo-b"]);
    assert_eq!(nodes[0].children[0].label, "2025-01");
    assert_eq!(nodes[1].children[0].label, "2025-02");
    assert_eq!(nodes.iter().map(|node| node.stats.total_commits).sum::<u64>(), 2);
    assert_eq!(nodes.iter().map(|node| node.stats.total_additions).sum::<u64>(), 12);
}
```

The existing aggregator test helpers `make_commit_in_repo` and `rust_file` are defined upstream in `src/stats/aggregator.rs`; this test introduces no future presentation symbol. In `src/github/api.rs`, add `github_group_node_tree_follows_plan_level_order_and_preserves_totals` using explicit `RepoContribution`/`CommitData` literals from Task 8 and assert the same repo labels, period children, commit count, additions, deletions, and language metrics. Add Clap tests asserting local/GitHub help explicitly says fallback vs subgroup, GitHub default candidates are `repo,language`, and `github fetch ... --groups repo,period` parses. Add integration test `cli_group_and_groups_historical_semantics` and JSON shape tests.

- [ ] **Step 2: Run RED against current local/GitHub divergence**

Run: `cargo test --all-features group_plan_keeps_group_as_fallback_and_groups_as_sublevels`

Expected: FAIL because no shared plan exists and current main deduplicates every subgroup occurrence rather than erroring on non-primary duplicates.

Run: `cargo test --all-features github_explicit_author_group_is_actionable_error`

Expected: FAIL because GitHub accepts author and returns empty `by_author` data.

Run: `cargo test --all-features --test fixture_test cli_group_and_groups_historical_semantics`

Expected: FAIL because help/README/examples contradict implementation and Github lacks `--groups`.

- [ ] **Step 3: Implement source-aware shared planning**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupSource { Local, Github }

#[derive(Debug, Clone, Copy)]
pub struct GroupCardinality {
    pub repo: usize,
    pub author: usize,
    pub period: usize,
    pub language: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    pub primary: GroupBy,
    pub levels: Vec<GroupBy>,
    pub hierarchical: bool,
}

pub fn resolve_group_plan(
    primary_candidates: &[GroupBy],
    subgroups: &[GroupBy],
    counts: &GroupCardinality,
    source: GroupSource,
) -> Result<GroupPlan, String>;
```

Resolver rules are exact: reject unsupported explicit dimensions; choose the first supported candidate with cardinality > 1; use language as final fallback; remove a subgroup only when it equals selected primary; reject every other duplicate; reject language before the final position; drop retained subgroup levels with cardinality <= 1; set `hierarchical = levels.len() > 1`.

Local cardinalities derive from stable repo label, full author identity, `bucket_timestamp`, and file language. GitHub cardinalities/trees derive from exact contributions and support only repo/period/language. GitHub explicit author in either flag returns nonzero before rendering/network reuse work.

- [ ] **Step 4: Wire every format without changing JSON presentation rules**

Add `groups: Vec<GroupBy>` to `GithubFetchArgs`, set GitHub fallback default to `repo,language`, and use the same resolver in local stats and GitHub fetch. Build one `GroupNode` tree for retained multi-level plans. Flat local JSON remains `{periods,totals}`; local hierarchical JSON remains a serialized `Vec<GroupNode>`. GitHub flat JSON preserves `metadata`, `user`, `periods`, `totals`, `summary`; GitHub hierarchical JSON preserves `metadata`, `user`, `summary` and adds `groups: Vec<GroupNode>`.

Add JSON regression asserting `--columns files` does not remove additions/deletions/commits fields. Do not pass presentation columns or number format into JSON functions.

- [ ] **Step 5: Update help and README in the same behavior task**

Use `--group` only for ordered fallback examples and `--groups` for nested examples, for example:

```text
logit stats --group repo,author,language --groups author,period
logit github fetch USER --group repo,language --groups period,language
```

Document source capabilities, unique-level skipping, selected-primary duplicate removal, other duplicate errors, and language-final restriction. Remove “Overrides --group” and stale examples that use `--group` as a tree list.

- [ ] **Step 6: Run GREEN for resolver, surfaces, and JSON compatibility**

Run: `cargo test --all-features group_plan_ -- --nocapture`

Run: `cargo test --all-features github_explicit_author_group_is_actionable_error`

Run: `cargo test --all-features output::json::tests -- --nocapture`

Run: `cargo test --all-features --test fixture_test cli_group_and_groups_historical_semantics`

Expected: all PASS; local/GitHub plans obey identical ordering/cardinality rules, each source's `GroupNode` labels/children/metrics/totals match explicit assertions, unsupported author is nonzero, flat/hierarchical JSON shapes are stable, and columns never trim JSON.

- [ ] **Step 7: Record the task boundary without Git writes**

Record for review: `src/cli.rs`, `src/stats/aggregator.rs`, `src/github/api.rs`, `src/main.rs`, `src/output/json.rs`, `README.md`, `tests/fixture_test.rs`. Do not stage or commit them.

### Task 11: Unify table/TUI semantics with a numeric presentation model

**Depends on:** Task 10

**Files:**
- Create: `src/output/presentation.rs`
- Modify: `src/output/mod.rs:1-6`
- Modify: `src/output/column.rs:1-278`
- Modify: `src/output/table.rs:1-1452`
- Modify: `src/output/tui.rs:1-568`
- Modify: `src/main.rs:181-257,391-438`
- Test: inline presentation/table/TUI tests and `tests/fixture_test.rs`

**Interfaces:**
- Consumes: flat `PeriodStats + totals + primary` or hierarchical `GroupNode + ordered GroupBy levels`, author display/dedup options, sort, selected columns, inline-tree flag.
- Produces: numeric `PresentationModel`; `render_presentation_table(&PresentationModel, NumberFormat, compact) -> String`; `TuiApp::new(PresentationModel, NumberFormat, compact)`; `run_tui(&PresentationModel, NumberFormat, compact) -> anyhow::Result<()>`.

- [ ] **Step 1: Add failing model and TestBackend parity tests**

Define tests against these exact structures:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationRowKind { Group, Language, Total }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentationMetrics {
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
    pub files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationRow {
    pub depth: usize,
    pub label: String,
    pub kind: PresentationRowKind,
    pub metrics: PresentationMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationModel {
    pub label_header: String,
    pub columns: Vec<Column>,
    pub rows: Vec<PresentationRow>,
    pub total: PresentationRow,
    pub inline_tree: bool,
}
```

Add tests `presentation_model_orders_columns_rows_languages_and_total_once`, `tree_author_dimension_controls_dedup_and_email_without_rewriting_repo_labels`, `local_and_github_group_plans_build_equal_presentation_models`, `table_and_tui_testbackend_show_same_semantic_rows`, `tui_multigroup_is_nonempty_and_successful`, and `tui_has_no_fixed_period_or_five_metric_assumption`. Define the presentation fixture and rendering helpers in this same Task 11 test module:

```rust
fn presentation_fixture_nodes() -> Vec<GroupNode> {
    let stats = PeriodStats {
        period_label: "Alice <a@x>".into(),
        by_language: HashMap::from([("Rust".into(), LangStats {
            additions: 5,
            deletions: 1,
            files_changed: 1,
            net_modifications: 5,
            net_additions: 4,
        })]),
        by_author: HashMap::new(),
        total_commits: 1,
        total_additions: 5,
        total_deletions: 1,
        total_net_modifications: 5,
        total_net_additions: 4,
    };
    vec![GroupNode {
        label: "repo-a".into(),
        stats: stats.clone(),
        children: vec![GroupNode {
            label: "Alice <a@x>".into(),
            stats,
            children: vec![],
        }],
    }]
}

fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect::<Vec<_>>().join(" ")
}

fn strip_ansi_for_test(value: &str) -> String {
    regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap().replace_all(value, "").into_owned()
}

let nodes = presentation_fixture_nodes();
let columns = vec![Column::Files, Column::Commits, Column::Net];
let identity_map = HashMap::new();
let model = build_presentation(
    PresentationData::Tree {
        nodes: &nodes,
        levels: &[GroupBy::Repo, GroupBy::Author, GroupBy::Language],
    },
    PresentationOptions {
        columns: &columns,
        sort: Some(&SortBy::Name),
        email_display: &EmailDisplay::Full,
        dedup: &DedupMode::None,
        identity_map: &identity_map,
        inline_tree: true,
    },
);
assert_eq!(model.columns, [Column::Files, Column::Commits, Column::Net]);
assert_eq!(model.rows.iter().map(|r| (&r.label, r.depth)).collect::<Vec<_>>(),
    [("repo-a", 0), ("Alice <a@x>", 1), ("Rust", 2)]);
assert_eq!(model.total.kind, PresentationRowKind::Total);

let table_text = render_presentation_table(&model, NumberFormat::Plain, false);
let mut app = TuiApp::new(model.clone(), NumberFormat::Plain, false);
let backend = ratatui::backend::TestBackend::new(120, 30);
let mut terminal = ratatui::Terminal::new(backend).unwrap();
terminal.draw(|frame| app.render(frame)).unwrap();
let tui_text = buffer_text(terminal.backend().buffer());
for token in ["repo-a", "Alice", "Rust", "Files", "Commits", "Net", "Total"] {
    assert!(strip_ansi_for_test(&table_text).contains(token));
    assert!(tui_text.contains(token));
}
```

Add a dimension-safety regression with a root repository label deliberately shaped like an author identity and two author children sharing one name:

```rust
#[test]
fn tree_author_dimension_controls_dedup_and_email_without_rewriting_repo_labels() {
    let nodes = tree_with_root_and_authors(
        "Alex <repo@example.com>",
        [("Alex <a@example.com>", 1), ("Alex <b@example.com>", 2)],
    );
    let columns = [Column::Commits];
    let identity_map = HashMap::new();
    let model = build_presentation(
        PresentationData::Tree {
            nodes: &nodes,
            levels: &[GroupBy::Repo, GroupBy::Author],
        },
        PresentationOptions {
            columns: &columns,
            sort: Some(&SortBy::Name),
            email_display: &EmailDisplay::None,
            dedup: &DedupMode::Name,
            identity_map: &identity_map,
            inline_tree: false,
        },
    );
    assert_eq!(model.rows[0].label, "Alex <repo@example.com>",
        "repo labels must not be parsed as author identities");
    let author_rows = model.rows.iter().filter(|row| row.depth == 1).collect::<Vec<_>>();
    assert_eq!(author_rows.len(), 1);
    assert_eq!(author_rows[0].label, "Alex");
    assert_eq!(author_rows[0].metrics.commits, 3);
}
```

Define `tree_with_root_and_authors(root: &str, authors: [(&str, u64); 2]) -> Vec<GroupNode>` in this same test module. It returns one root with the supplied literal label and two child nodes whose `PeriodStats.total_commits` are the supplied counts and whose other numeric fields/maps are zero/empty. The test must fail if author parsing is applied by label shape instead of `levels[depth] == GroupBy::Author`.

Add the cross-source parity test only here, after `build_presentation` is the current task's declared output. Construct both inputs with full upstream types—no renderer helper from a later task:

```rust
#[cfg(feature = "github")]
#[test]
fn local_and_github_group_plans_build_equal_presentation_models() {
    let timestamp = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let identity = Author { name: "Alice".into(), email: "a@x".into() };
    let local_commits = vec![CommitStats {
        repo_id: "repo-a-id".into(),
        repo: "repo-a".into(),
        oid: "A".into(),
        author: identity.clone(),
        committer: identity,
        co_authors: vec![],
        timestamp,
        message_subject: "one".into(),
        file_changes: vec![FileChange {
            path: "a.rs".into(), language: Some("Rust".into()),
            additions: 5, deletions: 1, net_modifications: 5, net_additions: 4,
        }],
    }];
    let github_contributions = vec![RepoContribution {
        repo_name: "repo-a".into(),
        total_commits: 1,
        total_additions: 5,
        total_deletions: 1,
        weeks: vec![],
        commits: vec![CommitData {
            oid: Some("A".into()), additions: 5, deletions: 1,
            committed_date: "2025-01-01T00:00:00Z".into(),
        }],
        languages: HashMap::from([("Rust".into(), 1)]),
    }];
    let levels = [GroupBy::Repo, GroupBy::Period, GroupBy::Language];
    let local_nodes = build_group_tree(&local_commits, &levels, &Period::Month, None, None);
    let github_nodes = contributions_to_group_tree(&github_contributions, &levels, &Period::Month);
    let columns = [Column::Commits, Column::Adds, Column::Dels, Column::Files];
    let identity_map = HashMap::new();
    let local_model = build_presentation(
        PresentationData::Tree { nodes: &local_nodes, levels: &levels },
        PresentationOptions {
            columns: &columns, sort: Some(&SortBy::Name),
            email_display: &EmailDisplay::Full, dedup: &DedupMode::None,
            identity_map: &identity_map, inline_tree: true,
        },
    );
    let github_model = build_presentation(
        PresentationData::Tree { nodes: &github_nodes, levels: &levels },
        PresentationOptions {
            columns: &columns, sort: Some(&SortBy::Name),
            email_display: &EmailDisplay::Full, dedup: &DedupMode::None,
            identity_map: &identity_map, inline_tree: true,
        },
    );
    assert_eq!(local_model.rows, github_model.rows);
    assert_eq!(local_model.total, github_model.total);
    assert_eq!(local_model.columns, github_model.columns);
}
```

- [ ] **Step 2: Run RED and prove renderer drift**

Run: `cargo test --all-features table_and_tui_testbackend_show_same_semantic_rows -- --nocapture`

Expected: FAIL because no shared model exists, TUI only accepts flat periods, fixes Period plus five columns, sorts languages differently, and cannot render multi-group trees.

Run: `cargo test --all-features tree_author_dimension_controls_dedup_and_email_without_rewriting_repo_labels -- --nocapture`

Expected: FAIL because the current tree renderer has no ordered dimension metadata and therefore cannot safely distinguish author identities from repo labels with the same textual shape.

Run: `cargo test --all-features local_and_github_group_plans_build_equal_presentation_models -- --nocapture`

Expected: FAIL because Task 10 provides source `GroupNode` trees but no shared numeric presentation conversion yet.

Run: `cargo test --all-features tui_multigroup_is_nonempty_and_successful`

Expected: FAIL because current main prints a warning and exits zero without data.

- [ ] **Step 3: Build renderer-neutral rows before formatting**

Implement:

```rust
pub enum PresentationData<'a> {
    Flat {
        stats: &'a [PeriodStats],
        totals: &'a PeriodStats,
        primary: GroupBy,
    },
    Tree {
        nodes: &'a [GroupNode],
        levels: &'a [GroupBy],
    },
}

pub struct PresentationOptions<'a> {
    pub columns: &'a [Column],
    pub sort: Option<&'a SortBy>,
    pub email_display: &'a EmailDisplay,
    pub dedup: &'a DedupMode,
    pub identity_map: &'a HashMap<String, String>,
    pub inline_tree: bool,
}

pub fn build_presentation(
    data: PresentationData<'_>,
    options: PresentationOptions<'_>,
) -> PresentationModel;
```

Move author dedup/label resolution, group flattening, language-row ordering, requested column order, sort resolution, depth, row kind, and one grand total into this module. In tree mode, require `levels.len()` to cover every non-inline tree depth and apply author parsing/dedup/email only when `levels[depth] == GroupBy::Author`; repo/period labels that happen to contain `<...>` remain literal. Return an error or assert at construction on a levels/depth mismatch rather than guessing dimensions. Keep every metric numeric. `NumberFormat`, ANSI colors, Ratatui `Style`, borders, width/truncation, and compact Adds+Dels combination remain renderer concerns.

- [ ] **Step 4: Make table and TUI consume only the shared model**

Replace renderer-local row construction/sorting with model iteration. Table converts numeric metrics through existing `ColLayout/format_cell`; TUI creates dynamic headers/constraints from `model.columns`, renders group/language/total row kinds, and applies identical `format_num`. Preserve TUI up/down navigation and `t` tree/flat prefix toggle; row count is `model.rows.len() + 1` when nonempty.

Main builds the model once for table or TUI, for flat or hierarchical data. Delete the multi-group TUI warning/success branch. JSON continues directly from domain stats/trees and does not construct a presentation model.

- [ ] **Step 5: Run GREEN for semantic parity and navigation**

Run: `cargo test --all-features output::presentation::tests -- --nocapture`

Run: `cargo test --all-features output::table::tests -- --nocapture`

Run: `cargo test --all-features output::tui::tests -- --nocapture`

Run: `cargo test --all-features table_and_tui_testbackend_show_same_semantic_rows -- --nocapture`

Run: `cargo test --all-features tree_author_dimension_controls_dedup_and_email_without_rewriting_repo_labels -- --nocapture`

Run: `cargo test --all-features tui_multigroup_is_nonempty_and_successful`

Run: `cargo test --all-features local_and_github_group_plans_build_equal_presentation_models -- --nocapture`

Expected: all PASS; ordered levels make nested author dedup/email dimension-safe without rewriting repo labels, equivalent local/GitHub `GroupNode` trees become equal `PresentationModel.rows/total/columns`, labels/depth/order/format/inline rows match table and TUI, TUI handles arbitrary selected columns and group trees, and navigation/toggle tests remain green.

- [ ] **Step 6: Run a real CLI JSON non-trimming regression**

Run: `cargo test --all-features --test fixture_test cli_json_ignores_presentation_columns`

Expected: PASS; output produced with `--columns files --number-format short` still contains complete numeric commit/addition/deletion/net/language data.

- [ ] **Step 7: Record the task boundary without Git writes**

Record for review: `src/output/presentation.rs`, `src/output/mod.rs`, `src/output/column.rs`, `src/output/table.rs`, `src/output/tui.rs`, `src/main.rs`, `tests/fixture_test.rs`. Do not stage or commit them.

### Task 12: Comprehensive verification, real surfaces, and debugging cleanup

**Depends on:** Tasks 1–11

**Files:**
- Delete after extracting no further evidence: `.debug-journal.md`
- Modify only to remove the exact journal exclusion line: `.git/info/exclude`
- Verify: every file listed in the File Map
- Test: complete workspace

**Interfaces:**
- Consumes: completed remediation and all task-level GREEN evidence.
- Produces: formatting/clippy/test/LSP/CLI/TUI/SVG/Action evidence, clean debug artifacts, `git diff --check`, and an expected-scope worktree listing.

- [ ] **Step 1: Run formatter and lint gates**

Run: `cargo fmt --check`

Expected: exit 0 with no formatting diff.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: exit 0 with no warnings.

- [ ] **Step 2: Run the complete regression suite**

Run: `cargo test --all-features`

Expected: exit 0; all original 171 unit + 3 integration tests and every new regression pass. The final count must be greater than the baseline; record Cargo's exact unit/integration counts in the execution report.

- [ ] **Step 3: Exercise real CLI fixture surfaces through `assert_cmd`**

Run: `cargo test --all-features --test fixture_test cli_ -- --nocapture`

Expected: exit 0; this invokes the compiled `logit` binary against `git2`-created temp repos and proves duplicate/collision/repo/committer/language/date/exclude/group/JSON/all-failure behavior without shell Git writes.

- [ ] **Step 4: Exercise TUI, SVG XML, and Action security surfaces**

Run: `cargo test --all-features table_and_tui_testbackend_show_same_semantic_rows -- --nocapture`

Expected: PASS with a nonempty Ratatui `TestBackend` buffer containing selected headers, tree rows, language rows, and total.

Run: `cargo test --all-features --test github_svg_test -- --nocapture`

Expected: PASS; exact entity assertions succeed and no injected element/attribute exists. The conditional parser test prints `DOTNET_XML_PARSED` whenever `pwsh` is available.

Current Windows acceptance must prove that parser branch actually ran. Execute in PowerShell:

```powershell
$pwshCommand = Get-Command "pwsh" -ErrorAction Stop
if (-not $pwshCommand) { throw "pwsh is required for Windows SVG XML acceptance" }
cargo test --all-features --test github_svg_test cli_generated_svg_is_well_formed_xml_when_pwsh_available -- --nocapture
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Expected: exit 0 and test output contains `DOTNET_XML_PARSED`; a skip message on this Windows workspace is a failed acceptance gate.

Run: `cargo test --all-features --test action_test -- --nocapture`

Expected: PASS; static assertions always execute, and Bash-capable environments additionally prove stub argv/retry/final-status behavior.

- [ ] **Step 5: Run LSP diagnostics on every changed Rust surface**

Use `lsp_diagnostics` with severity `all` on:

```text
src/main.rs
src/cli.rs
src/scanner.rs
src/exclude.rs
src/analyze.rs
src/filter.rs
src/git/repo.rs
src/git/diff.rs
src/git/author.rs
src/stats/models.rs
src/stats/aggregator.rs
src/output/presentation.rs
src/output/column.rs
src/output/table.rs
src/output/tui.rs
src/output/json.rs
src/github/cache.rs
src/github/api.rs
src/github/svg.rs
tests/action_test.rs
tests/github_svg_test.rs
tests/fixture_test.rs
tests/common/mod.rs
```

Expected: “No diagnostics published by the language server” for every file; any diagnostic blocks completion.

- [ ] **Step 6: Remove the debugging journal and restore the local exclude file**

Read `.debug-journal.md` one final time only if an unresolved verification fact still depends on it. Delete exactly `.debug-journal.md`. Edit `.git/info/exclude` to remove only the exact line `.debug-journal.md`, leaving all other local exclusions byte-for-byte unchanged.

Verify in PowerShell:

```powershell
if (Test-Path -LiteralPath ".debug-journal.md") { throw ".debug-journal.md still exists" }
rg -n --fixed-strings ".debug-journal.md" ".git/info/exclude"
if ($LASTEXITCODE -eq 0) { throw ".git/info/exclude still contains the journal line" }
if ($LASTEXITCODE -ne 1) { exit $LASTEXITCODE }
```

Expected: no journal file and no matching exclude line.

- [ ] **Step 7: Check whitespace and exact worktree scope without Git writes**

Run: `git diff --check`

Expected: exit 0 with no whitespace errors.

Run: `git status --short`

Run: `git diff --name-only -- action.yml README.md src tests`

Run: `git ls-files --others --exclude-standard`

Expected scope: only the spec/plan artifacts plus files enumerated in this plan's File Map; `.debug-journal.md` is absent; `.git/info/exclude` no longer contains its temporary line. Investigate and remove any generated SVG, cache, temp fixture, log, or unplanned source file. Do not stage or commit.

- [ ] **Step 8: Produce the execution evidence handoff**

Report each command and exit result, exact final test counts, LSP result, real-surface test names, changed/untracked file list, and any residual warning. Completion requires every gate above to pass; a skipped Bash stub test is acceptable only when the always-on Action static tests pass and the report explicitly states Bash was unavailable. The SVG parser subtest may skip only on platforms where `pwsh` is absent; on the current Windows workspace it must print `DOTNET_XML_PARSED`.

## Design Requirement Coverage Matrix

| Approved requirement / confirmed defect | Implementing task(s) | Failing-first evidence |
|---|---|---|
| P0 Action shell injection and final failure propagation | 1, 12 | Action run-block static assertions; literal argv/retry/status Bash stub |
| P0 SVG/XML injection | 2, 12 | Exact profile/multi/JSON-input entity assertions plus conditional `pwsh` .NET `[xml]` parse; Windows acceptance requires parser execution |
| P0 GitHub history cache cross-user contamination | 4 | schema-v2 key comparison and same-repo/two-user cache-flow test |
| Group/help/README historical contract | 10 | resolver fallback/subgroup tests, Clap help, CLI regression |
| Shared local/GitHub group keys/order/metrics/totals | 10, 11 | Task 10 explicit `GroupPlan`/`GroupNode` tests and Task 11 equal `PresentationModel` parity |
| Multi-group TUI empty success | 11, 12 | nonempty TestBackend multi-group test |
| GitHub fake author support | 10 | actionable nonzero unsupported-author test |
| `--repo` not wired | 5 | pre-analysis selector CLI test |
| `--committer` not wired | 6 | distinct author/committer name/email CLI test |
| Author email lost; none/name/remote and show-email semantics | 6, 11 | full-identity aggregation plus dimension-tagged nested-author dedup/label/total tests |
| Same basename merge, duplicate/overlapping paths | 5 | collision labels and duplicate totals CLI fixtures |
| Date-only until, reversed/invalid/non-finite ranges | 3, 7 | `TimeRange` unit tests and 23:59/next-midnight Git tests |
| Invalid exclude qualifier/malformed/empty group | 3 | `parse_many -> Result` RED cases and CLI nonzero behavior |
| Merge churn, rename detection, binary file retention | 7 | dedicated git2 merge/rename/binary fixtures |
| Language filter commit semantics | 6 | no-matching-file commit disappears from aggregate |
| GitHub contribution summary lost on cache hit | 4 | completed-window roundtrip with nonzero summary |
| GitHub day/month Monday normalization | 8 | Jan/Feb exact commit boundary buckets |
| Per-repository batch window leakage | 8 | per-alias variables and post-fetch per-repo filter test |
| RFC3339 lexical comparison | 8 | equivalent offset instant retained at inclusive since plus exact `until_exclusive` exclusion |
| Distinct commits collapsed without stable identity | 8 | OID dedup and missing-OID retention test |
| HTTP timeout and transient retry gaps | 9 | delayed TCP timeout with wall-clock/request bound plus 408/429/503/transport retry and exhaustion tests |
| REST query encoding/retry practicality | 9 | REST 503→success and bounded exhaustion plus `%2B`/`%40`, `%20`-or-`+` space, and exact decoded semantic value |
| Missing cursor and API caps invisible | 9 | pagination error and capped decision-message tests |
| Cache init/read/parse/write failures silent | 4 | malformed cache error plus warning-and-fresh-fetch paths |
| All-repository analysis failure returns success | 3 | real CLI repo that opens but has a deleted HEAD object: one analyze error, nonzero, no `No commits found` |
| Table/TUI columns/sort/format/inline/totals drift | 11 | shared numeric model and table/TestBackend semantic assertions |
| JSON trimmed or reshaped by presentation options | 10, 11 | JSON shape and `--columns files --number-format short` regression |
| Full final quality and cleanup boundary | 12 | fmt, clippy, all-features, LSP, CLI/TUI/XML/Action, journal cleanup, diff/scope checks |

## Self-Review Receipt

- Spec coverage: every P0, P1, directly related P2, error-handling rule, and verification surface maps to at least one row above.
- Dependency consistency: Wave 1 is file-independent; local shared files serialize through Tasks 3→5→6→7→10→11; GitHub API serializes through Tasks 4→8→9→10; final verification depends on all tasks.
- Interface consistency: `until_exclusive`, `RepoInput`, full author keys, schema-v2 cache, exact `CommitData`, `GroupPlan`, source `GroupNode` builders, and level-tagged `PresentationModel` input are introduced before downstream consumers; Task 4 does not use Task 8's OID, and Task 10 contains no Task 11 symbol.
- Round-1 blocker ledger resolved: Task 11 tree input now carries ordered dimensions and tests nested author-only dedup; Task 9 proves timeout plus REST transient success/exhaustion; Task 5/File Map/final LSP include every current `CommitStats` literal site.
- Round-2 regression resolved: Task 5 validates inline tests with the repository's actual binary target via `cargo test --all-features --bin logit -- --nocapture`; the project has no library target.
- Dependency policy: no task changes dependency manifests or adds/installs a package; SVG parser coverage uses existing `pwsh`/.NET and exact entity assertions.
- Placeholder scan: no implementation placeholders or unspecified error-handling steps remain; every RED names an assertion/current failure and every GREEN gives an executable PowerShell-safe command.
- Git policy: no task contains staging, commit, push, tag, or other Git write commands.

**Plan review receipt status:** waiting for orchestrator-owned current-revision plan-critic receipt.
