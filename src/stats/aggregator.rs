use std::collections::HashMap;

use chrono::{DateTime, Datelike, Utc};

use crate::cli::Period;
use crate::stats::models::{AuthorStats, CommitStats, PeriodStats};

/// Bucket a timestamp into a period label string.
/// - Day: "2024-01-15"
/// - Week: "2024-W03" (ISO 8601 week)
/// - Month: "2024-01"
pub fn bucket_timestamp(ts: &DateTime<Utc>, period: &Period) -> String {
    match period {
        Period::Day => ts.format("%Y-%m-%d").to_string(),
        Period::Week => {
            let iso = ts.iso_week();
            format!("{}-W{:02}", iso.year(), iso.week())
        }
        Period::Month => ts.format("%Y-%m").to_string(),
    }
}

/// Aggregate a slice of commits into per-period statistics.
///
/// - `author_filter`: if `Some`, only include commits where the author or a
///   co-author matches the pattern (via `commit_involves_author`).
/// - `lang_filter`: if `Some`, only include file changes whose language matches
///   (case-insensitive exact match). Commits with zero matching file changes
///   still count toward `total_commits` if they pass the author filter.
pub fn aggregate_commits(
    commits: &[CommitStats],
    period: &Period,
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> Vec<PeriodStats> {
    use crate::git::author::commit_involves_author;

    let mut buckets: HashMap<String, PeriodStats> = HashMap::new();

    for commit in commits {
        if let Some(pattern) = author_filter
            && !commit_involves_author(commit, pattern)
        {
            continue;
        }

        let label = bucket_timestamp(&commit.timestamp, period);
        let ps = buckets.entry(label.clone()).or_insert_with(|| PeriodStats {
            period_label: label,
            by_language: HashMap::new(),
            by_author: HashMap::new(),
            total_commits: 0,
            total_additions: 0,
            total_deletions: 0,
        });

        ps.total_commits += 1;

        let author_key = format!("{} <{}>", commit.author.name, commit.author.email);
        let author_entry = ps.by_author.entry(author_key).or_default();
        author_entry.commits += 1;

        for co in &commit.co_authors {
            let co_key = format!("{} <{}>", co.name, co.email);
            let co_entry = ps.by_author.entry(co_key).or_default();
            co_entry.co_authored_commits += 1;
        }

        for fc in &commit.file_changes {
            let lang = fc.language.as_deref().unwrap_or("Other").to_string();

            if let Some(lf) = lang_filter
                && !lang.eq_ignore_ascii_case(lf)
            {
                continue;
            }

            ps.total_additions += fc.additions;
            ps.total_deletions += fc.deletions;

            let lang_entry = ps.by_language.entry(lang.clone()).or_default();
            lang_entry.additions += fc.additions;
            lang_entry.deletions += fc.deletions;
            lang_entry.files_changed += 1;

            {
                let author_entry = ps.by_author.get_mut(&format!("{} <{}>", commit.author.name, commit.author.email)).expect("just inserted");
                author_entry.additions += fc.additions;
                author_entry.deletions += fc.deletions;

                let author_lang = author_entry.languages.entry(lang.clone()).or_default();
                author_lang.additions += fc.additions;
                author_lang.deletions += fc.deletions;
                author_lang.files_changed += 1;
            }

            for co in &commit.co_authors {
                let co_key = format!("{} <{}>", co.name, co.email);
                let co_entry = ps.by_author.get_mut(&co_key).expect("just inserted");
                co_entry.co_authored_additions += fc.additions;
                co_entry.co_authored_deletions += fc.deletions;

                let co_lang = co_entry.languages.entry(lang.clone()).or_default();
                co_lang.additions += fc.additions;
                co_lang.deletions += fc.deletions;
                co_lang.files_changed += 1;
            }
        }
    }

    let mut result: Vec<PeriodStats> = buckets.into_values().collect();
    result.sort_by(|a, b| a.period_label.cmp(&b.period_label));
    result
}

/// Merge all period stats into a single summary with `period_label = "Total"`.
pub fn aggregate_totals(period_stats: &[PeriodStats]) -> PeriodStats {
    let mut total = PeriodStats {
        period_label: "Total".to_string(),
        by_language: HashMap::new(),
        by_author: HashMap::new(),
        total_commits: 0,
        total_additions: 0,
        total_deletions: 0,
    };

    for ps in period_stats {
        total.total_commits += ps.total_commits;
        total.total_additions += ps.total_additions;
        total.total_deletions += ps.total_deletions;

        for (lang, ls) in &ps.by_language {
            let entry = total.by_language.entry(lang.clone()).or_default();
            entry.additions += ls.additions;
            entry.deletions += ls.deletions;
            entry.files_changed += ls.files_changed;
        }

        for (author_key, author_stats) in &ps.by_author {
            let entry: &mut AuthorStats = total.by_author.entry(author_key.clone()).or_default();
            entry.commits += author_stats.commits;
            entry.co_authored_commits += author_stats.co_authored_commits;
            entry.additions += author_stats.additions;
            entry.co_authored_additions += author_stats.co_authored_additions;
            entry.deletions += author_stats.deletions;
            entry.co_authored_deletions += author_stats.co_authored_deletions;

            for (lang, ls) in &author_stats.languages {
                let lang_entry = entry.languages.entry(lang.clone()).or_default();
                lang_entry.additions += ls.additions;
                lang_entry.deletions += ls.deletions;
                lang_entry.files_changed += ls.files_changed;
            }
        }
    }

    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn bucket_day() {
        let ts = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        assert_eq!(bucket_timestamp(&ts, &Period::Day), "2024-01-15");
    }

    #[test]
    fn bucket_week() {
        let ts = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        assert_eq!(bucket_timestamp(&ts, &Period::Week), "2024-W03");
    }

    #[test]
    fn bucket_month() {
        let ts = Utc.with_ymd_and_hms(2024, 1, 15, 10, 0, 0).unwrap();
        assert_eq!(bucket_timestamp(&ts, &Period::Month), "2024-01");
    }

    #[test]
    fn bucket_month_february() {
        let ts = Utc.with_ymd_and_hms(2024, 2, 1, 11, 0, 0).unwrap();
        assert_eq!(bucket_timestamp(&ts, &Period::Month), "2024-02");
    }

    #[test]
    fn bucket_week_year_boundary() {
        // Dec 31, 2024 is a Tuesday — ISO week 1 of 2025
        let ts = Utc.with_ymd_and_hms(2024, 12, 31, 0, 0, 0).unwrap();
        let result = bucket_timestamp(&ts, &Period::Week);
        // This should be "2025-W01" since Dec 31 2024 is in ISO week 1 of 2025
        assert_eq!(result, "2025-W01");
    }

    #[test]
    fn bucket_day_end_of_day() {
        let ts = Utc.with_ymd_and_hms(2024, 1, 15, 23, 59, 59).unwrap();
        assert_eq!(bucket_timestamp(&ts, &Period::Day), "2024-01-15");
    }

    use crate::stats::models::{Author, FileChange};

    fn make_commit(
        author_name: &str,
        author_email: &str,
        co_authors: Vec<Author>,
        ts: DateTime<Utc>,
        file_changes: Vec<FileChange>,
    ) -> CommitStats {
        CommitStats {
            repo: "test-repo".to_string(),
            oid: format!("{:x}", ts.timestamp()),
            author: Author {
                name: author_name.to_string(),
                email: author_email.to_string(),
            },
            committer: Author {
                name: author_name.to_string(),
                email: author_email.to_string(),
            },
            co_authors,
            timestamp: ts,
            message_subject: "test commit".to_string(),
            file_changes,
        }
    }

    fn rust_file(path: &str, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.to_string(),
            language: Some("Rust".to_string()),
            additions: adds,
            deletions: dels,
        }
    }

    fn py_file(path: &str, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.to_string(),
            language: Some("Python".to_string()),
            additions: adds,
            deletions: dels,
        }
    }

    fn no_lang_file(path: &str, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.to_string(),
            language: None,
            additions: adds,
            deletions: dels,
        }
    }

    #[test]
    fn aggregate_no_filters() {
        let commits = vec![
            make_commit(
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 10, 2), py_file("scripts/a.py", 5, 1)],
            ),
            make_commit(
                "Bob",
                "bob@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 20, 12, 0, 0).unwrap(),
                vec![rust_file("src/b.rs", 20, 5)],
            ),
            make_commit(
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 2, 5, 12, 0, 0).unwrap(),
                vec![py_file("scripts/b.py", 8, 3)],
            ),
        ];

        let result = aggregate_commits(&commits, &Period::Month, None, None);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].period_label, "2024-01");
        assert_eq!(result[0].total_commits, 2);
        assert_eq!(result[0].total_additions, 35);
        assert_eq!(result[0].total_deletions, 8);

        assert_eq!(result[1].period_label, "2024-02");
        assert_eq!(result[1].total_commits, 1);
        assert_eq!(result[1].total_additions, 8);
        assert_eq!(result[1].total_deletions, 3);

        assert!(result[0].by_language.contains_key("Rust"));
        assert!(result[0].by_language.contains_key("Python"));
        assert_eq!(result[0].by_language["Rust"].additions, 30);
        assert_eq!(result[0].by_language["Rust"].files_changed, 2);

        assert!(result[0].by_author.contains_key("Alice <alice@test.com>"));
        assert!(result[0].by_author.contains_key("Bob <bob@test.com>"));
        assert_eq!(result[0].by_author["Alice <alice@test.com>"].commits, 1);
        assert_eq!(result[0].by_author["Bob <bob@test.com>"].commits, 1);
    }

    #[test]
    fn aggregate_with_author_filter() {
        let commits = vec![
            make_commit(
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 10, 2)],
            ),
            make_commit(
                "Bob",
                "bob@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
                vec![rust_file("src/b.rs", 20, 5)],
            ),
            make_commit(
                "Charlie",
                "charlie@test.com",
                vec![Author {
                    name: "Alice".to_string(),
                    email: "alice@test.com".to_string(),
                }],
                Utc.with_ymd_and_hms(2024, 1, 20, 12, 0, 0).unwrap(),
                vec![rust_file("src/c.rs", 15, 4)],
            ),
        ];

        let result = aggregate_commits(&commits, &Period::Month, Some("alice"), None);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].total_commits, 2);
        assert_eq!(result[0].total_additions, 25);
        assert_eq!(result[0].total_deletions, 6);

        assert!(result[0].by_author.contains_key("Alice <alice@test.com>"));
        assert!(result[0]
            .by_author
            .contains_key("Charlie <charlie@test.com>"));
        assert!(!result[0].by_author.contains_key("Bob <bob@test.com>"));
    }

    #[test]
    fn aggregate_with_lang_filter() {
        let commits = vec![make_commit(
            "Alice",
            "alice@test.com",
            vec![],
            Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
            vec![
                rust_file("src/a.rs", 10, 2),
                py_file("scripts/a.py", 5, 1),
                no_lang_file("README", 3, 0),
            ],
        )];

        let result = aggregate_commits(&commits, &Period::Month, None, Some("rust"));

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].total_commits, 1);
        assert_eq!(result[0].total_additions, 10);
        assert_eq!(result[0].total_deletions, 2);

        assert_eq!(result[0].by_language.len(), 1);
        assert!(result[0].by_language.contains_key("Rust"));
        assert!(!result[0].by_language.contains_key("Python"));
    }

    #[test]
    fn aggregate_totals_merges_periods() {
        let commits = vec![
            make_commit(
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 10, 2)],
            ),
            make_commit(
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 2, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/b.rs", 20, 5), py_file("scripts/b.py", 8, 3)],
            ),
        ];

        let periods = aggregate_commits(&commits, &Period::Month, None, None);
        assert_eq!(periods.len(), 2);

        let totals = aggregate_totals(&periods);
        assert_eq!(totals.period_label, "Total");
        assert_eq!(totals.total_commits, 2);
        assert_eq!(totals.total_additions, 38);
        assert_eq!(totals.total_deletions, 10);

        assert_eq!(totals.by_language["Rust"].additions, 30);
        assert_eq!(totals.by_language["Rust"].files_changed, 2);
        assert_eq!(totals.by_language["Python"].additions, 8);
        assert_eq!(totals.by_language["Python"].files_changed, 1);

        assert_eq!(totals.by_author["Alice <alice@test.com>"].commits, 2);
        assert_eq!(totals.by_author["Alice <alice@test.com>"].additions, 38);
    }

    #[test]
    fn aggregate_empty_input() {
        let result = aggregate_commits(&[], &Period::Day, None, None);
        assert!(result.is_empty());

        let totals = aggregate_totals(&[]);
        assert_eq!(totals.period_label, "Total");
        assert_eq!(totals.total_commits, 0);
    }

    #[test]
    fn aggregate_no_lang_falls_back_to_other() {
        let commits = vec![make_commit(
            "Alice",
            "alice@test.com",
            vec![],
            Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
            vec![no_lang_file("Makefile", 5, 1)],
        )];

        let result = aggregate_commits(&commits, &Period::Month, None, None);

        assert_eq!(result[0].by_language.len(), 1);
        assert!(result[0].by_language.contains_key("Other"));
        assert_eq!(result[0].by_language["Other"].additions, 5);
    }
}
