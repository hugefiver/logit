use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use git2::{Repository, Sort};

use crate::stats::models::Author;

#[cfg(feature = "github")]
pub struct RemoteInfo {
    pub platform: Platform,
    pub owner: String,
    pub repo: String,
}

#[cfg(feature = "github")]
pub enum Platform {
    GitHub,
    GitLab,
}

#[cfg(feature = "github")]
pub fn parse_remote_url(url: &str) -> Option<RemoteInfo> {
    let (platform, path) = if let Some(rest) = url.strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        (Platform::GitHub, rest)
    } else if let Some(rest) = url.strip_prefix("https://gitlab.com/")
        .or_else(|| url.strip_prefix("http://gitlab.com/"))
    {
        (Platform::GitLab, rest)
    } else if let Some(rest) = url.strip_prefix("git@github.com:") {
        (Platform::GitHub, rest)
    } else if let Some(rest) = url.strip_prefix("git@gitlab.com:") {
        (Platform::GitLab, rest)
    } else {
        return None;
    };

    let path = path.trim_end_matches('/').strip_suffix(".git").unwrap_or(path.trim_end_matches('/'));
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(RemoteInfo { platform, owner: owner.to_string(), repo: repo.to_string() })
}

#[cfg(feature = "github")]
pub fn get_remote_origin(repo_path: &Path) -> Option<String> {
    let repo = Repository::open(repo_path).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    remote.url().map(|u| u.to_string())
}

pub struct RepoAnalyzer {
    repo: Repository,
    repo_name: String,
}

/// Extracted commit information from a git repository.
pub struct CommitInfo {
    pub oid: git2::Oid,
    pub author: Author,
    pub committer: Author,
    pub timestamp: DateTime<Utc>,
    pub message: String,
    /// Kept for future drill-down features (e.g., TUI commit graph).
    #[allow(dead_code)]
    pub parent_oids: Vec<git2::Oid>,
}

impl RepoAnalyzer {
    /// Open a git repository at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::open(path)
            .with_context(|| format!("Failed to open git repo at {}", path.display()))?;
        let repo_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        Ok(Self { repo, repo_name })
    }

    pub fn repo_name(&self) -> &str {
        &self.repo_name
    }

    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    /// Walk commits from HEAD, optionally filtered by date range.
    /// Returns commits in chronological order (oldest first).
    pub fn walk_commits(
        &self,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> Result<Vec<CommitInfo>> {
        let head = match self.repo.head() {
            Ok(h) => h,
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => return Ok(Vec::new()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).context("Failed to get HEAD reference"),
        };

        let head_oid = match head.target() {
            Some(oid) => oid,
            None => return Ok(Vec::new()),
        };

        let mut revwalk = self.repo.revwalk().context("Failed to create revwalk")?;
        revwalk.set_sorting(Sort::TIME | Sort::REVERSE)?;
        revwalk.push(head_oid)?;

        let mut commits = Vec::new();

        for oid_result in revwalk {
            let oid = oid_result.context("Failed to get oid during revwalk")?;
            let commit = self
                .repo
                .find_commit(oid)
                .with_context(|| format!("Failed to find commit {oid}"))?;

            let time = commit.time();
            let timestamp = DateTime::from_timestamp(time.seconds(), 0).unwrap_or_default();

            if let Some(ref s) = since
                && timestamp < *s
            {
                continue;
            }
            if let Some(ref u) = until
                && timestamp > *u
            {
                continue;
            }

            let author_sig = commit.author();
            let committer_sig = commit.committer();

            let author = Author {
                name: author_sig.name().unwrap_or("Unknown").to_string(),
                email: author_sig.email().unwrap_or("").to_string(),
            };
            let committer = Author {
                name: committer_sig.name().unwrap_or("Unknown").to_string(),
                email: committer_sig.email().unwrap_or("").to_string(),
            };

            let message = commit.message().unwrap_or("").to_string();

            let parent_oids: Vec<git2::Oid> = (0..commit.parent_count())
                .filter_map(|i| commit.parent_id(i).ok())
                .collect();

            commits.push(CommitInfo {
                oid,
                author,
                committer,
                timestamp,
                message,
                parent_oids,
            });
        }

        Ok(commits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_simple_test_repo(dir: &Path) {
        let repo = Repository::init(dir).unwrap();
        let sig = git2::Signature::new("Test", "test@test.com", &git2::Time::new(1_705_312_800, 0))
            .unwrap();
        let blob = repo.blob(b"hello").unwrap();
        let tree_oid = {
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("file.txt", blob, 0o100644).unwrap();
            tb.write().unwrap()
        };
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
            .unwrap();

        // Second commit
        let sig2 = git2::Signature::new(
            "Test2",
            "test2@test.com",
            &git2::Time::new(1_705_413_600, 0),
        )
        .unwrap();
        let blob2 = repo.blob(b"world").unwrap();
        let tree_oid2 = {
            let mut tb2 = repo.treebuilder(Some(&tree)).unwrap();
            tb2.insert("file2.txt", blob2, 0o100644).unwrap();
            tb2.write().unwrap()
        };
        let tree2 = repo.find_tree(tree_oid2).unwrap();
        let parent = repo
            .find_commit(repo.head().unwrap().target().unwrap())
            .unwrap();
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
    fn walk_all_commits() {
        let tmp = TempDir::new().unwrap();
        create_simple_test_repo(tmp.path());
        let analyzer = RepoAnalyzer::open(tmp.path()).unwrap();
        let commits = analyzer.walk_commits(None, None).unwrap();
        assert_eq!(commits.len(), 2);
    }

    #[test]
    fn walk_commits_chronological_order() {
        let tmp = TempDir::new().unwrap();
        create_simple_test_repo(tmp.path());
        let analyzer = RepoAnalyzer::open(tmp.path()).unwrap();
        let commits = analyzer.walk_commits(None, None).unwrap();
        // First should be older (1705312800)
        assert!(commits[0].timestamp < commits[1].timestamp);
    }

    #[test]
    fn walk_commits_with_since_filter() {
        let tmp = TempDir::new().unwrap();
        create_simple_test_repo(tmp.path());
        let analyzer = RepoAnalyzer::open(tmp.path()).unwrap();
        // since = 2024-01-16 00:00 UTC (epoch 1705363200) — should only get commit 2
        let since = DateTime::from_timestamp(1_705_363_200, 0).unwrap();
        let commits = analyzer.walk_commits(Some(since), None).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].author.name, "Test2");
    }

    #[test]
    fn walk_commits_with_until_filter() {
        let tmp = TempDir::new().unwrap();
        create_simple_test_repo(tmp.path());
        let analyzer = RepoAnalyzer::open(tmp.path()).unwrap();
        // until = 2024-01-16 00:00 UTC — should only get commit 1
        let until = DateTime::from_timestamp(1_705_363_200, 0).unwrap();
        let commits = analyzer.walk_commits(None, Some(until)).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].author.name, "Test");
    }

    #[test]
    fn empty_repo_returns_empty() {
        let tmp = TempDir::new().unwrap();
        Repository::init(tmp.path()).unwrap();
        let analyzer = RepoAnalyzer::open(tmp.path()).unwrap();
        let commits = analyzer.walk_commits(None, None).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn commit_info_has_parent_oids() {
        let tmp = TempDir::new().unwrap();
        create_simple_test_repo(tmp.path());
        let analyzer = RepoAnalyzer::open(tmp.path()).unwrap();
        let commits = analyzer.walk_commits(None, None).unwrap();
        // First commit has no parents
        assert!(commits[0].parent_oids.is_empty());
        // Second commit has one parent
        assert_eq!(commits[1].parent_oids.len(), 1);
    }

    #[cfg(feature = "github")]
    #[test]
    fn parse_github_https_url() {
        let info = parse_remote_url("https://github.com/hugefiver/logit.git").unwrap();
        assert!(matches!(info.platform, Platform::GitHub));
        assert_eq!(info.owner, "hugefiver");
        assert_eq!(info.repo, "logit");
    }

    #[cfg(feature = "github")]
    #[test]
    fn parse_github_ssh_url() {
        let info = parse_remote_url("git@github.com:hugefiver/logit.git").unwrap();
        assert!(matches!(info.platform, Platform::GitHub));
        assert_eq!(info.owner, "hugefiver");
        assert_eq!(info.repo, "logit");
    }

    #[cfg(feature = "github")]
    #[test]
    fn parse_gitlab_https_url() {
        let info = parse_remote_url("https://gitlab.com/user/project").unwrap();
        assert!(matches!(info.platform, Platform::GitLab));
        assert_eq!(info.owner, "user");
        assert_eq!(info.repo, "project");
    }

    #[cfg(feature = "github")]
    #[test]
    fn parse_unknown_host_returns_none() {
        assert!(parse_remote_url("https://bitbucket.org/user/repo.git").is_none());
    }

    #[cfg(feature = "github")]
    #[test]
    fn parse_invalid_path_returns_none() {
        assert!(parse_remote_url("https://github.com/").is_none());
        assert!(parse_remote_url("https://github.com/owner-only").is_none());
    }
}
