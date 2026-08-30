use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Utc};

use crate::cli::{GroupBy, Period};
use crate::git::author::commit_involves_author;
use crate::stats::models::{AuthorStats, CommitStats, GroupNode, LangStats, PeriodStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupSource {
    Local,
    Github,
}

#[derive(Debug, Clone, Copy)]
pub struct GroupCardinality {
    pub repo: usize,
    pub author: usize,
    pub period: usize,
    pub language: usize,
}

impl GroupCardinality {
    fn for_group(&self, group: GroupBy) -> usize {
        match group {
            GroupBy::Repo => self.repo,
            GroupBy::Author => self.author,
            GroupBy::Period => self.period,
            GroupBy::Language => self.language,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    pub primary: GroupBy,
    pub levels: Vec<GroupBy>,
    pub hierarchical: bool,
}

fn group_label(group: GroupBy) -> &'static str {
    match group {
        GroupBy::Repo => "repo",
        GroupBy::Author => "author",
        GroupBy::Period => "period",
        GroupBy::Language => "language",
    }
}

fn supports_group(source: GroupSource, group: GroupBy) -> bool {
    !matches!((source, group), (GroupSource::Github, GroupBy::Author))
}

pub fn validate_group_source(
    primary_candidates: &[GroupBy],
    subgroups: &[GroupBy],
    source: GroupSource,
) -> Result<(), String> {
    for group in primary_candidates.iter().chain(subgroups) {
        if !supports_group(source, *group) {
            return Err(format!(
                "Grouping by {} is not supported: GitHub contribution records have no author identity",
                group_label(*group)
            ));
        }
    }
    Ok(())
}

fn validate_language_is_final(groups: &[GroupBy], flag: &str) -> Result<(), String> {
    if groups
        .iter()
        .enumerate()
        .any(|(index, group)| matches!(group, GroupBy::Language) && index + 1 < groups.len())
    {
        return Err(format!(
            "Language can only be the final {flag} level (one commit spans multiple languages)"
        ));
    }
    Ok(())
}

/// Resolves ordered `--group` fallback candidates and `--groups` subgroup levels
/// into one source-aware grouping plan.
pub fn resolve_group_plan(
    primary_candidates: &[GroupBy],
    subgroups: &[GroupBy],
    counts: &GroupCardinality,
    source: GroupSource,
) -> Result<GroupPlan, String> {
    validate_group_source(primary_candidates, subgroups, source)?;

    let mut primary_seen = HashSet::new();
    for group in primary_candidates {
        if !primary_seen.insert(group) {
            return Err(format!("Duplicate --group fallback: {group:?}"));
        }
    }
    validate_language_is_final(primary_candidates, "--group fallback")?;
    validate_language_is_final(subgroups, "--groups")?;

    let non_language_candidates: Vec<GroupBy> = primary_candidates
        .iter()
        .copied()
        .filter(|group| !matches!(group, GroupBy::Language))
        .collect();
    let primary = non_language_candidates
        .iter()
        .copied()
        .find(|group| counts.for_group(*group) > 1)
        .or_else(|| (primary_candidates.len() == 1).then(|| primary_candidates[0]))
        .unwrap_or(GroupBy::Language);

    let mut levels = vec![primary];
    let mut seen = HashSet::from([primary]);
    let mut removed_selected_primary = false;
    for group in subgroups {
        if *group == primary && !removed_selected_primary {
            removed_selected_primary = true;
            continue;
        }
        if !seen.insert(*group) {
            return Err(format!("Duplicate --groups level: {group:?}"));
        }
        if counts.for_group(*group) > 1 {
            levels.push(*group);
        }
    }

    validate_language_is_final(&levels, "grouping")?;
    let hierarchical = levels.len() > 1;
    Ok(GroupPlan {
        primary,
        levels,
        hierarchical,
    })
}

/// Count distinct values for every local grouping dimension. Repository labels
/// deliberately use the display label, while authors retain their full identity.
pub fn local_group_cardinality(
    commits: &[CommitStats],
    period: &Period,
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> GroupCardinality {
    let mut repos = HashSet::new();
    let mut authors = HashSet::new();
    let mut periods = HashSet::new();
    let mut languages = HashSet::new();

    for commit in commits {
        if !commit_matches_filters(commit, author_filter, lang_filter) {
            continue;
        }

        repos.insert(commit.repo.as_str());
        authors.insert(commit.author.to_string());
        authors.extend(commit.co_authors.iter().map(ToString::to_string));
        periods.insert(bucket_timestamp(&commit.timestamp, period));
        for file_change in &commit.file_changes {
            if language_matches_filter(file_change.language.as_deref(), lang_filter) {
                languages.insert(file_change.language.as_deref().unwrap_or("Other"));
            }
        }
    }

    GroupCardinality {
        repo: repos.len(),
        author: authors.len(),
        period: periods.len(),
        language: languages.len(),
    }
}

fn commit_matches_filters(
    commit: &CommitStats,
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
) -> bool {
    author_filter.is_none_or(|pattern| commit_involves_author(commit, pattern))
        && lang_filter.is_none_or(|language| {
            commit.file_changes.iter().any(|file_change| {
                language_matches_filter(file_change.language.as_deref(), Some(language))
            })
        })
}

fn language_matches_filter(language: Option<&str>, lang_filter: Option<&str>) -> bool {
    lang_filter
        .is_none_or(|filter| language.is_some_and(|language| language.eq_ignore_ascii_case(filter)))
}

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
///   (case-insensitive exact match), and drop commits with no matching files.
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

fn aggregate_commits_with_bucket_key<F>(
    commits: &[CommitStats],
    author_filter: Option<&str>,
    lang_filter: Option<&str>,
    bucket_key: F,
) -> Vec<PeriodStats>
where
    F: Fn(&CommitStats) -> String,
{
    let mut buckets: HashMap<String, PeriodStats> = HashMap::new();

    for commit in commits {
        if !commit_matches_filters(commit, author_filter, lang_filter) {
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

        let author_key = commit.author.to_string();
        let author_entry = ps.by_author.entry(author_key.clone()).or_default();
        author_entry.commits += 1;

        for co in &commit.co_authors {
            let co_key = co.to_string();
            let co_entry = ps.by_author.entry(co_key).or_default();
            co_entry.co_authored_commits += 1;
        }

        for fc in &commit.file_changes {
            let lang = fc.language.as_deref().unwrap_or("Other").to_string();

            if !language_matches_filter(fc.language.as_deref(), lang_filter) {
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
                let co_key = co.to_string();
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

                let co_lang2 = co_entry
                    .co_authored_languages
                    .entry(lang.clone())
                    .or_default();
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
            if let Some(removed) =
                remove_language_case_insensitive(&mut author_stats.languages, lang)
            {
                let co_removed =
                    remove_language_case_insensitive(&mut author_stats.co_authored_languages, lang)
                        .unwrap_or_default();

                let primary_adds = removed.additions.saturating_sub(co_removed.additions);
                let primary_dels = removed.deletions.saturating_sub(co_removed.deletions);
                let primary_net_mods = removed
                    .net_modifications
                    .saturating_sub(co_removed.net_modifications);
                let primary_net_adds = removed
                    .net_additions
                    .saturating_sub(co_removed.net_additions);

                author_stats.additions = author_stats.additions.saturating_sub(primary_adds);
                author_stats.deletions = author_stats.deletions.saturating_sub(primary_dels);
                author_stats.net_modifications = author_stats
                    .net_modifications
                    .saturating_sub(primary_net_mods);
                author_stats.net_additions =
                    author_stats.net_additions.saturating_sub(primary_net_adds);

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
                let lang_entry = entry.co_authored_languages.entry(lang.clone()).or_default();
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

fn group_keys(commit: &CommitStats, group: &GroupBy, period: &Period) -> Vec<String> {
    match group {
        GroupBy::Repo => vec![commit.repo.clone()],
        GroupBy::Author => {
            let mut keys = vec![commit.author.to_string()];
            for co_author in &commit.co_authors {
                let key = co_author.to_string();
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            keys
        }
        GroupBy::Period => vec![bucket_timestamp(&commit.timestamp, period)],
        GroupBy::Language => unreachable!("Language is not a tree-level partition"),
    }
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
    let effective: Vec<GroupBy> =
        if groups.len() > 1 && matches!(groups.last(), Some(GroupBy::Language)) {
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

    if matches!(current_group, GroupBy::Language) {
        debug_assert!(remaining.is_empty());
        return aggregate_commits(commits, period, author_filter, lang_filter)
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
        for key in group_keys(commit, current_group, period) {
            partitions.entry(key).or_default().push(commit.clone());
        }
    }

    let mut nodes: Vec<GroupNode> = partitions
        .into_iter()
        .map(|(key, partition_commits)| {
            let mut stats = aggregate_totals(&aggregate_commits(
                &partition_commits,
                period,
                author_filter,
                lang_filter,
            ));
            stats.period_label = key.clone();
            let children = if remaining.is_empty() {
                Vec::new()
            } else {
                build_group_tree_inner(
                    &partition_commits,
                    remaining,
                    period,
                    author_filter,
                    lang_filter,
                )
            };
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
            repo_id: "test-repo-id".to_string(),
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
        assert!(
            result[0]
                .by_author
                .contains_key("Charlie <charlie@test.com>")
        );
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
    fn same_name_different_email_identities_survive_raw_aggregation() {
        let commits = vec![
            make_commit(
                "Alex",
                "alex.one@example.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
                vec![rust_file("src/one.rs", 10, 2)],
            ),
            make_commit(
                "Alex",
                "alex.two@example.com",
                vec![],
                Utc.with_ymd_and_hms(2024, 1, 11, 12, 0, 0).unwrap(),
                vec![rust_file("src/two.rs", 5, 1)],
            ),
        ];

        let result = aggregate_commits(&commits, &Period::Month, None, None);

        assert_eq!(result.len(), 1);
        assert!(
            result[0]
                .by_author
                .contains_key("Alex <alex.one@example.com>")
        );
        assert!(
            result[0]
                .by_author
                .contains_key("Alex <alex.two@example.com>")
        );
    }

    #[test]
    fn language_filter_drops_commit_without_matching_file_language() {
        let commits = vec![make_commit(
            "Alice",
            "alice@test.com",
            vec![],
            Utc.with_ymd_and_hms(2024, 1, 10, 12, 0, 0).unwrap(),
            vec![py_file("scripts/only.py", 10, 2)],
        )];

        let result = aggregate_commits(&commits, &Period::Month, None, Some("Rust"));

        assert!(result.is_empty());
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
        let bob_stats = &totals.by_author["Bob <bob@test.com>"];
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

        let alice = totals.by_author.get("Alice <alice@test.com>").unwrap();
        assert_eq!(alice.additions, 5);
        assert_eq!(alice.deletions, 1);
        assert!(!alice.languages.contains_key("Rust"));
        assert!(alice.languages.contains_key("Python"));

        let bob = totals.by_author.get("Bob <bob@test.com>").unwrap();
        assert_eq!(bob.additions, 0);
        assert_eq!(bob.deletions, 0);
        assert!(!bob.languages.contains_key("Rust"));
    }

    #[test]
    fn group_plan_keeps_group_as_fallback_and_groups_as_sublevels() {
        let counts = GroupCardinality {
            repo: 1,
            author: 2,
            period: 3,
            language: 4,
        };

        let plan = resolve_group_plan(
            &[GroupBy::Repo, GroupBy::Author, GroupBy::Language],
            &[GroupBy::Author, GroupBy::Period],
            &counts,
            GroupSource::Local,
        )
        .unwrap();

        assert_eq!(plan.primary, GroupBy::Author);
        assert_eq!(plan.levels, vec![GroupBy::Author, GroupBy::Period]);
        assert!(plan.hierarchical);
    }

    #[test]
    fn language_only_group_plan_with_zero_cardinality_is_flat() {
        let plan = resolve_group_plan(
            &[GroupBy::Language],
            &[],
            &GroupCardinality {
                repo: 0,
                author: 0,
                period: 0,
                language: 0,
            },
            GroupSource::Local,
        )
        .unwrap();

        assert_eq!(
            plan,
            GroupPlan {
                primary: GroupBy::Language,
                levels: vec![GroupBy::Language],
                hierarchical: false,
            }
        );
    }

    #[test]
    fn group_plan_multi_candidate_fallback_uses_language_over_unique_repo() {
        let counts = GroupCardinality {
            repo: 1,
            author: 0,
            period: 0,
            language: 2,
        };

        for source in [GroupSource::Local, GroupSource::Github] {
            let plan =
                resolve_group_plan(&[GroupBy::Repo, GroupBy::Language], &[], &counts, source)
                    .unwrap();
            assert_eq!(plan.primary, GroupBy::Language);
            assert_eq!(plan.levels, vec![GroupBy::Language]);
            assert!(!plan.hierarchical);
        }

        let plan = resolve_group_plan(&[GroupBy::Repo], &[], &counts, GroupSource::Local).unwrap();
        assert_eq!(plan.primary, GroupBy::Repo);
    }

    #[test]
    fn single_explicit_primary_is_preserved_when_unique() {
        let plan = resolve_group_plan(
            &[GroupBy::Author],
            &[GroupBy::Language],
            &GroupCardinality {
                repo: 1,
                author: 1,
                period: 2,
                language: 2,
            },
            GroupSource::Local,
        )
        .unwrap();

        assert_eq!(plan.primary, GroupBy::Author);
        assert_eq!(plan.levels, vec![GroupBy::Author, GroupBy::Language]);
        assert!(plan.hierarchical);
    }

    #[test]
    fn duplicate_selected_primary_is_removed_but_other_duplicate_errors() {
        let counts = GroupCardinality {
            repo: 2,
            author: 1,
            period: 2,
            language: 2,
        };

        let plan = resolve_group_plan(
            &[GroupBy::Repo, GroupBy::Language],
            &[GroupBy::Repo, GroupBy::Period],
            &counts,
            GroupSource::Local,
        )
        .unwrap();
        assert_eq!(plan.levels, vec![GroupBy::Repo, GroupBy::Period]);

        let error = resolve_group_plan(
            &[GroupBy::Repo, GroupBy::Language],
            &[GroupBy::Period, GroupBy::Period],
            &counts,
            GroupSource::Local,
        )
        .unwrap_err();
        assert!(error.contains("Period"), "error: {error}");
    }

    #[test]
    fn github_explicit_author_group_is_actionable_error() {
        for (candidates, subgroups) in [
            (
                vec![GroupBy::Repo, GroupBy::Author, GroupBy::Language],
                vec![],
            ),
            (
                vec![GroupBy::Repo, GroupBy::Language],
                vec![GroupBy::Author],
            ),
        ] {
            let error = resolve_group_plan(
                &candidates,
                &subgroups,
                &GroupCardinality {
                    repo: 2,
                    author: 0,
                    period: 2,
                    language: 2,
                },
                GroupSource::Github,
            )
            .unwrap_err();

            assert!(error.contains("author"), "error: {error}");
            assert!(
                error.contains("GitHub contribution records have no author identity"),
                "error: {error}"
            );
        }
    }

    #[test]
    fn unique_subgroups_are_skipped_for_flat_and_hierarchical_paths() {
        let flat = resolve_group_plan(
            &[GroupBy::Repo, GroupBy::Language],
            &[GroupBy::Author, GroupBy::Period],
            &GroupCardinality {
                repo: 2,
                author: 1,
                period: 1,
                language: 2,
            },
            GroupSource::Local,
        )
        .unwrap();
        assert_eq!(flat.levels, vec![GroupBy::Repo]);
        assert!(!flat.hierarchical);

        let hierarchical = resolve_group_plan(
            &[GroupBy::Repo, GroupBy::Language],
            &[GroupBy::Author, GroupBy::Period],
            &GroupCardinality {
                repo: 2,
                author: 2,
                period: 1,
                language: 2,
            },
            GroupSource::Local,
        )
        .unwrap();
        assert_eq!(hierarchical.levels, vec![GroupBy::Repo, GroupBy::Author]);
        assert!(hierarchical.hierarchical);
    }

    #[test]
    fn group_node_tree_follows_plan_level_order_and_preserves_totals() {
        let commits = vec![
            make_commit_in_repo(
                "repo-a",
                "Alice",
                "alice@example.com",
                vec![],
                Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 7, 2)],
            ),
            make_commit_in_repo(
                "repo-b",
                "Bob",
                "bob@example.com",
                vec![],
                Utc.with_ymd_and_hms(2025, 2, 15, 12, 0, 0).unwrap(),
                vec![rust_file("src/b.rs", 5, 1)],
            ),
        ];
        let plan = GroupPlan {
            primary: GroupBy::Repo,
            levels: vec![GroupBy::Repo, GroupBy::Period, GroupBy::Language],
            hierarchical: true,
        };

        let nodes = build_group_tree(&commits, &plan.levels, &Period::Month, None, None);

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].label, "repo-a");
        assert_eq!(nodes[0].children[0].label, "2025-01");
        assert_eq!(nodes[1].label, "repo-b");
        assert_eq!(nodes[1].children[0].label, "2025-02");
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.stats.total_commits)
                .sum::<u64>(),
            2
        );
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.stats.total_additions)
                .sum::<u64>(),
            12
        );
        assert_eq!(nodes[0].children[0].stats.by_language["Rust"].additions, 7);
    }

    #[test]
    fn group_plan_author_tree_includes_coauthors_without_double_counting_ancestors() {
        let commits = vec![make_commit_in_repo(
            "repo-a",
            "Alice",
            "alice@example.com",
            vec![Author {
                name: "Bob".to_string(),
                email: "bob@example.com".to_string(),
            }],
            Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            vec![rust_file("src/a.rs", 7, 2)],
        )];
        let levels = [GroupBy::Repo, GroupBy::Author, GroupBy::Language];

        let nodes = build_group_tree(&commits, &levels, &Period::Month, None, None);

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].label, "repo-a");
        assert_eq!(nodes[0].stats.total_commits, 1);
        assert_eq!(
            nodes[0]
                .children
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice <alice@example.com>", "Bob <bob@example.com>"]
        );
        for author in &nodes[0].children {
            assert_eq!(author.stats.total_commits, 1);
            assert_eq!(author.stats.total_additions, 7);
            assert!(author.stats.by_language.contains_key("Rust"));
        }

        let root_author_nodes = build_group_tree(
            &commits,
            &[GroupBy::Author, GroupBy::Language],
            &Period::Month,
            None,
            None,
        );
        assert_eq!(
            root_author_nodes
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice <alice@example.com>", "Bob <bob@example.com>"]
        );
    }

    #[test]
    fn local_group_cardinality_uses_display_repos_full_authors_periods_and_languages() {
        let commits = vec![
            make_commit_in_repo(
                "left/service",
                "Alex",
                "alex.one@example.com",
                vec![],
                Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 1, 0)],
            ),
            make_commit_in_repo(
                "right/service",
                "Alex",
                "alex.two@example.com",
                vec![],
                Utc.with_ymd_and_hms(2025, 2, 15, 12, 0, 0).unwrap(),
                vec![py_file("src/b.py", 1, 0)],
            ),
        ];

        let counts = local_group_cardinality(&commits, &Period::Month, None, None);

        assert_eq!(counts.repo, 2);
        assert_eq!(counts.author, 2);
        assert_eq!(counts.period, 2);
        assert_eq!(counts.language, 2);
    }

    #[test]
    fn local_group_cardinality_includes_primary_and_coauthor_identities() {
        let commits = vec![make_commit(
            "Alice",
            "alice@example.com",
            vec![Author {
                name: "Bob".to_string(),
                email: "bob@example.com".to_string(),
            }],
            Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
            vec![rust_file("src/a.rs", 1, 0)],
        )];

        let counts = local_group_cardinality(&commits, &Period::Month, None, None);

        assert_eq!(counts.author, 2);
    }

    #[test]
    fn local_group_cardinality_applies_author_filter_before_all_dimensions() {
        let commits = vec![
            make_commit_in_repo(
                "included-repo",
                "Alice",
                "alice@example.com",
                vec![Author {
                    name: "Bob".to_string(),
                    email: "bob@example.com".to_string(),
                }],
                Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 1, 0)],
            ),
            make_commit_in_repo(
                "excluded-repo",
                "Carol",
                "carol@example.com",
                vec![Author {
                    name: "Dan".to_string(),
                    email: "dan@example.com".to_string(),
                }],
                Utc.with_ymd_and_hms(2025, 2, 15, 12, 0, 0).unwrap(),
                vec![py_file("src/b.py", 1, 0)],
            ),
        ];

        let counts = local_group_cardinality(&commits, &Period::Month, Some("alice"), None);

        assert_eq!(counts.repo, 1);
        assert_eq!(counts.author, 2);
        assert_eq!(counts.period, 1);
        assert_eq!(counts.language, 1);
    }

    #[test]
    fn local_group_cardinality_applies_language_filter_to_commits_and_languages() {
        let commits = vec![
            make_commit_in_repo(
                "rust-repo",
                "Alice",
                "alice@example.com",
                vec![],
                Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap(),
                vec![rust_file("src/a.rs", 1, 0), py_file("src/a.py", 1, 0)],
            ),
            make_commit_in_repo(
                "python-repo",
                "Bob",
                "bob@example.com",
                vec![],
                Utc.with_ymd_and_hms(2025, 2, 15, 12, 0, 0).unwrap(),
                vec![py_file("src/b.py", 1, 0)],
            ),
        ];

        let counts = local_group_cardinality(&commits, &Period::Month, None, Some("Rust"));

        assert_eq!(counts.repo, 1);
        assert_eq!(counts.author, 1);
        assert_eq!(counts.period, 1);
        assert_eq!(counts.language, 1);
    }
}
