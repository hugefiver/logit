use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Utc};

use crate::cli::{GroupBy, Period};
use crate::stats::models::{AuthorStats, CommitStats, GroupNode, LangStats, PeriodStats};

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
    aggregate_commits_with_bucket_key(commits, author_filter, lang_filter, |commit| {
        bucket_timestamp(&commit.timestamp, period)
    })
}

/// Aggregate a slice of commits into per-repo statistics.
///
/// The output shape is identical to period aggregation, but `period_label`
/// contains the repository name from `CommitStats.repo`.
pub fn aggregate_by_repo(
    commits: &[CommitStats],
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> Vec<PeriodStats> {
    aggregate_commits_with_bucket_key(commits, author_filter, lang_filter, |commit| {
        commit.repo.clone()
    })
}

pub fn aggregate_by_author(
    commits: &[CommitStats],
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> Vec<PeriodStats> {
    aggregate_commits_with_bucket_key(commits, author_filter, lang_filter, |commit| {
        commit.author.name.clone()
    })
}

fn aggregate_commits_with_bucket_key<F>(
    commits: &[CommitStats],
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
    bucket_key: F,
) -> Vec<PeriodStats>
where
    F: Fn(&CommitStats) -> String,
{
    use crate::git::author::commit_involves_author;

    let mut buckets: HashMap<String, PeriodStats> = HashMap::new();

    for commit in commits {
        if let Some(pattern) = author_filter
            && !commit_involves_author(commit, pattern)
        {
            continue;
        }

        let label = bucket_key(commit);
        let ps = buckets.entry(label.clone()).or_insert_with(|| PeriodStats {
            period_label: label,
            by_language: HashMap::new(),
            by_author: HashMap::new(),
            total_commits: 0,
            total_additions: 0,
            total_deletions: 0,
            total_net_modifications: 0,
            total_net_additions: 0,
        });

        ps.total_commits += 1;

        let author_key = commit.author.name.clone();
        let author_entry = ps.by_author.entry(author_key.clone()).or_default();
        author_entry.commits += 1;

        for co in &commit.co_authors {
            let co_key = co.name.clone();
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
            ps.total_net_modifications += fc.net_modifications;
            ps.total_net_additions += fc.net_additions;

            let lang_entry = ps.by_language.entry(lang.clone()).or_default();
            lang_entry.additions += fc.additions;
            lang_entry.deletions += fc.deletions;
            lang_entry.files_changed += 1;
            lang_entry.net_modifications += fc.net_modifications;
            lang_entry.net_additions += fc.net_additions;

            {
                let author_entry = ps.by_author.get_mut(&author_key).expect("just inserted");
                author_entry.additions += fc.additions;
                author_entry.deletions += fc.deletions;
                author_entry.net_modifications += fc.net_modifications;
                author_entry.net_additions += fc.net_additions;

                let author_lang = author_entry.languages.entry(lang.clone()).or_default();
                author_lang.additions += fc.additions;
                author_lang.deletions += fc.deletions;
                author_lang.files_changed += 1;
                author_lang.net_modifications += fc.net_modifications;
                author_lang.net_additions += fc.net_additions;
            }

            for co in &commit.co_authors {
                let co_key = co.name.clone();
                let co_entry = ps.by_author.get_mut(&co_key).expect("just inserted");
                co_entry.co_authored_additions += fc.additions;
                co_entry.co_authored_deletions += fc.deletions;
                co_entry.co_authored_net_modifications += fc.net_modifications;
                co_entry.co_authored_net_additions += fc.net_additions;

                let co_lang = co_entry.languages.entry(lang.clone()).or_default();
                co_lang.additions += fc.additions;
                co_lang.deletions += fc.deletions;
                co_lang.files_changed += 1;
                co_lang.net_modifications += fc.net_modifications;
                co_lang.net_additions += fc.net_additions;

                let co_lang2 = co_entry.co_authored_languages.entry(lang.clone()).or_default();
                co_lang2.additions += fc.additions;
                co_lang2.deletions += fc.deletions;
                co_lang2.files_changed += 1;
                co_lang2.net_modifications += fc.net_modifications;
                co_lang2.net_additions += fc.net_additions;
            }
        }
    }

    let mut result: Vec<PeriodStats> = buckets.into_values().collect();
    result.sort_by(|a, b| a.period_label.cmp(&b.period_label));
    result
}

/// Remove excluded languages (case-insensitive) from period rows and totals.
///
/// This removes matching languages from:
/// - `PeriodStats.by_language`
/// - each author's `AuthorStats.languages`
///
/// and adjusts additions/deletions counters accordingly.
pub fn filter_excluded_languages(
    stats: &mut Vec<PeriodStats>,
    totals: &mut PeriodStats,
    excluded: &[String],
) {
    if excluded.is_empty() {
        return;
    }

    for period in stats {
        remove_excluded_from_period(period, excluded);
    }

    remove_excluded_from_period(totals, excluded);
}

pub fn remove_excluded_from_period(period: &mut PeriodStats, excluded: &[String]) {
    for lang in excluded {
        if let Some(removed) = remove_language_case_insensitive(&mut period.by_language, lang) {
            period.total_additions = period.total_additions.saturating_sub(removed.additions);
            period.total_deletions = period.total_deletions.saturating_sub(removed.deletions);
            period.total_net_modifications = period
                .total_net_modifications
                .saturating_sub(removed.net_modifications);
            period.total_net_additions = period
                .total_net_additions
                .saturating_sub(removed.net_additions);
        }

        for author_stats in period.by_author.values_mut() {
            if let Some(removed) = remove_language_case_insensitive(&mut author_stats.languages, lang)
            {
                let co_removed = remove_language_case_insensitive(
                    &mut author_stats.co_authored_languages,
                    lang,
                )
                .unwrap_or_default();

                let primary_adds = removed.additions.saturating_sub(co_removed.additions);
                let primary_dels = removed.deletions.saturating_sub(co_removed.deletions);
                let primary_net_mods =
                    removed.net_modifications.saturating_sub(co_removed.net_modifications);
                let primary_net_adds =
                    removed.net_additions.saturating_sub(co_removed.net_additions);

                author_stats.additions = author_stats.additions.saturating_sub(primary_adds);
                author_stats.deletions = author_stats.deletions.saturating_sub(primary_dels);
                author_stats.net_modifications = author_stats
                    .net_modifications
                    .saturating_sub(primary_net_mods);
                author_stats.net_additions = author_stats
                    .net_additions
                    .saturating_sub(primary_net_adds);

                author_stats.co_authored_additions = author_stats
                    .co_authored_additions
                    .saturating_sub(co_removed.additions);
                author_stats.co_authored_deletions = author_stats
                    .co_authored_deletions
                    .saturating_sub(co_removed.deletions);
                author_stats.co_authored_net_modifications = author_stats
                    .co_authored_net_modifications
                    .saturating_sub(co_removed.net_modifications);
                author_stats.co_authored_net_additions = author_stats
                    .co_authored_net_additions
                    .saturating_sub(co_removed.net_additions);
            }
        }
    }
}

fn remove_language_case_insensitive(
    map: &mut HashMap<String, LangStats>,
    lang: &str,
) -> Option<LangStats> {
    let keys: Vec<String> = map
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(lang))
        .cloned()
        .collect();

    if keys.is_empty() {
        return None;
    }

    let mut removed_total = LangStats::default();
    for key in keys {
        if let Some(removed) = map.remove(&key) {
            removed_total.additions += removed.additions;
            removed_total.deletions += removed.deletions;
            removed_total.files_changed += removed.files_changed;
            removed_total.net_modifications += removed.net_modifications;
            removed_total.net_additions += removed.net_additions;
        }
    }

    Some(removed_total)
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
        total_net_modifications: 0,
        total_net_additions: 0,
    };

    for ps in period_stats {
        total.total_commits += ps.total_commits;
        total.total_additions += ps.total_additions;
        total.total_deletions += ps.total_deletions;
        total.total_net_modifications += ps.total_net_modifications;
        total.total_net_additions += ps.total_net_additions;

        for (lang, ls) in &ps.by_language {
            let entry = total.by_language.entry(lang.clone()).or_default();
            entry.additions += ls.additions;
            entry.deletions += ls.deletions;
            entry.files_changed += ls.files_changed;
            entry.net_modifications += ls.net_modifications;
            entry.net_additions += ls.net_additions;
        }

        for (author_key, author_stats) in &ps.by_author {
            let entry: &mut AuthorStats = total.by_author.entry(author_key.clone()).or_default();
            entry.commits += author_stats.commits;
            entry.co_authored_commits += author_stats.co_authored_commits;
            entry.additions += author_stats.additions;
            entry.co_authored_additions += author_stats.co_authored_additions;
            entry.deletions += author_stats.deletions;
            entry.co_authored_deletions += author_stats.co_authored_deletions;
            entry.net_modifications += author_stats.net_modifications;
            entry.co_authored_net_modifications += author_stats.co_authored_net_modifications;
            entry.net_additions += author_stats.net_additions;
            entry.co_authored_net_additions += author_stats.co_authored_net_additions;

            for (lang, ls) in &author_stats.languages {
                let lang_entry = entry.languages.entry(lang.clone()).or_default();
                lang_entry.additions += ls.additions;
                lang_entry.deletions += ls.deletions;
                lang_entry.files_changed += ls.files_changed;
                lang_entry.net_modifications += ls.net_modifications;
                lang_entry.net_additions += ls.net_additions;
            }

            for (lang, ls) in &author_stats.co_authored_languages {
                let lang_entry =
                    entry.co_authored_languages.entry(lang.clone()).or_default();
                lang_entry.additions += ls.additions;
                lang_entry.deletions += ls.deletions;
                lang_entry.files_changed += ls.files_changed;
                lang_entry.net_modifications += ls.net_modifications;
                lang_entry.net_additions += ls.net_additions;
            }
        }
    }

    total
}

fn group_key(commit: &CommitStats, group: &GroupBy, period: &Period) -> String {
    match group {
        GroupBy::Repo => commit.repo.clone(),
        GroupBy::Author => commit.author.name.clone(),
        GroupBy::Period => bucket_timestamp(&commit.timestamp, period),
        GroupBy::Language => unreachable!("Language is not a tree-level partition"),
    }
}

pub fn effective_groups(
    commits: &[CommitStats],
    groups: &[GroupBy],
    period: &Period,
) -> Vec<GroupBy> {
    if groups.len() <= 1 {
        return groups.to_vec();
    }
    let filtered: Vec<GroupBy> = groups
        .iter()
        .filter(|g| {
            if matches!(g, GroupBy::Language) {
                return true;
            }
            let unique: HashSet<String> =
                commits.iter().map(|c| group_key(c, g, period)).collect();
            unique.len() > 1
        })
        .copied()
        .collect();

    if filtered.is_empty() {
        vec![*groups.last().unwrap()]
    } else {
        filtered
    }
}

pub fn validate_groups(groups: &[GroupBy]) -> Result<(), String> {
    if groups.len() <= 1 {
        return Ok(());
    }
    let mut seen = HashSet::new();
    for g in groups {
        if !seen.insert(g) {
            return Err(format!("Duplicate --group level: {:?}", g));
        }
    }
    for (i, g) in groups.iter().enumerate() {
        if matches!(g, GroupBy::Language) && i < groups.len() - 1 {
            return Err(
                "Language can only be the last --group level (one commit spans multiple languages)"
                    .to_string(),
            );
        }
    }
    Ok(())
}

pub fn build_group_tree(
    commits: &[CommitStats],
    groups: &[GroupBy],
    period: &Period,
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> Vec<GroupNode> {
    if groups.is_empty() {
        return vec![];
    }
    // Trailing Language collapses into the leaf node's by_language breakdown,
    // which the renderer prints automatically. A separate partition level
    // would just duplicate the parent.
    let effective: Vec<GroupBy> = if groups.len() > 1 && matches!(groups.last(), Some(GroupBy::Language)) {
        groups[..groups.len() - 1].to_vec()
    } else {
        groups.to_vec()
    };
    let mut nodes = build_group_tree_inner(commits, &effective, period, author_filter, lang_filter);
    prune_empty_nodes(&mut nodes);
    nodes
}

fn build_group_tree_inner(
    commits: &[CommitStats],
    groups: &[GroupBy],
    period: &Period,
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> Vec<GroupNode> {
    let current_group = &groups[0];
    let remaining = &groups[1..];

    if remaining.is_empty() {
        let rows = match current_group {
            GroupBy::Repo => aggregate_by_repo(commits, author_filter, lang_filter),
            GroupBy::Author => aggregate_by_author(commits, author_filter, lang_filter),
            GroupBy::Period | GroupBy::Language => {
                aggregate_commits(commits, period, author_filter, lang_filter)
            }
        };
        return rows
            .into_iter()
            .map(|s| GroupNode {
                label: s.period_label.clone(),
                stats: s,
                children: vec![],
            })
            .collect();
    }

    let mut partitions: HashMap<String, Vec<CommitStats>> = HashMap::new();
    for commit in commits {
        let key = group_key(commit, current_group, period);
        partitions.entry(key).or_default().push(commit.clone());
    }

    let mut nodes: Vec<GroupNode> = partitions
        .into_iter()
        .map(|(key, partition_commits)| {
            let children = build_group_tree_inner(
                &partition_commits,
                remaining,
                period,
                author_filter,
                lang_filter,
            );
            let child_stats: Vec<PeriodStats> =
                children.iter().map(|c| c.stats.clone()).collect();
            let stats = aggregate_totals(&child_stats);
            GroupNode {
                label: key,
                stats,
                children,
            }
        })
        .collect();

    nodes.sort_by(|a, b| a.label.cmp(&b.label));
    nodes
}

fn prune_empty_nodes(nodes: &mut Vec<GroupNode>) {
    for node in nodes.iter_mut() {
        prune_empty_nodes(&mut node.children);
    }
    nodes.retain(|n| n.stats.total_commits > 0);
}

pub fn filter_excluded_languages_tree(nodes: &mut [GroupNode], excluded: &[String]) {
    if excluded.is_empty() {
        return;
    }
    for node in nodes.iter_mut() {
        remove_excluded_from_period(&mut node.stats, excluded);
        if !node.children.is_empty() {
            filter_excluded_languages_tree(&mut node.children, excluded);
        }
    }
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

    fn make_commit_in_repo(
        repo: &str,
        author_name: &str,
        author_email: &str,
        co_authors: Vec<Author>,
        ts: DateTime<Utc>,
        file_changes: Vec<FileChange>,
    ) -> CommitStats {
        let mut commit = make_commit(author_name, author_email, co_authors, ts, file_changes);
        commit.repo = repo.to_string();
        commit
    }

    fn rust_file(path: &str, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.to_string(),
            language: Some("Rust".to_string()),
            additions: adds,
            deletions: dels,
            net_modifications: adds.max(dels),
            net_additions: adds.saturating_sub(dels),
        }
    }

    fn py_file(path: &str, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.to_string(),
            language: Some("Python".to_string()),
            additions: adds,
            deletions: dels,
            net_modifications: adds.max(dels),
            net_additions: adds.saturating_sub(dels),
        }
    }

    fn no_lang_file(path: &str, adds: u64, dels: u64) -> FileChange {
        FileChange {
            path: path.to_string(),
            language: None,
            additions: adds,
            deletions: dels,
            net_modifications: adds.max(dels),
            net_additions: adds.saturating_sub(dels),
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

        assert!(result[0].by_author.contains_key("Alice"));
        assert!(result[0].by_author.contains_key("Bob"));
        assert_eq!(result[0].by_author["Alice"].commits, 1);
        assert_eq!(result[0].by_author["Bob"].commits, 1);
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

        assert!(result[0].by_author.contains_key("Alice"));
        assert!(result[0]
            .by_author
            .contains_key("Charlie"));
        assert!(!result[0].by_author.contains_key("Bob"));
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

        assert_eq!(totals.by_author["Alice"].commits, 2);
        assert_eq!(totals.by_author["Alice"].additions, 38);
    }

    #[test]
    fn aggregate_totals_merges_co_authored_languages() {
        let bob = Author {
            name: "Bob".to_string(),
            email: "bob@test.com".to_string(),
        };
        let commits = vec![
            make_commit(
                "Alice",
                "alice@test.com",
                vec![bob.clone()],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 10, 2)],
            ),
            make_commit(
                "Alice",
                "alice@test.com",
                vec![bob.clone()],
                Utc.with_ymd_and_hms(2024, 2, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/b.rs", 20, 5)],
            ),
        ];

        let periods = aggregate_commits(&commits, &Period::Month, None, None);
        let totals = aggregate_totals(&periods);

        // Bob appears as co-author across both periods
        let bob_stats = &totals.by_author["Bob"];
        assert_eq!(bob_stats.co_authored_additions, 30);
        assert_eq!(bob_stats.co_authored_deletions, 7);

        // co_authored_languages should be merged across periods
        assert!(
            bob_stats.co_authored_languages.contains_key("Rust"),
            "co_authored_languages should contain Rust after merging"
        );
        let co_rust = &bob_stats.co_authored_languages["Rust"];
        assert_eq!(co_rust.additions, 30);
        assert_eq!(co_rust.deletions, 7);
        assert_eq!(co_rust.files_changed, 2);
        assert_eq!(co_rust.net_modifications, 30); // max(10,2) + max(20,5) = 10+20
        assert_eq!(co_rust.net_additions, 23); // (10-2) + (20-5) = 8+15
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

    #[test]
    fn aggregate_by_repo_groups_by_repo_name() {
        let commits = vec![
            make_commit_in_repo(
                "repo-z",
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 10, 2)],
            ),
            make_commit_in_repo(
                "repo-a",
                "Bob",
                "bob@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 20, 12, 0, 0).unwrap(),
                vec![py_file("scripts/a.py", 7, 1)],
            ),
            make_commit_in_repo(
                "repo-z",
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 2, 5, 12, 0, 0).unwrap(),
                vec![py_file("scripts/b.py", 5, 3)],
            ),
        ];

        let result = aggregate_by_repo(&commits, None, None);

        assert_eq!(result.len(), 2);
        // Sorted by repo label ascending
        assert_eq!(result[0].period_label, "repo-a");
        assert_eq!(result[0].total_commits, 1);
        assert_eq!(result[0].total_additions, 7);
        assert_eq!(result[0].total_deletions, 1);
        assert_eq!(result[0].by_language["Python"].additions, 7);

        assert_eq!(result[1].period_label, "repo-z");
        assert_eq!(result[1].total_commits, 2);
        assert_eq!(result[1].total_additions, 15);
        assert_eq!(result[1].total_deletions, 5);
        assert_eq!(result[1].by_language["Rust"].additions, 10);
        assert_eq!(result[1].by_language["Python"].additions, 5);
    }

    #[test]
    fn filter_excluded_languages_removes_from_periods_totals_and_authors() {
        let commits = vec![
            make_commit_in_repo(
                "repo-a",
                "Alice",
                "alice@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 10, 2), py_file("scripts/a.py", 5, 1)],
            ),
            make_commit_in_repo(
                "repo-b",
                "Bob",
                "bob@test.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 12, 12, 0, 0).unwrap(),
                vec![rust_file("src/b.rs", 7, 3)],
            ),
        ];

        let mut periods = aggregate_by_repo(&commits, None, None);
        let mut totals = aggregate_totals(&periods);

        let excluded = vec!["rUsT".to_string()];
        filter_excluded_languages(&mut periods, &mut totals, &excluded);

        for period in &periods {
            assert!(!period.by_language.contains_key("Rust"));
            for author in period.by_author.values() {
                assert!(!author.languages.contains_key("Rust"));
            }
        }

        // Only Python stats should remain: +5/-1
        assert_eq!(totals.total_additions, 5);
        assert_eq!(totals.total_deletions, 1);
        assert!(!totals.by_language.contains_key("Rust"));
        assert!(totals.by_language.contains_key("Python"));

        let alice = totals.by_author.get("Alice").unwrap();
        assert_eq!(alice.additions, 5);
        assert_eq!(alice.deletions, 1);
        assert!(!alice.languages.contains_key("Rust"));
        assert!(alice.languages.contains_key("Python"));

        let bob = totals.by_author.get("Bob").unwrap();
        assert_eq!(bob.additions, 0);
        assert_eq!(bob.deletions, 0);
        assert!(!bob.languages.contains_key("Rust"));
    }
}
