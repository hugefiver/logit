use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitStats {
    pub repo: String,
    pub oid: String,
    pub author: Author,
    pub committer: Author,
    pub co_authors: Vec<Author>,
    pub timestamp: DateTime<Utc>,
    pub message_subject: String,
    pub file_changes: Vec<FileChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Author {
    pub name: String,
    pub email: String,
}

impl Author {
    pub fn matches(&self, pattern: &str) -> bool {
        let pattern_lower = pattern.to_lowercase();
        self.name.to_lowercase().contains(&pattern_lower)
            || self.email.to_lowercase().contains(&pattern_lower)
    }
}

impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} <{}>", self.name, self.email)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub language: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    /// `max(additions, deletions)` — counts each same-line change as one modification,
    /// plus any purely-added or purely-deleted lines.
    #[serde(default)]
    pub net_modifications: u64,
    /// `additions.saturating_sub(deletions)` — only the truly new lines,
    /// excluding deletions and same-line replacements.
    #[serde(default)]
    pub net_additions: u64,
}

impl fmt::Display for FileChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (+{}/-{})", self.path, self.additions, self.deletions)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodStats {
    pub period_label: String,
    pub by_language: HashMap<String, LangStats>,
    pub by_author: HashMap<String, AuthorStats>,
    pub total_commits: u64,
    pub total_additions: u64,
    pub total_deletions: u64,
    /// Local path: computed per-file then summed. GitHub path: computed per-commit then summed.
    /// Neither equals per-hunk computation; GitHub lacks hunk-level data.
    #[serde(default)]
    pub total_net_modifications: u64,
    /// Local path: `Σ file.additions.saturating_sub(file.deletions)`.
    /// GitHub path: `Σ commit.additions.saturating_sub(commit.deletions)`.
    #[serde(default)]
    pub total_net_additions: u64,
}

/// A node in a multi-level grouping tree.
///
/// Leaf nodes have empty `children` and `stats` contains the data for that
/// single bucket. Non-leaf nodes have children and `stats` is the aggregated
/// total across all descendants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupNode {
    pub label: String,
    pub stats: PeriodStats,
    pub children: Vec<GroupNode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LangStats {
    pub additions: u64,
    pub deletions: u64,
    pub files_changed: u64,
    #[serde(default)]
    pub net_modifications: u64,
    #[serde(default)]
    pub net_additions: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorStats {
    pub commits: u64,
    pub co_authored_commits: u64,
    pub additions: u64,
    pub co_authored_additions: u64,
    pub deletions: u64,
    pub co_authored_deletions: u64,
    pub languages: HashMap<String, LangStats>,
    /// Per-language breakdown of co-authored contributions only.
    /// Used to correctly split authored vs co-authored when excluding languages.
    #[serde(default)]
    pub co_authored_languages: HashMap<String, LangStats>,
    #[serde(default)]
    pub net_modifications: u64,
    #[serde(default)]
    pub co_authored_net_modifications: u64,
    #[serde(default)]
    pub net_additions: u64,
    #[serde(default)]
    pub co_authored_net_additions: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn author_matches_name_case_insensitive() {
        let author = Author {
            name: "Alice Smith".to_string(),
            email: "alice@example.com".to_string(),
        };
        assert!(author.matches("alice"));
        assert!(author.matches("ALICE"));
        assert!(author.matches("Alice"));
        assert!(author.matches("smith"));
    }

    #[test]
    fn author_matches_email() {
        let author = Author {
            name: "Alice Smith".to_string(),
            email: "alice@example.com".to_string(),
        };
        assert!(author.matches("example.com"));
        assert!(author.matches("alice@"));
        assert!(author.matches("EXAMPLE"));
    }

    #[test]
    fn author_matches_partial() {
        let author = Author {
            name: "Alice Smith".to_string(),
            email: "alice@example.com".to_string(),
        };
        assert!(author.matches("ice"));
        assert!(author.matches("mith"));
    }

    #[test]
    fn author_no_match() {
        let author = Author {
            name: "Alice Smith".to_string(),
            email: "alice@example.com".to_string(),
        };
        assert!(!author.matches("bob"));
        assert!(!author.matches("nonexistent"));
    }

    #[test]
    fn author_display() {
        let author = Author {
            name: "Alice Smith".to_string(),
            email: "alice@example.com".to_string(),
        };
        assert_eq!(format!("{author}"), "Alice Smith <alice@example.com>");
    }

    #[test]
    fn file_change_display() {
        let fc = FileChange {
            path: "src/main.rs".to_string(),
            language: Some("Rust".to_string()),
            additions: 10,
            deletions: 3,
            net_modifications: 10,
            net_additions: 7,
        };
        assert_eq!(format!("{fc}"), "src/main.rs (+10/-3)");
    }

    #[test]
    fn serde_round_trip_commit_stats() {
        let commit = CommitStats {
            repo: "logit".to_string(),
            oid: "abc123".to_string(),
            author: Author {
                name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
            },
            committer: Author {
                name: "Bob".to_string(),
                email: "bob@example.com".to_string(),
            },
            co_authors: vec![Author {
                name: "Carol".to_string(),
                email: "carol@example.com".to_string(),
            }],
            timestamp: Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            message_subject: "feat: add stats".to_string(),
            file_changes: vec![FileChange {
                path: "src/lib.rs".to_string(),
                language: Some("Rust".to_string()),
                additions: 50,
                deletions: 10,
                net_modifications: 50,
                net_additions: 40,
            }],
        };

        let json = serde_json::to_string(&commit).unwrap();
        let deserialized: CommitStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.repo, commit.repo);
        assert_eq!(deserialized.oid, commit.oid);
        assert_eq!(deserialized.author, commit.author);
        assert_eq!(deserialized.committer, commit.committer);
        assert_eq!(deserialized.co_authors, commit.co_authors);
        assert_eq!(deserialized.timestamp, commit.timestamp);
        assert_eq!(deserialized.message_subject, commit.message_subject);
        assert_eq!(deserialized.file_changes.len(), 1);
        assert_eq!(deserialized.file_changes[0].path, "src/lib.rs");
        assert_eq!(deserialized.file_changes[0].additions, 50);
        assert_eq!(deserialized.file_changes[0].deletions, 10);
    }

    #[test]
    fn serde_round_trip_period_stats() {
        let mut by_language = HashMap::new();
        by_language.insert(
            "Rust".to_string(),
            LangStats {
                additions: 100,
                deletions: 20,
                files_changed: 5,
                ..Default::default()
            },
        );

        let mut author_languages = HashMap::new();
        author_languages.insert(
            "Rust".to_string(),
            LangStats {
                additions: 100,
                deletions: 20,
                files_changed: 5,
                ..Default::default()
            },
        );

        let mut by_author = HashMap::new();
        by_author.insert(
            "alice@example.com".to_string(),
            AuthorStats {
                commits: 10,
                co_authored_commits: 0,
                co_authored_additions: 0,
                co_authored_deletions: 0,
                additions: 100,
                deletions: 20,
                languages: author_languages,
                ..Default::default()
            },
        );

        let period = PeriodStats {
            period_label: "2025-W03".to_string(),
            by_language,
            by_author,
            total_commits: 10,
            total_additions: 100,
            total_deletions: 20,
            total_net_modifications: 100,
            total_net_additions: 80,
        };

        let json = serde_json::to_string(&period).unwrap();
        let deserialized: PeriodStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.period_label, "2025-W03");
        assert_eq!(deserialized.total_commits, 10);
        assert_eq!(deserialized.total_additions, 100);
        assert_eq!(deserialized.total_deletions, 20);
        assert!(deserialized.by_language.contains_key("Rust"));
        assert!(deserialized.by_author.contains_key("alice@example.com"));
    }
}
