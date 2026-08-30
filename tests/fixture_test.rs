mod common;
use std::{fs, process::Command};

use assert_cmd::cargo::CommandCargoExt;
use serde_json::Value;
use tempfile::TempDir;

fn run_stats_json(paths: &[&std::path::Path], selectors: &[&str]) -> std::process::Output {
    let mut command = Command::cargo_bin("logit").expect("locate logit binary");
    command.arg("stats");
    command.args(paths);
    command.arg("--format").arg("json");
    for selector in selectors {
        command.arg("--repo").arg(selector);
    }
    command.output().expect("run logit stats")
}

#[cfg(feature = "github")]
#[test]
fn github_multi_rejects_out_of_range_period_before_token_check() {
    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .args(["github", "multi", "octocat", "--periods", "1000000000d"])
        .env_remove("GITHUB_TOKEN")
        .output()
        .expect("run logit github multi");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "logit github multi unexpectedly succeeded\nstderr: {stderr}"
    );
    assert!(stderr.contains("1000000000d"), "stderr: {stderr}");
    assert!(
        stderr.contains("range") && stderr.contains("too large"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("GITHUB_TOKEN"), "stderr: {stderr}");
}

#[cfg(feature = "github")]
#[test]
fn github_future_since_is_rejected_before_token_or_network() {
    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .args(["github", "fetch", "octocat", "--since", "2999-01-01"])
        .env_remove("GITHUB_TOKEN")
        .output()
        .expect("run logit github fetch");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("--since") && stderr.contains("future"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("GITHUB_TOKEN"), "stderr: {stderr}");
}

#[cfg(feature = "github")]
#[test]
fn github_no_cache_refresh_conflict_warns_exactly_once() {
    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .args([
            "github",
            "fetch",
            "octocat",
            "--no-cache",
            "--refresh-cache",
        ])
        .env_remove("GITHUB_TOKEN")
        .output()
        .expect("run logit github fetch");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let warning = "Warning: --no-cache overrides --refresh-cache; cache is disabled";

    assert!(!output.status.success(), "stderr: {stderr}");
    assert_eq!(stderr.matches(warning).count(), 1, "stderr: {stderr}");
}

fn successful_stats_json(paths: &[&std::path::Path], selectors: &[&str]) -> Value {
    let output = run_stats_json(paths, selectors);
    assert!(
        output.status.success(),
        "logit stats failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stats JSON output")
}

fn append_commit_with_committer(repo: &git2::Repository, committer_name: &str) {
    let parent = repo
        .head()
        .expect("fixture HEAD")
        .peel_to_commit()
        .expect("fixture HEAD commit");
    let parent_tree = parent.tree().expect("fixture parent tree");
    let blob = repo.blob(b"filtered commit\n").expect("filtered blob");
    let mut tree_builder = repo
        .treebuilder(Some(&parent_tree))
        .expect("filtered tree builder");
    tree_builder
        .insert("filtered.txt", blob, 0o100644)
        .expect("insert filtered file");
    let tree = repo
        .find_tree(tree_builder.write().expect("write filtered tree"))
        .expect("find filtered tree");
    let author = git2::Signature::new(
        "Filter Author",
        "filter-author@test.com",
        &git2::Time::new(1_709_000_000, 0),
    )
    .expect("filtered author signature");
    let committer = git2::Signature::new(
        committer_name,
        "matching-committer@test.com",
        &git2::Time::new(1_709_000_000, 0),
    )
    .expect("filtered committer signature");
    repo.commit(
        Some("HEAD"),
        &author,
        &committer,
        "Add filtered commit",
        &tree,
        &[&parent],
    )
    .expect("commit filtered fixture change");
}

#[test]
fn test_fixture_creates_five_commits() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());

    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    let count = revwalk.count();
    assert_eq!(count, 5, "Expected 5 commits, got {count}");
}

#[test]
fn test_fixture_authors() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());

    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .unwrap();

    let commits: Vec<_> = revwalk
        .map(|oid| repo.find_commit(oid.unwrap()).unwrap())
        .collect();

    assert_eq!(commits[0].author().name().unwrap(), "Alice");
    assert_eq!(commits[0].author().email().unwrap(), "alice@test.com");
    assert_eq!(commits[1].author().name().unwrap(), "Bob");
    assert_eq!(commits[1].author().email().unwrap(), "bob@test.com");
    assert_eq!(commits[2].author().name().unwrap(), "Alice");
}

#[test]
fn test_fixture_co_author_trailer() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());

    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::REVERSE)
        .unwrap();

    let commits: Vec<_> = revwalk
        .map(|oid| repo.find_commit(oid.unwrap()).unwrap())
        .collect();

    let msg = commits[2].message().unwrap();
    assert!(msg.contains("Co-authored-by: Charlie"));
}

#[test]
fn cli_all_repositories_failed_returns_nonzero_once() {
    let tmp = TempDir::new().unwrap();
    let repo = common::create_test_repo(tmp.path());
    let head = repo.head().unwrap().target().unwrap();
    drop(repo);

    let head_hex = head.to_string();
    let object_path = tmp
        .path()
        .join(".git")
        .join("objects")
        .join(&head_hex[..2])
        .join(&head_hex[2..]);
    fs::remove_file(&object_path).expect("remove HEAD loose object");
    assert!(git2::Repository::open(tmp.path()).is_ok());

    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .output()
        .expect("run logit stats");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "logit stats unexpectedly succeeded\nstderr: {stderr}"
    );
    assert_eq!(
        stderr.matches("failed to analyze").count(),
        1,
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("No commits found"), "stderr: {stderr}");
}

#[test]
fn cli_committer_matches_name_and_email() {
    let tmp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(tmp.path());

    for pattern in ["Release Bot", "release-bot@test.com"] {
        let output = Command::cargo_bin("logit")
            .expect("locate logit binary")
            .arg("stats")
            .arg(tmp.path())
            .arg("--committer")
            .arg(pattern)
            .arg("--format")
            .arg("json")
            .output()
            .expect("run logit stats");
        assert!(
            output.status.success(),
            "logit stats failed for {pattern}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).expect("stats JSON output");

        assert_eq!(json["totals"]["total_commits"], 1, "pattern: {pattern}");
    }
}

#[test]
fn cli_filter_empty_diagnostic_is_not_no_commits_diagnostic() {
    let tmp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(tmp.path());

    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .args([
            "stats",
            tmp.path().to_str().expect("UTF-8 temporary path"),
            "--committer",
            "does-not-exist",
            "--format",
            "json",
        ])
        .output()
        .expect("run filtered stats");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    assert!(stderr.contains("commits exist"), "stderr: {stderr}");
    assert!(stderr.contains("filters"), "stderr: {stderr}");
    assert!(
        !stderr.contains("No commits found in the given period."),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_partial_committer_filter_reports_matching_filters_for_skipped_repositories() {
    let tmp = TempDir::new().unwrap();
    let matching_path = tmp.path().join("matching");
    let skipped_path = tmp.path().join("skipped");
    fs::create_dir_all(&matching_path).expect("create matching repository parent");
    fs::create_dir_all(&skipped_path).expect("create skipped repository parent");
    let matching_repo = common::create_test_repo(&matching_path);
    let _skipped_repo = common::create_test_repo(&skipped_path);
    append_commit_with_committer(&matching_repo, "Matching Bot");

    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .args([
            "--committer",
            "Matching Bot",
            "--group",
            "repo",
            "--format",
            "json",
        ])
        .output()
        .expect("run partially filtered stats");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    let json: Value = serde_json::from_slice(&output.stdout).expect("stats JSON output");
    assert_eq!(json["totals"]["total_commits"], 1);
    let periods = json["periods"].as_array().expect("repo periods array");
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0]["period_label"], "matching");
    assert!(stderr.contains("matching"), "stderr: {stderr}");
    assert!(stderr.contains("filters"), "stderr: {stderr}");
    assert!(
        !stderr.contains("no activity in the period"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_show_email_full_displays_history_email_without_changing_totals() {
    let tmp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(tmp.path());

    let baseline = successful_stats_json(&[tmp.path()], &[]);
    let full_json = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .arg("--show-email")
        .arg("full")
        .output()
        .expect("run logit stats");
    assert!(
        full_json.status.success(),
        "logit stats failed\nstderr: {}",
        String::from_utf8_lossy(&full_json.stderr)
    );
    let full_json: Value = serde_json::from_slice(&full_json.stdout).expect("stats JSON output");
    assert_eq!(full_json["totals"], baseline["totals"]);

    let table = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .arg("--group")
        .arg("author")
        .arg("--show-email")
        .arg("full")
        .output()
        .expect("run logit stats");
    assert!(
        table.status.success(),
        "logit stats failed\nstderr: {}",
        String::from_utf8_lossy(&table.stderr)
    );
    let table = String::from_utf8(table.stdout).expect("table output");
    assert!(table.contains("Alice <alice@test.com>"), "table: {table}");
    assert!(table.contains("Bob <bob@test.com>"), "table: {table}");
}

#[test]
fn cli_duplicate_and_overlapping_paths_do_not_change_totals() {
    let tmp = TempDir::new().unwrap();
    let repo_path = tmp.path().join("repos").join("service");
    fs::create_dir_all(&repo_path).expect("create repo parent");
    let _repo = common::create_test_repo(&repo_path);

    let baseline = successful_stats_json(&[&repo_path], &[]);
    let overlapping = successful_stats_json(&[tmp.path(), &repo_path, &repo_path], &[]);

    assert_eq!(overlapping["totals"], baseline["totals"]);
}

#[test]
fn cli_same_basename_repositories_have_distinct_shortest_labels() {
    let tmp = TempDir::new().unwrap();
    let left = tmp.path().join("left").join("service");
    let right = tmp.path().join("right").join("service");
    fs::create_dir_all(&left).expect("create left repo parent");
    fs::create_dir_all(&right).expect("create right repo parent");
    let _left_repo = common::create_test_repo(&left);
    let _right_repo = common::create_test_repo(&right);

    let mut command = Command::cargo_bin("logit").expect("locate logit binary");
    let output = command
        .arg("stats")
        .arg(tmp.path())
        .arg("--group")
        .arg("repo")
        .arg("--format")
        .arg("json")
        .output()
        .expect("run logit stats");
    assert!(
        output.status.success(),
        "logit stats failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("stats JSON output");
    let labels: Vec<&str> = json["periods"]
        .as_array()
        .expect("periods array")
        .iter()
        .map(|period| period["period_label"].as_str().expect("repo label"))
        .collect();

    assert_eq!(labels, ["left/service", "right/service"]);
}

#[test]
fn cli_repo_selector_is_applied_before_analysis() {
    let tmp = TempDir::new().unwrap();
    let selected = tmp.path().join("left").join("selected");
    let skipped = tmp.path().join("right").join("skipped");
    fs::create_dir_all(&selected).expect("create selected repo parent");
    fs::create_dir_all(&skipped).expect("create skipped repo parent");
    let _selected_repo = common::create_test_repo(&selected);
    let _skipped_repo = common::create_test_repo(&skipped);

    let mut command = Command::cargo_bin("logit").expect("locate logit binary");
    let output = command
        .arg("stats")
        .arg(tmp.path())
        .arg("--repo")
        .arg("selected")
        .arg("--group")
        .arg("repo")
        .arg("--format")
        .arg("json")
        .output()
        .expect("run logit stats");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        !stderr.contains("failed to analyze"),
        "unselected repository was analyzed\nstderr: {stderr}"
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("stats JSON output");
    assert_eq!(json["totals"]["total_commits"], 5);
    // A single explicit non-language primary remains visible even when the
    // selector leaves exactly one repository.
    assert_eq!(json["periods"].as_array().map(Vec::len), Some(1));
}

#[test]
fn cli_ambiguous_bare_repo_selector_lists_distinguishing_labels() {
    let tmp = TempDir::new().unwrap();
    let left = tmp.path().join("left").join("service");
    let right = tmp.path().join("right").join("service");
    fs::create_dir_all(&left).expect("create left repo parent");
    fs::create_dir_all(&right).expect("create right repo parent");
    let _left_repo = common::create_test_repo(&left);
    let _right_repo = common::create_test_repo(&right);

    let output = run_stats_json(&[tmp.path()], &["service"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "ambiguous selector unexpectedly succeeded"
    );
    assert!(stderr.contains("left/service"), "stderr: {stderr}");
    assert!(stderr.contains("right/service"), "stderr: {stderr}");
}

#[test]
fn cli_repo_selector_accepts_exact_normalized_identity_and_unique_basename() {
    let tmp = TempDir::new().unwrap();
    let left = tmp.path().join("left").join("service");
    let right = tmp.path().join("right").join("service");
    let unique = tmp.path().join("unique");
    fs::create_dir_all(&left).expect("create left repo parent");
    fs::create_dir_all(&right).expect("create right repo parent");
    fs::create_dir_all(&unique).expect("create unique repo parent");
    let _left_repo = common::create_test_repo(&left);
    let _right_repo = common::create_test_repo(&right);
    let _unique_repo = common::create_test_repo(&unique);

    let normalized_id = left
        .canonicalize()
        .expect("canonical left repository")
        .to_string_lossy()
        .replace('\\', "/");
    let selected_by_id = successful_stats_json(&[tmp.path()], &[&normalized_id]);
    assert_eq!(selected_by_id["totals"]["total_commits"], 5);

    let selected_by_label = successful_stats_json(&[tmp.path()], &["left/service"]);
    assert_eq!(selected_by_label["totals"]["total_commits"], 5);

    let selected_by_basename = successful_stats_json(&[tmp.path()], &["unique"]);
    assert_eq!(selected_by_basename["totals"]["total_commits"], 5);
}

#[cfg(windows)]
#[test]
fn cli_repo_selector_matches_display_label_case_insensitively_on_windows() {
    let tmp = TempDir::new().unwrap();
    let team = tmp.path().join("Team").join("Service");
    let other = tmp.path().join("Other").join("Service");
    fs::create_dir_all(&team).expect("create Team/Service parent");
    fs::create_dir_all(&other).expect("create Other/Service parent");
    let _team_repo = common::create_test_repo(&team);
    let _other_repo = common::create_test_repo(&other);

    let selected = successful_stats_json(&[tmp.path()], &["team/service"]);
    assert_eq!(selected["totals"]["total_commits"], 5);
}

#[test]
fn cli_group_and_groups_historical_semantics() {
    let tmp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(tmp.path());

    let flat = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .arg("--group")
        .arg("repo,author,language")
        .arg("--columns")
        .arg("files")
        .output()
        .expect("run flat stats");
    assert!(
        flat.status.success(),
        "flat stats failed: {}",
        String::from_utf8_lossy(&flat.stderr)
    );
    let flat: Value = serde_json::from_slice(&flat.stdout).expect("flat JSON output");
    assert!(flat.is_object());
    assert!(flat["periods"].is_array());
    assert!(flat.get("totals").is_some());
    assert!(flat["totals"].get("total_commits").is_some());
    assert!(flat["totals"].get("total_additions").is_some());
    assert!(flat["totals"].get("total_deletions").is_some());

    let tree = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .arg("--group")
        .arg("repo,author,language")
        .arg("--groups")
        .arg("author,period")
        .output()
        .expect("run grouped stats");
    assert!(
        tree.status.success(),
        "grouped stats failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree: Value = serde_json::from_slice(&tree.stdout).expect("tree JSON output");
    let groups = tree.as_array().expect("hierarchical groups array");
    assert_eq!(groups[0]["label"], "Alice <alice@test.com>");
    assert!(groups[0]["children"].is_array());
    assert!(
        groups[0]["children"].as_array().unwrap()[0]["label"]
            .as_str()
            .unwrap()
            .starts_with("2024-")
    );

    for args in [
        vec!["--groups", "period,period"],
        vec!["--groups", "language,period"],
        vec!["--group", "repo,repo"],
    ] {
        let output = Command::cargo_bin("logit")
            .expect("locate logit binary")
            .arg("stats")
            .arg(tmp.path())
            .args(&args)
            .output()
            .expect("run invalid grouping");
        assert!(
            !output.status.success(),
            "invalid grouping unexpectedly succeeded: {args:?}"
        );
    }

    #[cfg(feature = "github")]
    {
        let output = Command::cargo_bin("logit")
            .expect("locate logit binary")
            .args(["github", "fetch", "octocat", "--group", "author"])
            .output()
            .expect("run invalid GitHub grouping");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stderr: {stderr}");
        assert!(stderr.contains("author"), "stderr: {stderr}");
        assert!(
            stderr.contains("GitHub contribution records have no author identity"),
            "stderr: {stderr}"
        );
    }
}

#[test]
fn cli_json_ignores_presentation_columns() {
    let tmp = TempDir::new().unwrap();
    let _repo = common::create_test_repo(tmp.path());
    let output = Command::cargo_bin("logit")
        .expect("locate logit binary")
        .arg("stats")
        .arg(tmp.path())
        .arg("--format")
        .arg("json")
        .arg("--columns")
        .arg("files")
        .arg("--number-format")
        .arg("short")
        .output()
        .expect("run JSON stats with presentation-only options");
    assert!(
        output.status.success(),
        "JSON stats failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("stats JSON output");
    let totals = &json["totals"];
    for field in [
        "total_commits",
        "total_additions",
        "total_deletions",
        "total_net_modifications",
        "total_net_additions",
    ] {
        assert!(totals[field].is_u64(), "{field} was not numeric: {totals}");
    }
    let languages = totals["by_language"]
        .as_object()
        .expect("complete language breakdown");
    assert!(!languages.is_empty());
    for language in languages.values() {
        for field in ["additions", "deletions", "files_changed"] {
            assert!(
                language[field].is_u64(),
                "language {field} was not numeric: {language}"
            );
        }
    }
}
