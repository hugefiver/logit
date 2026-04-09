use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rayon::prelude::*;

use crate::git::author::extract_co_authors;
use crate::git::diff::analyze_commit_diff;
use crate::git::repo::RepoAnalyzer;
use crate::lang::apply_language_to_changes;
use crate::stats::models::CommitStats;

/// Error from analyzing a single repository.
pub struct RepoError {
    pub path: PathBuf,
    pub error: String,
}

/// Analyze multiple repos in parallel.
///
/// Returns `(all_commits, repo_errors)`.
/// Each rayon task opens its own `Repository` — `git2::Repository` is not `Send`,
/// so we pass only `PathBuf` into the parallel iterator.
pub fn analyze_repos(
    paths: &[PathBuf],
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> (Vec<CommitStats>, Vec<RepoError>) {
    let results: Vec<Result<Vec<CommitStats>, RepoError>> = paths
        .par_iter()
        .map(|path| analyze_single_repo(path, since, until))
        .collect();

    let mut all_commits = Vec::new();
    let mut errors = Vec::new();

    for result in results {
        match result {
            Ok(commits) => all_commits.extend(commits),
            Err(e) => {
                eprintln!("Error analyzing repo {}: {}", e.path.display(), e.error);
                errors.push(e);
            }
        }
    }

    (all_commits, errors)
}

/// Analyze a single repository. Opens its own `Repository` handle.
fn analyze_single_repo(
    path: &Path,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<Vec<CommitStats>, RepoError> {
    let analyzer = RepoAnalyzer::open(path).map_err(|e| RepoError {
        path: path.to_path_buf(),
        error: format!("{e:#}"),
    })?;

    let commit_infos = analyzer.walk_commits(since, until).map_err(|e| RepoError {
        path: path.to_path_buf(),
        error: format!("{e:#}"),
    })?;

    let repo = analyzer.repo();
    let repo_name = analyzer.repo_name().to_string();

    let mut stats = Vec::with_capacity(commit_infos.len());

    for ci in &commit_infos {
        let commit = repo.find_commit(ci.oid).map_err(|e| RepoError {
            path: path.to_path_buf(),
            error: format!("Failed to find commit {}: {e:#}", ci.oid),
        })?;

        let mut file_changes = analyze_commit_diff(repo, &commit).map_err(|e| RepoError {
            path: path.to_path_buf(),
            error: format!("Failed to analyze diff for {}: {e:#}", ci.oid),
        })?;

        apply_language_to_changes(&mut file_changes);

        let co_authors = extract_co_authors(&ci.message);
        let message_subject = ci.message.lines().next().unwrap_or("").to_string();

        stats.push(CommitStats {
            repo: repo_name.clone(),
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

        let paths = vec![tmp.path().to_path_buf()];
        let (commits, errors) = analyze_repos(&paths, None, None);

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
    }

    #[test]
    fn test_analyze_multiple_repos() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();

        create_test_repo(tmp1.path(), "main.rs", "fn main() {}\n", "Repo1 commit");
        create_test_repo(tmp2.path(), "lib.py", "print('hi')\n", "Repo2 commit");

        let paths = vec![tmp1.path().to_path_buf(), tmp2.path().to_path_buf()];
        let (commits, errors) = analyze_repos(&paths, None, None);

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
        let paths = vec![bad_path.clone(), tmp_good.path().to_path_buf()];
        let (commits, errors) = analyze_repos(&paths, None, None);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, bad_path);

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message_subject, "Good commit");
    }

    #[test]
    fn test_analyze_empty_paths() {
        let paths: Vec<PathBuf> = vec![];
        let (commits, errors) = analyze_repos(&paths, None, None);
        assert!(commits.is_empty());
        assert!(errors.is_empty());
    }
}
