use regex::Regex;
use std::sync::LazyLock;

use crate::stats::models::{Author, CommitStats};

static CO_AUTHOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^co-authored-by:\s*(.+?)\s*<(.+?)>")
        .expect("co-author regex is a valid compile-time constant")
});

/// Extract co-authors from a commit message by parsing Co-authored-by trailers.
pub fn extract_co_authors(message: &str) -> Vec<Author> {
    CO_AUTHOR_RE
        .captures_iter(message)
        .map(|cap| Author {
            name: cap[1].trim().to_string(),
            email: cap[2].trim().to_string(),
        })
        .collect()
}

/// Check if a commit involves a given author (as author or co-author).
pub fn commit_involves_author(commit: &CommitStats, pattern: &str) -> bool {
    commit.author.matches(pattern) || commit.co_authors.iter().any(|a| a.matches(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn extract_single_co_author() {
        let msg = "Add feature\n\nCo-authored-by: Charlie <charlie@test.com>";
        let co_authors = extract_co_authors(msg);
        assert_eq!(co_authors.len(), 1);
        assert_eq!(co_authors[0].name, "Charlie");
        assert_eq!(co_authors[0].email, "charlie@test.com");
    }

    #[test]
    fn extract_multiple_co_authors() {
        let msg = "Add feature\n\nCo-authored-by: Charlie <charlie@test.com>\nCo-authored-by: Dave <dave@test.com>";
        let co_authors = extract_co_authors(msg);
        assert_eq!(co_authors.len(), 2);
        assert_eq!(co_authors[0].name, "Charlie");
        assert_eq!(co_authors[1].name, "Dave");
    }

    #[test]
    fn extract_co_author_case_insensitive() {
        let msg = "Fix\n\nCO-AUTHORED-BY: Eve <eve@test.com>";
        let co_authors = extract_co_authors(msg);
        assert_eq!(co_authors.len(), 1);
        assert_eq!(co_authors[0].name, "Eve");
    }

    #[test]
    fn no_co_authors() {
        let msg = "Simple commit message";
        let co_authors = extract_co_authors(msg);
        assert!(co_authors.is_empty());
    }

    #[test]
    fn commit_involves_author_checks_author() {
        let commit = CommitStats {
            repo: "test".to_string(),
            oid: "abc".to_string(),
            author: Author {
                name: "Alice".to_string(),
                email: "alice@test.com".to_string(),
            },
            committer: Author {
                name: "Alice".to_string(),
                email: "alice@test.com".to_string(),
            },
            co_authors: vec![],
            timestamp: Utc::now(),
            message_subject: "test".to_string(),
            file_changes: vec![],
        };
        assert!(commit_involves_author(&commit, "alice"));
        assert!(commit_involves_author(&commit, "Alice"));
        assert!(!commit_involves_author(&commit, "bob"));
    }

    #[test]
    fn commit_involves_author_checks_co_authors() {
        let commit = CommitStats {
            repo: "test".to_string(),
            oid: "abc".to_string(),
            author: Author {
                name: "Alice".to_string(),
                email: "alice@test.com".to_string(),
            },
            committer: Author {
                name: "Alice".to_string(),
                email: "alice@test.com".to_string(),
            },
            co_authors: vec![Author {
                name: "Charlie".to_string(),
                email: "charlie@test.com".to_string(),
            }],
            timestamp: Utc::now(),
            message_subject: "test".to_string(),
            file_changes: vec![],
        };
        assert!(commit_involves_author(&commit, "charlie"));
        assert!(commit_involves_author(&commit, "alice"));
    }

    #[test]
    fn commit_involves_author_matches_email_domain() {
        let commit = CommitStats {
            repo: "test".to_string(),
            oid: "abc".to_string(),
            author: Author {
                name: "Alice".to_string(),
                email: "alice@company.com".to_string(),
            },
            committer: Author {
                name: "Alice".to_string(),
                email: "alice@company.com".to_string(),
            },
            co_authors: vec![],
            timestamp: Utc::now(),
            message_subject: "test".to_string(),
            file_changes: vec![],
        };
        assert!(commit_involves_author(&commit, "company.com"));
    }
}
