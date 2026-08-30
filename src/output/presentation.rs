use std::borrow::Cow;
use std::collections::HashMap;

use crate::cli::{Column, DedupMode, EmailDisplay, GroupBy, SortBy};
use crate::stats::models::{AuthorStats, GroupNode, LangStats, PeriodStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationRowKind {
    Group,
    Language,
    Total,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentationMetrics {
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
    pub files: u64,
}

impl PresentationMetrics {
    pub fn net(&self) -> i64 {
        self.additions as i64 - self.deletions as i64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationRow {
    pub depth: usize,
    pub label: String,
    pub kind: PresentationRowKind,
    pub metrics: PresentationMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationModel {
    pub label_header: String,
    pub columns: Vec<Column>,
    pub rows: Vec<PresentationRow>,
    pub total: PresentationRow,
    pub inline_tree: bool,
}

pub enum PresentationData<'a> {
    Flat {
        stats: &'a [PeriodStats],
        totals: &'a PeriodStats,
        primary: GroupBy,
    },
    Tree {
        nodes: &'a [GroupNode],
        levels: &'a [GroupBy],
        totals: &'a PeriodStats,
    },
}

pub struct PresentationOptions<'a> {
    pub columns: &'a [Column],
    pub sort: Option<&'a SortBy>,
    pub email_display: &'a EmailDisplay,
    pub dedup: &'a DedupMode,
    pub identity_map: &'a HashMap<String, String>,
    pub inline_tree: bool,
}

struct FlatGroup {
    row: PresentationRow,
    languages: Vec<PresentationRow>,
}

struct AuthorAggregate {
    raw_labels: Vec<String>,
    stats: AuthorStats,
}

pub fn build_presentation(
    data: PresentationData<'_>,
    options: PresentationOptions<'_>,
) -> PresentationModel {
    let columns = options.columns.to_vec();
    let inline_tree = options.inline_tree;
    match data {
        PresentationData::Flat {
            stats,
            totals,
            primary,
        } => {
            let rows = build_flat_rows(stats, totals, primary, &options);
            PresentationModel {
                label_header: flat_label_header(primary).to_string(),
                columns,
                rows,
                total: total_row(metrics_for_period(totals)),
                inline_tree,
            }
        }
        PresentationData::Tree {
            nodes,
            levels,
            totals,
        } => {
            let group_levels = validate_tree_levels(nodes, levels);
            let mut rows = Vec::new();
            let root_nodes: Vec<&GroupNode> = nodes.iter().collect();
            flatten_tree_nodes(&root_nodes, group_levels, 0, &options, &mut rows);
            PresentationModel {
                label_header: tree_label_header(levels),
                columns,
                rows,
                total: total_row(metrics_for_period(totals)),
                inline_tree,
            }
        }
    }
}

fn build_flat_rows(
    stats: &[PeriodStats],
    totals: &PeriodStats,
    primary: GroupBy,
    options: &PresentationOptions<'_>,
) -> Vec<PresentationRow> {
    match primary {
        GroupBy::Language => language_rows(&totals.by_language, 0, options.sort),
        GroupBy::Author => author_rows(totals, options),
        GroupBy::Period | GroupBy::Repo => period_rows(stats, primary, options),
    }
}

fn period_rows(
    stats: &[PeriodStats],
    primary: GroupBy,
    options: &PresentationOptions<'_>,
) -> Vec<PresentationRow> {
    let mut groups: Vec<FlatGroup> = stats
        .iter()
        .map(|period| FlatGroup {
            row: PresentationRow {
                depth: 0,
                label: period.period_label.clone(),
                kind: PresentationRowKind::Group,
                metrics: metrics_for_period(period),
            },
            languages: language_rows(&period.by_language, 1, options.sort),
        })
        .collect();
    sort_groups(
        &mut groups,
        options.sort,
        if primary == GroupBy::Period {
            SortBy::Name
        } else {
            SortBy::Additions
        },
    );
    groups
        .into_iter()
        .flat_map(|group| std::iter::once(group.row).chain(group.languages))
        .collect()
}

fn author_rows(totals: &PeriodStats, options: &PresentationOptions<'_>) -> Vec<PresentationRow> {
    let mut authors: HashMap<String, AuthorAggregate> = HashMap::new();
    for (label, stats) in &totals.by_author {
        let key = author_merge_key(label, options.dedup, options.identity_map);
        let aggregate = authors.entry(key).or_insert_with(|| AuthorAggregate {
            raw_labels: Vec::new(),
            stats: AuthorStats::default(),
        });
        aggregate.raw_labels.push(label.clone());
        merge_author_stats(&mut aggregate.stats, stats);
    }

    let mut groups: Vec<FlatGroup> = authors
        .into_values()
        .map(|mut author| {
            author.raw_labels.sort();
            author.raw_labels.dedup();
            let languages = language_rows(&author.stats.languages, 1, options.sort);
            FlatGroup {
                row: PresentationRow {
                    depth: 0,
                    label: format_author_labels(&author.raw_labels, options.email_display),
                    kind: PresentationRowKind::Group,
                    metrics: metrics_for_author(&author.stats),
                },
                languages,
            }
        })
        .collect();
    sort_groups(&mut groups, options.sort, SortBy::Commits);
    groups
        .into_iter()
        .flat_map(|group| std::iter::once(group.row).chain(group.languages))
        .collect()
}

fn flatten_tree_nodes(
    nodes: &[&GroupNode],
    levels: &[GroupBy],
    depth: usize,
    options: &PresentationOptions<'_>,
    rows: &mut Vec<PresentationRow>,
) {
    let dimension = *levels
        .get(depth)
        .unwrap_or_else(|| panic!("group tree depth {depth} has no matching dimension"));
    let mut buckets: HashMap<String, Vec<&GroupNode>> = HashMap::new();
    for node in nodes {
        let key = if dimension == GroupBy::Author {
            author_merge_key(&node.label, options.dedup, options.identity_map)
        } else {
            node.label.clone()
        };
        buckets.entry(key).or_default().push(*node);
    }

    let mut groups: Vec<(PresentationRow, Vec<&GroupNode>)> = buckets
        .into_values()
        .map(|bucket| {
            let stats = crate::stats::aggregator::aggregate_totals(
                &bucket
                    .iter()
                    .map(|node| node.stats.clone())
                    .collect::<Vec<_>>(),
            );
            let label = if dimension == GroupBy::Author {
                let mut labels: Vec<String> =
                    bucket.iter().map(|node| node.label.clone()).collect();
                labels.sort();
                labels.dedup();
                format_author_labels(&labels, options.email_display)
            } else {
                bucket[0].label.clone()
            };
            (
                PresentationRow {
                    depth,
                    label,
                    kind: PresentationRowKind::Group,
                    metrics: metrics_for_period(&stats),
                },
                bucket,
            )
        })
        .collect();
    sort_tree_groups(
        &mut groups,
        options.sort,
        if dimension == GroupBy::Author {
            SortBy::Commits
        } else {
            SortBy::Name
        },
    );

    for (row, bucket) in groups {
        rows.push(row);
        let is_leaf = bucket[0].children.is_empty();
        assert!(
            bucket
                .iter()
                .all(|node| node.children.is_empty() == is_leaf),
            "merged group nodes at depth {depth} disagree on whether they are leaves"
        );
        if is_leaf {
            assert_eq!(
                depth + 1,
                levels.len(),
                "group tree leaf depth {depth} does not match {} dimensions",
                levels.len()
            );
            let stats = crate::stats::aggregator::aggregate_totals(
                &bucket
                    .iter()
                    .map(|node| node.stats.clone())
                    .collect::<Vec<_>>(),
            );
            rows.extend(language_rows(&stats.by_language, depth + 1, options.sort));
        } else {
            assert!(
                depth + 1 < levels.len(),
                "group tree has children below the final dimension at depth {depth}"
            );
            let children: Vec<&GroupNode> = bucket
                .iter()
                .flat_map(|node| node.children.iter())
                .collect();
            flatten_tree_nodes(&children, levels, depth + 1, options, rows);
        }
    }
}

fn validate_tree_levels<'a>(nodes: &[GroupNode], levels: &'a [GroupBy]) -> &'a [GroupBy] {
    assert!(
        !levels.is_empty(),
        "tree presentation requires at least one dimension"
    );
    assert!(
        !levels[..levels.len().saturating_sub(1)].contains(&GroupBy::Language),
        "language can only be the final tree dimension"
    );
    let group_levels = if levels.last() == Some(&GroupBy::Language) {
        &levels[..levels.len() - 1]
    } else {
        levels
    };
    assert!(
        nodes.is_empty() || !group_levels.is_empty(),
        "non-empty group trees require a non-language dimension"
    );
    group_levels
}

fn language_rows(
    languages: &HashMap<String, LangStats>,
    depth: usize,
    sort: Option<&SortBy>,
) -> Vec<PresentationRow> {
    let mut rows: Vec<PresentationRow> = languages
        .iter()
        .map(|(label, stats)| PresentationRow {
            depth,
            label: label.clone(),
            kind: PresentationRowKind::Language,
            metrics: metrics_for_language(stats),
        })
        .collect();
    let sort = match sort.copied().unwrap_or(SortBy::Additions) {
        SortBy::Commits => SortBy::Additions,
        other => other,
    };
    sort_rows(&mut rows, sort);
    rows
}

fn sort_groups(groups: &mut [FlatGroup], sort: Option<&SortBy>, default: SortBy) {
    let sort = sort.copied().unwrap_or(default);
    groups.sort_by(|left, right| compare_rows(&left.row, &right.row, sort));
}

fn sort_tree_groups(
    groups: &mut [(PresentationRow, Vec<&GroupNode>)],
    sort: Option<&SortBy>,
    default: SortBy,
) {
    let sort = sort.copied().unwrap_or(default);
    groups.sort_by(|left, right| compare_rows(&left.0, &right.0, sort));
}

fn sort_rows(rows: &mut [PresentationRow], sort: SortBy) {
    rows.sort_by(|left, right| compare_rows(left, right, sort));
}

fn compare_rows(
    left: &PresentationRow,
    right: &PresentationRow,
    sort: SortBy,
) -> std::cmp::Ordering {
    let metric_order = match sort {
        SortBy::Commits => right.metrics.commits.cmp(&left.metrics.commits),
        SortBy::Additions => right.metrics.additions.cmp(&left.metrics.additions),
        SortBy::Deletions => right.metrics.deletions.cmp(&left.metrics.deletions),
        SortBy::Files => right.metrics.files.cmp(&left.metrics.files),
        SortBy::Name => std::cmp::Ordering::Equal,
    };
    metric_order.then_with(|| left.label.cmp(&right.label))
}

fn metrics_for_period(period: &PeriodStats) -> PresentationMetrics {
    PresentationMetrics {
        commits: period.total_commits,
        additions: period.total_additions,
        deletions: period.total_deletions,
        files: period
            .by_language
            .values()
            .map(|language| language.files_changed)
            .sum(),
    }
}

fn metrics_for_author(author: &AuthorStats) -> PresentationMetrics {
    PresentationMetrics {
        commits: author.commits + author.co_authored_commits,
        additions: author.additions + author.co_authored_additions,
        deletions: author.deletions + author.co_authored_deletions,
        files: author
            .languages
            .values()
            .map(|language| language.files_changed)
            .sum(),
    }
}

fn metrics_for_language(language: &LangStats) -> PresentationMetrics {
    PresentationMetrics {
        additions: language.additions,
        deletions: language.deletions,
        files: language.files_changed,
        ..Default::default()
    }
}

fn total_row(metrics: PresentationMetrics) -> PresentationRow {
    PresentationRow {
        depth: 0,
        label: "Total".to_string(),
        kind: PresentationRowKind::Total,
        metrics,
    }
}

fn merge_author_stats(target: &mut AuthorStats, source: &AuthorStats) {
    target.commits += source.commits;
    target.co_authored_commits += source.co_authored_commits;
    target.additions += source.additions;
    target.co_authored_additions += source.co_authored_additions;
    target.deletions += source.deletions;
    target.co_authored_deletions += source.co_authored_deletions;
    target.net_modifications += source.net_modifications;
    target.co_authored_net_modifications += source.co_authored_net_modifications;
    target.net_additions += source.net_additions;
    target.co_authored_net_additions += source.co_authored_net_additions;
    for (language, stats) in &source.languages {
        let entry = target.languages.entry(language.clone()).or_default();
        entry.additions += stats.additions;
        entry.deletions += stats.deletions;
        entry.files_changed += stats.files_changed;
        entry.net_modifications += stats.net_modifications;
        entry.net_additions += stats.net_additions;
    }
    for (language, stats) in &source.co_authored_languages {
        let entry = target
            .co_authored_languages
            .entry(language.clone())
            .or_default();
        entry.additions += stats.additions;
        entry.deletions += stats.deletions;
        entry.files_changed += stats.files_changed;
        entry.net_modifications += stats.net_modifications;
        entry.net_additions += stats.net_additions;
    }
}

fn extract_name_email(author: &str) -> (&str, Option<&str>) {
    if let Some((name, rest)) = author.rsplit_once(" <")
        && let Some(email) = rest.strip_suffix('>')
    {
        return (name, Some(email));
    }
    (author, None)
}

fn author_merge_key(
    label: &str,
    dedup: &DedupMode,
    identity_map: &HashMap<String, String>,
) -> String {
    let (name, email) = extract_name_email(label);
    match dedup {
        DedupMode::None => label.to_string(),
        DedupMode::Name => name.to_string(),
        #[cfg(feature = "github")]
        DedupMode::Remote => email
            .and_then(|email| identity_map.get(&email.to_ascii_lowercase()))
            .cloned()
            .unwrap_or_else(|| label.to_string()),
    }
}

fn format_author_labels(labels: &[String], display: &EmailDisplay) -> String {
    let first = labels.first().expect("author aggregates are non-empty");
    let (name, _) = extract_name_email(first);
    match display {
        EmailDisplay::None => name.to_string(),
        EmailDisplay::Full => join_author_labels(name, labels.iter().map(String::as_str)),
        EmailDisplay::Simple => join_author_labels(
            name,
            labels
                .iter()
                .map(|label| simplify_author_email(label).into_owned()),
        ),
    }
}

fn join_author_labels(name: &str, labels: impl Iterator<Item = impl AsRef<str>>) -> String {
    let mut labels = labels.map(|label| label.as_ref().to_string());
    let first = labels.next().unwrap_or_else(|| name.to_string());
    let mut result = first;
    for label in labels {
        let suffix = label.strip_prefix(name).unwrap_or(&label).trim_start();
        result.push_str(", ");
        result.push_str(suffix);
    }
    result
}

fn simplify_author_email(author: &str) -> Cow<'_, str> {
    let Some(inner) = author.strip_suffix('>') else {
        return Cow::Borrowed(author);
    };
    let Some((name, email)) = inner.rsplit_once(" <") else {
        return Cow::Borrowed(author);
    };
    if !email.contains("noreply.github.com") && !email.contains("noreply.gitlab.com") {
        return Cow::Borrowed(author);
    }
    let tag = if email.contains("noreply.github.com") {
        "[github email]"
    } else {
        "[gitlab email]"
    };
    let short_email = email
        .split_once('+')
        .map_or_else(|| email.to_string(), |(_, user)| format!("...+{user}"))
        .replace("users.noreply.github.com", tag)
        .replace("noreply.gitlab.com", tag);
    Cow::Owned(format!("{name} <{short_email}>"))
}

fn flat_label_header(primary: GroupBy) -> &'static str {
    match primary {
        GroupBy::Language => "Language",
        GroupBy::Author => "Author / Language",
        GroupBy::Period => "Period / Language",
        GroupBy::Repo => "Repo / Language",
    }
}

fn tree_label_header(levels: &[GroupBy]) -> String {
    let mut labels: Vec<&str> = levels
        .iter()
        .map(|level| match level {
            GroupBy::Language => "Language",
            GroupBy::Author => "Author",
            GroupBy::Period => "Period",
            GroupBy::Repo => "Repo",
        })
        .collect();
    if levels.last() != Some(&GroupBy::Language) {
        labels.push("Language");
    }
    labels.join(" / ")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::cli::{Column, DedupMode, EmailDisplay, GroupBy, SortBy};
    use crate::stats::models::{AuthorStats, GroupNode, LangStats, PeriodStats};

    use super::{
        PresentationData, PresentationMetrics, PresentationOptions, PresentationRowKind,
        build_presentation, merge_author_stats,
    };

    fn period(label: &str, commits: u64, languages: &[(&str, u64, u64, u64)]) -> PeriodStats {
        let by_language = languages
            .iter()
            .map(|(language, additions, deletions, files)| {
                (
                    (*language).to_string(),
                    LangStats {
                        additions: *additions,
                        deletions: *deletions,
                        files_changed: *files,
                        ..Default::default()
                    },
                )
            })
            .collect();
        PeriodStats {
            period_label: label.to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: commits,
            total_additions: languages.iter().map(|(_, additions, _, _)| additions).sum(),
            total_deletions: languages.iter().map(|(_, _, deletions, _)| deletions).sum(),
            total_net_modifications: 0,
            total_net_additions: 0,
        }
    }

    #[test]
    fn presentation_model_orders_columns_rows_languages_and_total_once() {
        let stats = vec![
            period("repo-b", 1, &[("Go", 2, 1, 1)]),
            period("repo-a", 3, &[("Rust", 8, 2, 2), ("Python", 4, 1, 1)]),
        ];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let columns = vec![Column::Files, Column::Commits, Column::Net];
        let model = build_presentation(
            PresentationData::Flat {
                stats: &stats,
                totals: &totals,
                primary: GroupBy::Repo,
            },
            PresentationOptions {
                columns: &columns,
                sort: Some(&SortBy::Commits),
                email_display: &EmailDisplay::None,
                dedup: &DedupMode::Name,
                identity_map: &HashMap::new(),
                inline_tree: false,
            },
        );

        assert_eq!(model.columns, columns);
        assert_eq!(
            model
                .rows
                .iter()
                .map(|row| (row.depth, row.label.as_str(), row.kind))
                .collect::<Vec<_>>(),
            vec![
                (0, "repo-a", PresentationRowKind::Group),
                (1, "Rust", PresentationRowKind::Language),
                (1, "Python", PresentationRowKind::Language),
                (0, "repo-b", PresentationRowKind::Group),
                (1, "Go", PresentationRowKind::Language),
            ]
        );
        assert_eq!(model.total.kind, PresentationRowKind::Total);
        assert_eq!(model.total.metrics.commits, 4);
        assert_eq!(
            model
                .rows
                .iter()
                .filter(|row| row.kind == PresentationRowKind::Total)
                .count(),
            0
        );
        assert_eq!(
            model.rows[0].metrics,
            PresentationMetrics {
                commits: 3,
                additions: 12,
                deletions: 3,
                files: 3,
            }
        );
    }

    #[test]
    fn tree_author_dimension_controls_dedup_and_email_without_rewriting_repo_labels() {
        let first = GroupNode {
            label: "Alex <a@example.com>".to_string(),
            stats: period("first", 1, &[("Rust", 4, 1, 1)]),
            children: Vec::new(),
        };
        let second = GroupNode {
            label: "Alex <b@example.com>".to_string(),
            stats: period("second", 2, &[("Rust", 6, 2, 1)]),
            children: Vec::new(),
        };
        let root_stats = crate::stats::aggregator::aggregate_totals(&[
            first.stats.clone(),
            second.stats.clone(),
        ]);
        let totals = root_stats.clone();
        let nodes = vec![GroupNode {
            label: "Alex <repo@example.com>".to_string(),
            stats: root_stats,
            children: vec![first, second],
        }];
        let levels = [GroupBy::Repo, GroupBy::Author];
        let columns = Column::default_set();
        let model = build_presentation(
            PresentationData::Tree {
                nodes: &nodes,
                levels: &levels,
                totals: &totals,
            },
            PresentationOptions {
                columns: &columns,
                sort: None,
                email_display: &EmailDisplay::None,
                dedup: &DedupMode::Name,
                identity_map: &HashMap::new(),
                inline_tree: true,
            },
        );

        assert_eq!(model.rows[0].label, "Alex <repo@example.com>");
        assert_eq!(model.rows[0].depth, 0);
        assert_eq!(model.rows[1].label, "Alex");
        assert_eq!(model.rows[1].depth, 1);
        assert_eq!(model.rows[1].metrics.commits, 3);
        assert_eq!(
            model
                .rows
                .iter()
                .filter(|row| row.depth == 1 && row.kind == PresentationRowKind::Group)
                .count(),
            1
        );
    }

    #[test]
    fn group_plan_root_author_presentation_does_not_sum_overlapping_nodes() {
        let nodes = vec![
            GroupNode {
                label: "Alice <alice@example.com>".to_string(),
                stats: period("Alice", 1, &[("Rust", 7, 2, 1)]),
                children: Vec::new(),
            },
            GroupNode {
                label: "Bob <bob@example.com>".to_string(),
                stats: period("Bob", 1, &[("Rust", 7, 2, 1)]),
                children: Vec::new(),
            },
        ];
        let levels = [GroupBy::Author, GroupBy::Language];
        let columns = Column::default_set();
        let totals = period("Total", 1, &[("Rust", 7, 2, 1)]);

        let model = build_presentation(
            PresentationData::Tree {
                nodes: &nodes,
                levels: &levels,
                totals: &totals,
            },
            PresentationOptions {
                columns: &columns,
                sort: None,
                email_display: &EmailDisplay::Full,
                dedup: &DedupMode::None,
                identity_map: &HashMap::new(),
                inline_tree: true,
            },
        );

        assert_eq!(
            model
                .rows
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Alice <alice@example.com>",
                "Rust",
                "Bob <bob@example.com>",
                "Rust"
            ]
        );
        assert_eq!(model.total.metrics.commits, 1);
    }

    #[test]
    fn presentation_author_merge_preserves_all_primary_and_coauthored_metrics() {
        let mut target = AuthorStats {
            commits: 1,
            co_authored_commits: 2,
            additions: 3,
            co_authored_additions: 4,
            deletions: 5,
            co_authored_deletions: 6,
            languages: HashMap::from([(
                "Rust".to_string(),
                LangStats {
                    additions: 7,
                    deletions: 8,
                    files_changed: 9,
                    net_modifications: 10,
                    net_additions: 11,
                },
            )]),
            co_authored_languages: HashMap::from([(
                "Rust".to_string(),
                LangStats {
                    additions: 12,
                    deletions: 13,
                    files_changed: 14,
                    net_modifications: 15,
                    net_additions: 16,
                },
            )]),
            net_modifications: 17,
            co_authored_net_modifications: 18,
            net_additions: 19,
            co_authored_net_additions: 20,
        };
        let source = AuthorStats {
            commits: 101,
            co_authored_commits: 102,
            additions: 103,
            co_authored_additions: 104,
            deletions: 105,
            co_authored_deletions: 106,
            languages: HashMap::from([(
                "Rust".to_string(),
                LangStats {
                    additions: 107,
                    deletions: 108,
                    files_changed: 109,
                    net_modifications: 110,
                    net_additions: 111,
                },
            )]),
            co_authored_languages: HashMap::from([(
                "Rust".to_string(),
                LangStats {
                    additions: 112,
                    deletions: 113,
                    files_changed: 114,
                    net_modifications: 115,
                    net_additions: 116,
                },
            )]),
            net_modifications: 117,
            co_authored_net_modifications: 118,
            net_additions: 119,
            co_authored_net_additions: 120,
        };

        merge_author_stats(&mut target, &source);

        assert_eq!(target.commits, 102);
        assert_eq!(target.co_authored_commits, 104);
        assert_eq!(target.additions, 106);
        assert_eq!(target.co_authored_additions, 108);
        assert_eq!(target.deletions, 110);
        assert_eq!(target.co_authored_deletions, 112);
        assert_eq!(target.net_modifications, 134);
        assert_eq!(target.co_authored_net_modifications, 136);
        assert_eq!(target.net_additions, 138);
        assert_eq!(target.co_authored_net_additions, 140);
        let rust = &target.languages["Rust"];
        assert_eq!(rust.additions, 114);
        assert_eq!(rust.deletions, 116);
        assert_eq!(rust.files_changed, 118);
        assert_eq!(rust.net_modifications, 120);
        assert_eq!(rust.net_additions, 122);
        let co_rust = &target.co_authored_languages["Rust"];
        assert_eq!(co_rust.additions, 124);
        assert_eq!(co_rust.deletions, 126);
        assert_eq!(co_rust.files_changed, 128);
        assert_eq!(co_rust.net_modifications, 130);
        assert_eq!(co_rust.net_additions, 132);
    }

    #[cfg(feature = "github")]
    #[test]
    fn local_and_github_group_plans_build_equal_presentation_models() {
        use chrono::{TimeZone, Utc};

        use crate::cli::Period;
        use crate::github::api::{CommitData, RepoContribution};
        use crate::stats::models::{Author, CommitStats, FileChange};

        fn local_commit(oid: &str, month: u32, additions: u64, deletions: u64) -> CommitStats {
            let author = Author {
                name: "Alice".to_string(),
                email: "alice@example.com".to_string(),
            };
            CommitStats {
                repo_id: "repo-a-id".to_string(),
                repo: "repo-a".to_string(),
                oid: oid.to_string(),
                author: author.clone(),
                committer: author,
                co_authors: Vec::new(),
                timestamp: Utc.with_ymd_and_hms(2025, month, 2, 10, 0, 0).unwrap(),
                message_subject: "fixture".to_string(),
                file_changes: vec![FileChange {
                    path: format!("src/{oid}.rs"),
                    language: Some("Rust".to_string()),
                    additions,
                    deletions,
                    net_modifications: additions.max(deletions),
                    net_additions: additions.saturating_sub(deletions),
                }],
            }
        }

        let commits = vec![local_commit("one", 1, 10, 2), local_commit("two", 2, 5, 1)];
        let contributions = vec![RepoContribution {
            repo_name: "repo-a".to_string(),
            total_commits: 2,
            total_additions: 15,
            total_deletions: 3,
            commits: vec![
                CommitData {
                    oid: Some("one".to_string()),
                    additions: 10,
                    deletions: 2,
                    committed_date: "2025-01-02T10:00:00Z".to_string(),
                },
                CommitData {
                    oid: Some("two".to_string()),
                    additions: 5,
                    deletions: 1,
                    committed_date: "2025-02-02T10:00:00Z".to_string(),
                },
            ],
            weeks: Vec::new(),
            languages: HashMap::from([("Rust".to_string(), 1)]),
        }];
        let levels = [GroupBy::Repo, GroupBy::Period, GroupBy::Language];
        let local_nodes = crate::stats::aggregator::build_group_tree(
            &commits,
            &levels,
            &Period::Month,
            None,
            None,
        );
        let github_nodes = crate::github::api::contributions_to_group_tree(
            &contributions,
            &levels,
            &Period::Month,
        );
        let local_totals = crate::stats::aggregator::aggregate_totals(
            &crate::stats::aggregator::aggregate_commits(&commits, &Period::Month, None, None),
        );
        let github_totals = crate::stats::aggregator::aggregate_totals(
            &crate::github::api::contributions_to_period_stats(&contributions, &Period::Month),
        );
        let columns = vec![Column::Files, Column::Commits, Column::Net];
        let build = |nodes: &[GroupNode], totals: &PeriodStats| {
            build_presentation(
                PresentationData::Tree {
                    nodes,
                    levels: &levels,
                    totals,
                },
                PresentationOptions {
                    columns: &columns,
                    sort: Some(&SortBy::Name),
                    email_display: &EmailDisplay::None,
                    dedup: &DedupMode::Name,
                    identity_map: &HashMap::new(),
                    inline_tree: true,
                },
            )
        };

        assert_eq!(
            build(&local_nodes, &local_totals),
            build(&github_nodes, &github_totals)
        );
    }
}
