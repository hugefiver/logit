use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rayon::prelude::*;

use crate::git::author::extract_co_authors;
use crate::git::diff::analyze_commit_diff;
use crate::git::repo::RepoAnalyzer;
use crate::lang::apply_language_to_changes;
use crate::stats::models::CommitStats;

#[derive(Debug, Clone)]
pub struct RepoInput {
    pub path: PathBuf,
    pub id: String,
    pub label: String,
}

/// Canonicalize, deduplicate, label, and optionally select repository inputs.
pub fn normalize_repo_inputs(
    paths: Vec<PathBuf>,
    selectors: Option<&[String]>,
) -> anyhow::Result<Vec<RepoInput>> {
    let mut repos: Vec<RepoInput> = paths
        .into_iter()
        .map(|path| {
            let path = normalize_repo_path(&path);
            let id = repo_id_for_path(&path);
            RepoInput {
                path,
                id,
                label: String::new(),
            }
        })
        .collect();

    repos.sort_by(|left, right| {
        platform_repo_key(&left.id, cfg!(windows))
            .cmp(&platform_repo_key(&right.id, cfg!(windows)))
            .then_with(|| left.id.cmp(&right.id))
    });
    repos.dedup_by(|left, right| platform_repo_eq(&left.id, &right.id, cfg!(windows)));
    assign_repo_labels(&mut repos);

    let Some(selectors) = selectors else {
        return Ok(repos);
    };

    let mut selected_ids = HashSet::new();
    for selector in selectors {
        let normalized_selector = platform_repo_key(selector, cfg!(windows));
        if let Some(repo) = repos.iter().find(|repo| {
            platform_repo_eq(&repo.label, &normalized_selector, cfg!(windows))
                || platform_repo_eq(&repo.id, &normalized_selector, cfg!(windows))
        }) {
            selected_ids.insert(platform_repo_key(&repo.id, cfg!(windows)));
            continue;
        }

        if !normalized_selector.contains('/') {
            let matches: Vec<&RepoInput> = repos
                .iter()
                .filter(|repo| {
                    platform_repo_eq(repo_basename(&repo.id), &normalized_selector, cfg!(windows))
                })
                .collect();
            match matches.as_slice() {
                [repo] => {
                    selected_ids.insert(platform_repo_key(&repo.id, cfg!(windows)));
                    continue;
                }
                [] => {}
                _ => {
                    let labels = matches
                        .iter()
                        .map(|repo| repo.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    anyhow::bail!(
                        "repository selector '{selector}' is ambiguous; use one of: {labels}"
                    );
                }
            }
        }

        let labels = repos
            .iter()
            .map(|repo| repo.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("repository selector '{selector}' did not match; available: {labels}");
    }

    Ok(repos
        .into_iter()
        .filter(|repo| selected_ids.contains(&platform_repo_key(&repo.id, cfg!(windows))))
        .collect())
}

fn normalize_repo_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if !normalized.pop() {
                        normalized.push(component.as_os_str());
                    }
                }
                _ => normalized.push(component.as_os_str()),
            }
        }
        if normalized.as_os_str().is_empty() {
            path.to_path_buf()
        } else {
            normalized
        }
    })
}

fn repo_id_for_path(path: &Path) -> String {
    let id = path.to_string_lossy().replace('\\', "/");
    if id.is_empty() { ".".to_string() } else { id }
}

/// Normalize a repository identity for a local platform match.
pub fn platform_repo_key(value: &str, windows: bool) -> String {
    let normalized = value.replace('\\', "/");
    if windows {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

/// Compare repository identities using a local platform's path semantics.
pub fn platform_repo_eq(left: &str, right: &str, windows: bool) -> bool {
    platform_repo_key(left, windows) == platform_repo_key(right, windows)
}

fn repo_basename(id: &str) -> &str {
    id.rsplit('/')
        .find(|component| !component.is_empty())
        .unwrap_or(id)
}

fn suffix_label(id: &str, depth: usize) -> String {
    let components: Vec<&str> = id
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    if depth >= components.len() {
        return id.to_string();
    }
    let start = components.len().saturating_sub(depth);
    components[start..].join("/")
}

fn assign_repo_labels(repos: &mut [RepoInput]) {
    for index in 0..repos.len() {
        let basename = repo_basename(&repos[index].id);
        let matching_basenames = repos
            .iter()
            .filter(|repo| platform_repo_eq(repo_basename(&repo.id), basename, cfg!(windows)))
            .count();

        if matching_basenames == 1 {
            repos[index].label = basename.to_string();
            continue;
        }

        let mut depth = 2;
        loop {
            let label = suffix_label(&repos[index].id, depth);
            let collides = repos.iter().enumerate().any(|(other_index, repo)| {
                other_index != index
                    && platform_repo_eq(repo_basename(&repo.id), basename, cfg!(windows))
                    && platform_repo_eq(&suffix_label(&repo.id, depth), &label, cfg!(windows))
            });
            if !collides {
                repos[index].label = label;
                break;
            }
            depth += 1;
        }
    }
}

/// Error from analyzing a single repository.
pub struct RepoError {
    pub path: PathBuf,
    pub error: String,
}

/// Analyze multiple repos in parallel.
///
/// Returns `(all_commits, repo_errors)`.
/// Each rayon task opens its own `Repository` — `git2::Repository` is not `Send`,
/// so the parallel iterator receives only `RepoInput` values, not repository handles.
pub fn analyze_repos(
    repos: &[RepoInput],
    since: Option<DateTime<Utc>>,
    until_exclusive: Option<DateTime<Utc>>,
) -> (Vec<CommitStats>, Vec<RepoError>) {
    let results: Vec<Result<Vec<CommitStats>, RepoError>> = repos
        .par_iter()
        .map(|repo| analyze_single_repo(repo, since, until_exclusive))
        .collect();

    let mut all_commits = Vec::new();
    let mut errors = Vec::new();

    for result in results {
        match result {
            Ok(commits) => all_commits.extend(commits),
            Err(e) => errors.push(e),
        }
    }

    (all_commits, errors)
}

/// Analyze a single repository. Opens its own `Repository` handle.
fn analyze_single_repo(
    input: &RepoInput,
    since: Option<DateTime<Utc>>,
    until_exclusive: Option<DateTime<Utc>>,
) -> Result<Vec<CommitStats>, RepoError> {
    let analyzer = RepoAnalyzer::open(&input.path).map_err(|e| RepoError {
        path: input.path.clone(),
        error: format!("{e:#}"),
    })?;

    let commit_infos = analyzer
        .walk_commits(since, until_exclusive)
        .map_err(|e| RepoError {
            path: input.path.clone(),
            error: format!("{e:#}"),
        })?;

    let repo = analyzer.repo();

    let mut stats = Vec::with_capacity(commit_infos.len());

    for ci in &commit_infos {
        let commit = repo.find_commit(ci.oid).map_err(|e| RepoError {
            path: input.path.clone(),
            error: format!("Failed to find commit {}: {e:#}", ci.oid),
        })?;

        let mut file_changes = analyze_commit_diff(repo, &commit).map_err(|e| RepoError {
            path: input.path.clone(),
            error: format!("Failed to analyze diff for {}: {e:#}", ci.oid),
        })?;

        apply_language_to_changes(&mut file_changes);

        let co_authors = extract_co_authors(&ci.message, &ci.author);
        let message_subject = ci.message.lines().next().unwrap_or("").to_string();

        stats.push(CommitStats {
            repo_id: input.id.clone(),
            repo: input.label.clone(),
            oid: format!("{}", ci.oid),
            author: ci.author.clone(),
            committer: ci.committer.clone(),
            co_authors,
            timestamp: ci.timestamp,
            message_subject,
            file_changes,
        });
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature, Time};
    use tempfile::TempDir;

    use crate::cli::Period;
    use crate::stats::aggregator::aggregate_commits;

    fn create_test_repo(dir: &std::path::Path, file_name: &str, content: &str, msg: &str) {
        let repo = Repository::init(dir).unwrap();
        let sig =
            Signature::new("TestUser", "test@example.com", &Time::new(1_705_312_800, 0)).unwrap();
        let blob = repo.blob(content.as_bytes()).unwrap();
        let tree_oid = {
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert(file_name, blob, 0o100644).unwrap();
            tb.write().unwrap()
        };
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[])
            .unwrap();
    }

    fn create_test_repo_two_commits(dir: &std::path::Path) {
        let repo = Repository::init(dir).unwrap();
        let sig1 =
            Signature::new("Alice", "alice@example.com", &Time::new(1_705_312_800, 0)).unwrap();
        let blob1 = repo.blob(b"hello\n").unwrap();
        let tree_oid1 = {
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("file.rs", blob1, 0o100644).unwrap();
            tb.write().unwrap()
        };
        let tree1 = repo.find_tree(tree_oid1).unwrap();
        let oid1 = repo
            .commit(Some("HEAD"), &sig1, &sig1, "Initial commit", &tree1, &[])
            .unwrap();

        let sig2 = Signature::new("Bob", "bob@example.com", &Time::new(1_705_413_600, 0)).unwrap();
        let parent = repo.find_commit(oid1).unwrap();
        let blob2 = repo.blob(b"world\n").unwrap();
        let tree_oid2 = {
            let mut tb = repo.treebuilder(Some(&tree1)).unwrap();
            tb.insert("file2.py", blob2, 0o100644).unwrap();
            tb.write().unwrap()
        };
        let tree2 = repo.find_tree(tree_oid2).unwrap();
        repo.commit(
            Some("HEAD"),
            &sig2,
            &sig2,
            "Second commit",
            &tree2,
            &[&parent],
        )
        .unwrap();
    }

    #[test]
    fn test_analyze_single_repo() {
        let tmp = TempDir::new().unwrap();
        create_test_repo_two_commits(tmp.path());

        let repos = normalize_repo_inputs(vec![tmp.path().to_path_buf()], None).unwrap();
        let (commits, errors) = analyze_repos(&repos, None, None);

        assert!(
            errors.is_empty(),
            "expected no errors: {errors:?}",
            errors = errors.iter().map(|e| &e.error).collect::<Vec<_>>()
        );
        assert_eq!(commits.len(), 2);

        assert_eq!(commits[0].author.name, "Alice");
        assert_eq!(commits[0].message_subject, "Initial commit");
        assert!(!commits[0].file_changes.is_empty());

        assert_eq!(commits[1].author.name, "Bob");
        assert_eq!(commits[1].message_subject, "Second commit");

        let expected_name = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(commits[0].repo, expected_name);
        assert_eq!(commits[0].repo_id, repos[0].id);
    }

    #[test]
    fn test_analyze_multiple_repos() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();

        create_test_repo(tmp1.path(), "main.rs", "fn main() {}\n", "Repo1 commit");
        create_test_repo(tmp2.path(), "lib.py", "print('hi')\n", "Repo2 commit");

        let repos = normalize_repo_inputs(
            vec![tmp1.path().to_path_buf(), tmp2.path().to_path_buf()],
            None,
        )
        .unwrap();
        let (commits, errors) = analyze_repos(&repos, None, None);

        assert!(errors.is_empty());
        assert_eq!(commits.len(), 2);

        let repos: Vec<&str> = commits.iter().map(|c| c.repo.as_str()).collect();
        let name1 = tmp1
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let name2 = tmp2
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(repos.contains(&name1.as_str()));
        assert!(repos.contains(&name2.as_str()));
    }

    #[test]
    fn test_analyze_bad_repo_error_collected() {
        let tmp_good = TempDir::new().unwrap();
        create_test_repo(tmp_good.path(), "file.txt", "content\n", "Good commit");

        let bad_path = PathBuf::from("/nonexistent/fake/repo");
        let repos =
            normalize_repo_inputs(vec![bad_path.clone(), tmp_good.path().to_path_buf()], None)
                .unwrap();
        let (commits, errors) = analyze_repos(&repos, None, None);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, bad_path);

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message_subject, "Good commit");
    }

    #[test]
    fn test_analyze_empty_paths() {
        let repos: Vec<RepoInput> = vec![];
        let (commits, errors) = analyze_repos(&repos, None, None);
        assert!(commits.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn test_analyze_until_is_exclusive() {
        let tmp = TempDir::new().unwrap();
        create_test_repo(tmp.path(), "file.txt", "content\n", "Commit at boundary");
        let repos = normalize_repo_inputs(vec![tmp.path().to_path_buf()], None).unwrap();
        let boundary = DateTime::from_timestamp(1_705_312_800, 0).unwrap();

        let (commits, errors) = analyze_repos(&repos, None, Some(boundary));

        assert!(errors.is_empty());
        assert!(commits.is_empty());
    }

    #[test]
    fn normalize_repo_inputs_deduplicates_and_disambiguates_labels() {
        let tmp = TempDir::new().unwrap();
        let left = tmp.path().join("left").join("service");
        let right = tmp.path().join("right").join("service");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        create_test_repo(&left, "left.rs", "fn left() {}\n", "left");
        create_test_repo(&right, "right.rs", "fn right() {}\n", "right");

        let repos = normalize_repo_inputs(vec![left.clone(), left, right], None).unwrap();

        assert_eq!(repos.len(), 2);
        assert!(repos.iter().all(|repo| repo.id.contains('/')));
        assert_eq!(repos[0].label, "left/service");
        assert_eq!(repos[1].label, "right/service");
    }

    #[test]
    fn platform_repo_keys_and_equality_follow_the_requested_platform_mode() {
        assert_eq!(platform_repo_key(r"Team\Service", true), "team/service");
        assert_eq!(platform_repo_key(r"Team\Service", false), "Team/Service");
        assert!(platform_repo_eq("Team/Service", "team/service", true));
        assert!(!platform_repo_eq("Team/Service", "team/service", false));
    }

    #[test]
    fn merge_commit_counts_once_without_replaying_parent_churn() {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let base_sig =
            Signature::new("Test", "test@example.com", &Time::new(1_705_312_800, 0)).unwrap();
        let base_blob = repo.blob(b"base\n").unwrap();
        let mut base_tree_builder = repo.treebuilder(None).unwrap();
        base_tree_builder
            .insert("base.rs", base_blob, 0o100644)
            .unwrap();
        let base_tree = repo.find_tree(base_tree_builder.write().unwrap()).unwrap();
        let base_oid = repo
            .commit(Some("HEAD"), &base_sig, &base_sig, "Base", &base_tree, &[])
            .unwrap();
        let base = repo.find_commit(base_oid).unwrap();

        let main_sig =
            Signature::new("Test", "test@example.com", &Time::new(1_705_316_400, 0)).unwrap();
        let main_blob = repo.blob(b"base\nmain\n").unwrap();
        let mut main_tree_builder = repo.treebuilder(Some(&base_tree)).unwrap();
        main_tree_builder
            .insert("base.rs", main_blob, 0o100644)
            .unwrap();
        let main_tree = repo.find_tree(main_tree_builder.write().unwrap()).unwrap();
        let main_oid = repo
            .commit(
                Some("HEAD"),
                &main_sig,
                &main_sig,
                "Main",
                &main_tree,
                &[&base],
            )
            .unwrap();
        let main = repo.find_commit(main_oid).unwrap();

        let feature_sig =
            Signature::new("Test", "test@example.com", &Time::new(1_705_320_000, 0)).unwrap();
        let feature_blob = repo.blob(b"feature\n").unwrap();
        let mut feature_tree_builder = repo.treebuilder(Some(&base_tree)).unwrap();
        feature_tree_builder
            .insert("feature.rs", feature_blob, 0o100644)
            .unwrap();
        let feature_tree = repo
            .find_tree(feature_tree_builder.write().unwrap())
            .unwrap();
        let feature_oid = repo
            .commit(
                None,
                &feature_sig,
                &feature_sig,
                "Feature",
                &feature_tree,
                &[&base],
            )
            .unwrap();
        let feature = repo.find_commit(feature_oid).unwrap();

        let merge_sig =
            Signature::new("Test", "test@example.com", &Time::new(1_705_323_600, 0)).unwrap();
        let mut merge_tree_builder = repo.treebuilder(Some(&main_tree)).unwrap();
        merge_tree_builder
            .insert("feature.rs", feature_blob, 0o100644)
            .unwrap();
        let merge_tree = repo.find_tree(merge_tree_builder.write().unwrap()).unwrap();
        let merge_oid = repo
            .commit(
                Some("HEAD"),
                &merge_sig,
                &merge_sig,
                "Merge feature",
                &merge_tree,
                &[&main, &feature],
            )
            .unwrap();

        let repos = normalize_repo_inputs(vec![tmp.path().to_path_buf()], None).unwrap();
        let (commits, errors) = analyze_repos(&repos, None, None);

        assert!(errors.is_empty());
        assert_eq!(commits.len(), 4);
        let merge = commits
            .iter()
            .find(|commit| commit.oid == merge_oid.to_string())
            .unwrap();
        assert!(merge.file_changes.is_empty());
        let aggregate = aggregate_commits(&commits, &Period::Day, None, None);
        assert_eq!(
            aggregate
                .iter()
                .map(|stats| stats.total_commits)
                .sum::<u64>(),
            4
        );
    }
}
