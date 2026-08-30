use std::fmt::Write;
use std::path::PathBuf;

use colored::Colorize;
use comfy_table::{CellAlignment, ContentArrangement, Table};

use crate::cli::NumberFormat;
use crate::output::column::{COL_SEP, ColLayout, format_presentation_label};
use crate::output::presentation::{PresentationMetrics, PresentationModel, PresentationRow};

pub(crate) fn format_num(n: u64, num_fmt: NumberFormat) -> String {
    match num_fmt {
        NumberFormat::Plain => n.to_string(),
        NumberFormat::Short => {
            if n >= 1_000_000 {
                format!("{:.1}M", n as f64 / 1_000_000.0)
            } else if n >= 1_000 {
                format!("{:.1}k", n as f64 / 1_000.0)
            } else {
                n.to_string()
            }
        }
        NumberFormat::Separated => {
            let text = n.to_string();
            let len = text.len();
            if len <= 3 {
                return text;
            }
            let mut result = String::with_capacity(len + (len - 1) / 3);
            for (index, byte) in text.bytes().enumerate() {
                if index > 0 && (len - index).is_multiple_of(3) {
                    result.push(',');
                }
                result.push(byte as char);
            }
            result
        }
    }
}

pub fn render_presentation_table(
    model: &PresentationModel,
    num_fmt: NumberFormat,
    compact: bool,
) -> String {
    if model.rows.is_empty() && model.total.metrics == PresentationMetrics::default() {
        return "No data to display".to_string();
    }

    let metrics: Vec<_> = model
        .rows
        .iter()
        .chain(std::iter::once(&model.total))
        .map(|row| row.metrics)
        .collect();
    let layout = ColLayout::build(&model.columns, compact, &metrics, num_fmt);
    let label_width = model
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| display_label(model, index, row, num_fmt).len())
        .chain(std::iter::once(model.total.label.len()))
        .chain(std::iter::once(model.label_header.len()))
        .max()
        .unwrap_or(model.label_header.len())
        .clamp(20, 60);
    let line_width =
        1 + label_width + layout.widths.iter().sum::<usize>() + layout.cols.len() * COL_SEP;
    let mut output = String::new();
    let _ = writeln!(
        output,
        "{}",
        crate::output::column::header_row(&model.label_header, label_width, &layout)
    );
    let _ = writeln!(output, "{}", "━".repeat(line_width).bold());

    for (index, row) in model.rows.iter().enumerate() {
        let label = display_label(model, index, row, num_fmt);
        let _ = writeln!(
            output,
            "{}",
            crate::output::column::data_row(
                &label,
                label_width,
                &row.metrics,
                &layout,
                num_fmt,
                "",
                false,
            )
        );
    }

    let _ = writeln!(output, "{}", "━".repeat(line_width).bold());
    let total_label = format_presentation_label(&model.total, &model.columns, num_fmt);
    let _ = writeln!(
        output,
        "{}",
        crate::output::column::data_row(
            &total_label,
            label_width,
            &model.total.metrics,
            &layout,
            num_fmt,
            "",
            true,
        )
    );
    output
}

fn display_label(
    model: &PresentationModel,
    index: usize,
    row: &PresentationRow,
    num_fmt: NumberFormat,
) -> String {
    let label = format_presentation_label(row, &model.columns, num_fmt);
    if row.depth == 0 {
        return label;
    }
    if !model.inline_tree {
        return format!("{}{label}", "  ".repeat(row.depth));
    }

    let next_boundary = model.rows[index + 1..]
        .iter()
        .find(|next| next.depth <= row.depth);
    let is_last = next_boundary.is_none_or(|next| next.depth < row.depth);
    let branch = if is_last { "└── " } else { "├── " };
    format!("{}{branch}{label}", "    ".repeat(row.depth - 1))
}

pub fn render_scan_table(repos: &[PathBuf]) -> String {
    if repos.is_empty() {
        return "No data to display".to_string();
    }
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["#", "Repository Path"]);
    for (index, repo) in repos.iter().enumerate() {
        table.add_row(vec![(index + 1).to_string(), repo.display().to_string()]);
    }
    if let Some(column) = table.column_mut(0) {
        column.set_cell_alignment(CellAlignment::Right);
    }
    if let Some(column) = table.column_mut(1) {
        column.set_cell_alignment(CellAlignment::Left);
    }
    table.to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::cli::{Column, DedupMode, EmailDisplay, GroupBy};
    use crate::output::presentation::{
        PresentationData, PresentationMetrics, PresentationOptions, PresentationRowKind,
        build_presentation,
    };
    use crate::stats::models::{AuthorStats, LangStats, PeriodStats};

    fn default_cols() -> Vec<Column> {
        Column::default_set()
    }

    fn make_period(label: &str, langs: Vec<(&str, u64, u64, u64)>, commits: u64) -> PeriodStats {
        let by_language: HashMap<_, _> = langs
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
            total_additions: langs.iter().map(|(_, additions, _, _)| additions).sum(),
            total_deletions: langs.iter().map(|(_, _, deletions, _)| deletions).sum(),
            total_net_modifications: 0,
            total_net_additions: 0,
        }
    }

    type AuthorRow<'a> = (&'a str, u64, u64, u64, Vec<(&'a str, u64, u64, u64)>);

    fn make_period_with_authors(
        label: &str,
        langs: Vec<(&str, u64, u64, u64)>,
        authors: Vec<AuthorRow<'_>>,
    ) -> PeriodStats {
        let mut period = make_period(label, langs, authors.iter().map(|row| row.1).sum());
        for (name, commits, additions, deletions, languages) in authors {
            period.by_author.insert(
                name.to_string(),
                AuthorStats {
                    commits,
                    additions,
                    deletions,
                    languages: languages
                        .into_iter()
                        .map(|(language, additions, deletions, files)| {
                            (
                                language.to_string(),
                                LangStats {
                                    additions,
                                    deletions,
                                    files_changed: files,
                                    ..Default::default()
                                },
                            )
                        })
                        .collect(),
                    ..Default::default()
                },
            );
        }
        period
    }

    fn render_flat(
        stats: &[PeriodStats],
        totals: &PeriodStats,
        group: GroupBy,
        columns: &[Column],
        compact: bool,
        inline_tree: bool,
    ) -> String {
        let model = build_presentation(
            PresentationData::Flat {
                stats,
                totals,
                primary: group,
            },
            PresentationOptions {
                columns,
                sort: None,
                email_display: &EmailDisplay::None,
                dedup: &DedupMode::Name,
                identity_map: &HashMap::new(),
                inline_tree,
            },
        );
        render_presentation_table(&model, NumberFormat::Plain, compact)
    }

    #[test]
    fn test_group_by_language_format() {
        colored::control::set_override(false);
        let period = make_period(
            "2025-01",
            vec![("Rust", 150, 30, 5), ("Python", 40, 10, 2)],
            3,
        );
        let totals = crate::stats::aggregator::aggregate_totals(std::slice::from_ref(&period));
        let output = render_flat(
            &[period],
            &totals,
            GroupBy::Language,
            &default_cols(),
            true,
            false,
        );
        assert!(output.contains("Language"));
        assert!(output.contains("Changes"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Python"));
        assert!(output.find("Rust").unwrap() < output.find("Python").unwrap());
        assert!(output.contains("Total"));
    }

    #[test]
    fn test_group_by_author_format() {
        colored::control::set_override(false);
        let period = make_period_with_authors(
            "2025-01",
            vec![("Rust", 200, 40, 7)],
            vec![
                ("alice", 5, 150, 30, vec![("Rust", 150, 30, 5)]),
                ("bob", 2, 50, 10, vec![("Rust", 50, 10, 2)]),
            ],
        );
        let totals = crate::stats::aggregator::aggregate_totals(std::slice::from_ref(&period));
        let output = render_flat(
            &[period],
            &totals,
            GroupBy::Author,
            &default_cols(),
            true,
            false,
        );
        assert!(output.contains("Author"));
        assert!(output.contains("Language"));
        assert!(output.find("alice").unwrap() < output.find("bob").unwrap());
    }

    #[test]
    fn author_table_none_and_name_dedup_preserve_expected_totals_and_email_labels() {
        let period = make_period_with_authors(
            "2025-01",
            vec![("Rust", 22, 7, 3)],
            vec![
                ("Alex <a@example.com>", 1, 10, 2, vec![("Rust", 10, 2, 1)]),
                ("Alex <b@example.com>", 2, 5, 1, vec![("Rust", 5, 1, 1)]),
                ("Alex <c@example.com>", 3, 7, 4, vec![("Rust", 7, 4, 1)]),
            ],
        );
        let totals = crate::stats::aggregator::aggregate_totals(std::slice::from_ref(&period));
        let output = render_flat(
            &[period],
            &totals,
            GroupBy::Author,
            &default_cols(),
            true,
            false,
        );
        assert_eq!(output.matches("Alex").count(), 1);
        assert!(output.contains('6'));
        assert!(output.contains("22"));
    }

    #[test]
    fn test_group_by_author_language_tree() {
        let period = make_period_with_authors(
            "2025-01",
            vec![("Rust", 200, 40, 7), ("Python", 50, 10, 3)],
            vec![(
                "alice",
                5,
                150,
                30,
                vec![("Rust", 120, 25, 4), ("Python", 30, 5, 1)],
            )],
        );
        let totals = crate::stats::aggregator::aggregate_totals(std::slice::from_ref(&period));
        let output = render_flat(
            &[period],
            &totals,
            GroupBy::Author,
            &default_cols(),
            true,
            true,
        );
        assert!(output.contains("alice"));
        assert!(output.contains("├── Rust") || output.contains("└── Rust"));
        assert!(output.contains("Python"));
    }

    #[test]
    fn test_group_by_period_format() {
        let stats = vec![
            make_period("2025-01", vec![("Rust", 100, 20, 5)], 5),
            make_period("2025-02", vec![("Go", 50, 10, 3)], 3),
        ];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let output = render_flat(
            &stats,
            &totals,
            GroupBy::Period,
            &default_cols(),
            true,
            true,
        );
        assert!(output.contains("2025-01"));
        assert!(output.contains("2025-02"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Go"));
        assert!(!output.contains("5 commits"));
    }

    #[test]
    fn test_non_compact_keeps_adds_and_dels_headers() {
        let stats = vec![make_period("2025-01", vec![("Rust", 100, 20, 5)], 2)];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let columns = [Column::Commits, Column::Adds, Column::Dels, Column::Files];
        let output = render_flat(&stats, &totals, GroupBy::Language, &columns, false, false);
        assert!(output.contains("Additions"));
        assert!(output.contains("Deletions"));
        assert!(!output.contains("Changes"));
    }

    #[test]
    fn test_compact_changes_only_when_adds_dels_adjacent() {
        let stats = vec![make_period("2025-01", vec![("Rust", 100, 20, 5)], 2)];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let columns = [Column::Adds, Column::Files, Column::Dels];
        let output = render_flat(&stats, &totals, GroupBy::Language, &columns, true, false);
        assert!(output.contains("Additions"));
        assert!(output.contains("Deletions"));
        assert!(!output.contains("Changes"));
    }

    #[test]
    fn test_net_column_signed_colored_output_present() {
        let stats = vec![
            make_period("2025-01", vec![("Rust", 100, 20, 5)], 2),
            make_period("2025-02", vec![("Rust", 10, 40, 2)], 1),
        ];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let columns = [Column::Commits, Column::Net, Column::Files];
        let output = render_flat(&stats, &totals, GroupBy::Period, &columns, true, false);
        assert!(output.contains("+80"));
        assert!(output.contains("-30"));
    }

    #[test]
    fn test_period_label_commit_suffix_drops_when_commits_column_selected() {
        let stats = vec![make_period("2025-01", vec![("Rust", 100, 20, 5)], 5)];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let with = [Column::Commits, Column::Files];
        let without = [Column::Files];
        let with_output = render_flat(&stats, &totals, GroupBy::Period, &with, true, false);
        let without_output = render_flat(&stats, &totals, GroupBy::Period, &without, true, false);
        assert!(!with_output.contains("2025-01 (5 commits)"));
        assert!(without_output.contains("2025-01 (5 commits)"));
        assert!(without_output.contains("Total (5 commits)"));
    }

    #[test]
    fn test_group_by_repo_format() {
        let stats = vec![
            make_period("owner/repo-a", vec![("Rust", 100, 20, 5)], 5),
            make_period("owner/repo-b", vec![("Go", 50, 10, 3)], 3),
        ];
        let totals = crate::stats::aggregator::aggregate_totals(&stats);
        let output = render_flat(&stats, &totals, GroupBy::Repo, &default_cols(), true, false);
        assert!(output.contains("owner/repo-a"));
        assert!(output.contains("owner/repo-b"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Go"));
    }

    #[test]
    fn test_empty_returns_no_data() {
        let totals = make_period("Total", Vec::new(), 0);
        for group in [
            GroupBy::Language,
            GroupBy::Author,
            GroupBy::Period,
            GroupBy::Repo,
        ] {
            assert_eq!(
                render_flat(&[], &totals, group, &default_cols(), true, false),
                "No data to display"
            );
        }
    }

    #[test]
    fn empty_rows_with_nonzero_total_renders_total_only() {
        let model = PresentationModel {
            label_header: "Language".to_string(),
            columns: vec![Column::Commits],
            rows: Vec::new(),
            total: PresentationRow {
                depth: 0,
                label: "Total".to_string(),
                kind: PresentationRowKind::Total,
                metrics: PresentationMetrics {
                    commits: 1,
                    ..Default::default()
                },
            },
            inline_tree: false,
        };

        let output = render_presentation_table(&model, NumberFormat::Plain, true);

        assert!(output.contains("Language"));
        assert!(output.contains("Commits"));
        assert!(output.contains("Total"));
        assert_eq!(output.matches("Total").count(), 1);
        assert!(output.contains('1'));
        assert!(!output.contains("No data to display"));
    }

    #[test]
    fn test_short_numbers() {
        assert_eq!(format_num(999, NumberFormat::Short), "999");
        assert_eq!(format_num(1_500, NumberFormat::Short), "1.5k");
        assert_eq!(format_num(2_500_000, NumberFormat::Short), "2.5M");
        assert_eq!(format_num(42, NumberFormat::Plain), "42");
    }

    #[test]
    fn test_separated_numbers() {
        assert_eq!(format_num(0, NumberFormat::Separated), "0");
        assert_eq!(format_num(1_000, NumberFormat::Separated), "1,000");
        assert_eq!(format_num(1_234_567, NumberFormat::Separated), "1,234,567");
    }

    #[test]
    fn scan_table_contains_paths() {
        let output = render_scan_table(&[
            PathBuf::from("/home/user/repo-a"),
            PathBuf::from("/home/user/repo-b"),
        ]);
        assert!(output.contains("repo-a"));
        assert!(output.contains("repo-b"));
        assert!(output.contains("Repository Path"));
    }

    #[test]
    fn scan_table_empty_returns_no_data() {
        assert_eq!(render_scan_table(&[]), "No data to display");
    }
}
