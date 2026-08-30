use std::path::{Path, PathBuf};

use anyhow::Result;

pub struct ScanReport {
    pub repos: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Recursively scan for git repositories under `root`.
///
/// Returns sorted list of repo root paths. Uses stack-based iterative DFS.
/// Does not descend into discovered repositories. Skips symlinks.
pub fn scan_for_repos(root: &Path) -> Result<ScanReport> {
    anyhow::ensure!(
        root.is_dir(),
        "path '{}' does not exist or is not a directory",
        root.display()
    );

    let mut report = ScanReport {
        repos: Vec::new(),
        warnings: Vec::new(),
    };
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let git_marker = dir.join(".git");
        match git_marker.try_exists() {
            Ok(true) => match git2::Repository::open(&dir) {
                Ok(_) => {
                    report.repos.push(dir);
                    continue;
                }
                Err(e) => {
                    report.warnings.push(format!(
                        "cannot open repository with .git marker '{}': {e}",
                        git_marker.display()
                    ));
                    continue;
                }
            },
            Ok(false) => {}
            Err(e) => {
                report.warnings.push(format!(
                    "cannot inspect git marker '{}': {e}",
                    git_marker.display()
                ));
                continue;
            }
        }

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                report
                    .warnings
                    .push(format!("cannot read directory '{}': {e}", dir.display()));
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    report.warnings.push(format!(
                        "cannot read directory entry under '{}': {e}",
                        dir.display()
                    ));
                    continue;
                }
            };

            let path = entry.path();

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    report.warnings.push(format!(
                        "cannot read metadata for '{}': {e}",
                        path.display()
                    ));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                continue;
            }

            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }

    report.repos.sort();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Repository;
    use tempfile::TempDir;

    #[test]
    fn finds_repos_in_nested_structure() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        let repo_a = base.join("repo-a");
        let repo_b = base.join("nested").join("repo-b");
        let plain = base.join("plain-dir");

        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();
        std::fs::create_dir_all(&plain).unwrap();

        Repository::init(&repo_a).unwrap();
        Repository::init(&repo_b).unwrap();

        let report = scan_for_repos(base).expect("scan should succeed");
        let repos = report.repos;
        assert_eq!(
            repos.len(),
            2,
            "should find exactly 2 repos, got: {repos:?}"
        );
        assert!(repos.contains(&repo_a));
        assert!(repos.contains(&repo_b));
    }

    #[test]
    fn empty_directory_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let report = scan_for_repos(tmp.path()).expect("scan should succeed");
        let repos = report.repos;
        assert!(repos.is_empty());
    }

    #[test]
    fn nonexistent_path_returns_error() {
        let result = scan_for_repos(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_err());
    }

    #[test]
    fn results_are_sorted() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        let repo_c = base.join("c-repo");
        let repo_a = base.join("a-repo");
        let repo_b = base.join("b-repo");

        for dir in [&repo_c, &repo_a, &repo_b] {
            std::fs::create_dir_all(dir).unwrap();
            Repository::init(dir).unwrap();
        }

        let report = scan_for_repos(base).expect("scan should succeed");
        let repos = report.repos;
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[0], repo_a);
        assert_eq!(repos[1], repo_b);
        assert_eq!(repos[2], repo_c);
    }

    #[test]
    fn skips_non_repo_directories() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        let non_repo = base.join("not-a-repo");
        std::fs::create_dir_all(&non_repo).unwrap();

        let report = scan_for_repos(base).expect("scan should succeed");
        let repos = report.repos;
        assert!(repos.is_empty());
    }

    #[test]
    fn does_not_descend_into_repo_subdirs() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        let outer = base.join("outer");
        std::fs::create_dir_all(&outer).unwrap();
        Repository::init(&outer).unwrap();

        let inner = outer.join("subdir").join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        Repository::init(&inner).unwrap();

        let report = scan_for_repos(base).expect("scan should succeed");
        let repos = report.repos;
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0], outer);
    }

    #[test]
    fn invalid_git_marker_is_reported_once() {
        let tmp = TempDir::new().unwrap();
        let invalid_repo = tmp.path().join("invalid-repo");
        std::fs::create_dir_all(&invalid_repo).unwrap();
        std::fs::write(invalid_repo.join(".git"), "not a git directory").unwrap();

        let report = scan_for_repos(tmp.path()).expect("scan should succeed");

        assert!(report.repos.is_empty());
        assert_eq!(report.warnings.len(), 1, "warnings: {:?}", report.warnings);
        assert!(report.warnings[0].contains(".git"));
    }
}
