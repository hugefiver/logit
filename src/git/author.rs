use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::stats::models::{Author, CommitStats};

static CO_AUTHOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^co-authored-by:\s*(.+?)\s*<(.+?)>")
        .expect("co-author regex is a valid compile-time constant")
});

/// Produce the canonical comparison key for an email address.
pub fn canonical_email_key(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

/// Produce the commit-local identity key used to distinguish author roles.
pub fn commit_identity_key(author: &Author) -> String {
    let email = canonical_email_key(&author.email);
    if email.is_empty() {
        format!("name:{}", author.name.trim().to_ascii_lowercase())
    } else {
        format!("email:{email}")
    }
}

/// Preserve the first display identity for each non-primary commit identity.
pub fn normalize_co_authors(
    primary: &Author,
    co_authors: impl IntoIterator<Item = Author>,
) -> Vec<Author> {
    let mut seen = HashSet::from([commit_identity_key(primary)]);
    co_authors
        .into_iter()
        .filter(|author| seen.insert(commit_identity_key(author)))
        .collect()
}

/// Extract normalized co-authors from a commit message by parsing Co-authored-by trailers.
pub fn extract_co_authors(message: &str, primary: &Author) -> Vec<Author> {
    normalize_co_authors(
        primary,
        CO_AUTHOR_RE.captures_iter(message).map(|cap| Author {
            name: cap[1].trim().to_string(),
            email: cap[2].trim().to_string(),
        }),
    )
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
        let primary = Author {
            name: "Alice".to_string(),
            email: "alice@test.com".to_string(),
        };
        let co_authors = extract_co_authors(msg, &primary);
        assert_eq!(co_authors.len(), 1);
        assert_eq!(co_authors[0].name, "Charlie");
        assert_eq!(co_authors[0].email, "charlie@test.com");
    }

    #[test]
    fn extract_multiple_co_authors() {
        let msg = "Add feature\n\nCo-authored-by: Charlie <charlie@test.com>\nCo-authored-by: Dave <dave@test.com>";
        let primary = Author {
            name: "Alice".to_string(),
            email: "alice@test.com".to_string(),
        };
        let co_authors = extract_co_authors(msg, &primary);
        assert_eq!(co_authors.len(), 2);
        assert_eq!(co_authors[0].name, "Charlie");
        assert_eq!(co_authors[1].name, "Dave");
    }

    #[test]
    fn extract_co_author_case_insensitive() {
        let msg = "Fix\n\nCO-AUTHORED-BY: Eve <eve@test.com>";
        let primary = Author {
            name: "Alice".to_string(),
            email: "alice@test.com".to_string(),
        };
        let co_authors = extract_co_authors(msg, &primary);
        assert_eq!(co_authors.len(), 1);
        assert_eq!(co_authors[0].name, "Eve");
    }

    #[test]
    fn no_co_authors() {
        let msg = "Simple commit message";
        let primary = Author {
            name: "Alice".to_string(),
            email: "alice@test.com".to_string(),
        };
        let co_authors = extract_co_authors(msg, &primary);
        assert!(co_authors.is_empty());
    }

    #[test]
    fn extract_co_authors_removes_primary_and_canonical_duplicates() {
        let primary = Author {
            name: "Alice".to_string(),
            email: " Alice@Example.com ".to_string(),
        };
        let msg = "Add feature\n\nCo-authored-by: Alias <alice@example.com>\nCo-authored-by: Bob <bob@example.com>\nCo-authored-by: Robert < BOB@EXAMPLE.COM >";

        let co_authors = extract_co_authors(msg, &primary);

        assert_eq!(co_authors.len(), 1);
        assert_eq!(co_authors[0].name, "Bob");
        assert_eq!(co_authors[0].email, "bob@example.com");
        assert_eq!(commit_identity_key(&co_authors[0]), "email:bob@example.com");
    }

    #[test]
    fn empty_email_identity_falls_back_to_trimmed_lowercase_name() {
        let author = Author {
            name: " ÄLICE ".to_string(),
            email: "   ".to_string(),
        };

        assert_eq!(
            canonical_email_key(" Alice@Example.com "),
            "alice@example.com"
        );
        assert_eq!(commit_identity_key(&author), "name:Älice");
    }

    #[test]
    fn commit_involves_author_checks_author() {
        let commit = CommitStats {
            repo_id: "test-id".to_string(),
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
            repo_id: "test-id".to_string(),
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
            repo_id: "test-id".to_string(),
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
