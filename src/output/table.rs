use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;

use colored::Colorize;
use comfy_table::{CellAlignment, ContentArrangement, Table};

use crate::cli::{self, DedupMode, EmailDisplay, GroupBy, NumberFormat, SortBy};
use crate::output::column::{ColLayout, DisplayCol, RowMetric, COL_SEP};
use crate::stats::models::{GroupNode, PeriodStats};

const LINE_WIDTH: usize = 81;
const NAME_WIDTH: usize = 20;

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
            let s = n.to_string();
            let bytes = s.as_bytes();
            let len = bytes.len();
            if len <= 3 {
                return s;
            }
            let mut result = String::with_capacity(len + (len - 1) / 3);
            for (i, &b) in bytes.iter().enumerate() {
                if i > 0 && (len - i).is_multiple_of(3) {
                    result.push(',');
                }
                result.push(b as char);
            }
            result
        }
    }
}

fn simplify_noreply(name_part: &str, email: &str) -> String {
    let tag = if email.contains("noreply.github.com") {
        "[github email]"
    } else {
        "[gitlab email]"
    };
    let short_email = if let Some((_, user_part)) = email.split_once('+') {
        format!("...+{user_part}")
    } else {
        email.to_string()
    };
    let short_email = short_email.replace("users.noreply.github.com", tag)
        .replace("noreply.gitlab.com", tag);
    format!("{name_part} <{short_email}>")
}

fn format_author_name<'a>(author: &'a str, mode: &EmailDisplay) -> Cow<'a, str> {
    match mode {
        EmailDisplay::Full => Cow::Borrowed(author),
        EmailDisplay::None => {
            if let Some((name_part, _)) = author.rsplit_once(" <") {
                Cow::Borrowed(name_part)
            } else {
                Cow::Borrowed(author)
            }
        }
        EmailDisplay::Simple => {
            if let Some(inner) = author.strip_suffix('>')
                && let Some((name_part, email)) = inner.rsplit_once(" <")
                && (email.contains("noreply.github.com") || email.contains("noreply.gitlab.com"))
            {
                return Cow::Owned(simplify_noreply(name_part, email));
            }
            Cow::Borrowed(author)
        }
    }
}

fn heavy(w: usize) -> String { "━".repeat(w) }

fn table_line_width(label_w: usize, layout: &ColLayout) -> usize {
    1 + label_w + layout.widths.iter().sum::<usize>() + layout.cols.len() * COL_SEP
}

fn files_for_period(period: &PeriodStats) -> u64 {
    period.by_language.values().map(|ls| ls.files_changed).sum()
}

fn metric_for_period(period: &PeriodStats) -> RowMetric {
    RowMetric {
        commits: period.total_commits,
        adds: period.total_additions,
        dels: period.total_deletions,
        files: files_for_period(period),
    }
}

fn has_commits_col(layout: &ColLayout) -> bool {
    layout.cols.iter().any(|dc| matches!(dc, DisplayCol::Commits))
}

fn max_inline_widths(items: &[(&str, u64, u64)], num_fmt: NumberFormat) -> (usize, usize, usize, usize) {
    let lang_w = items.iter().map(|(l, _, _)| l.len()).max().unwrap_or(1);
    let net_w = items.iter().map(|(_, a, d)| {
        let net = *a as i64 - *d as i64;
        format_num(net.unsigned_abs(), num_fmt).len() + 1
    }).max().unwrap_or(2);
    let add_w = items.iter().map(|(_, a, _)| {
        format!("+{}", format_num(*a, num_fmt)).len()
    }).max().unwrap_or(2);
    let del_w = items.iter().map(|(_, _, d)| {
        format!("-{}", format_num(*d, num_fmt)).len()
    }).max().unwrap_or(2);
    (lang_w, net_w, add_w, del_w)
}

#[allow(clippy::too_many_arguments)]
fn format_inline_entry(
    prefix: &str,
    lang: &str,
    adds: u64,
    dels: u64,
    num_fmt: NumberFormat,
    lang_w: usize,
    net_w: usize,
    add_w: usize,
    del_w: usize,
) -> String {
    let net = adds as i64 - dels as i64;
    let net_s = format_num(net.unsigned_abs(), num_fmt);
    let net_display = if net >= 0 { format!("+{net_s}") } else { format!("-{net_s}") };
    let net_aligned = format!("{:>w$}", net_display, w = net_w);
    let net_colored = if net >= 0 {
        net_aligned.green().to_string()
    } else {
        net_aligned.red().to_string()
    };
    let add_s = format!("+{}", format_num(adds, num_fmt));
    let del_s = format!("-{}", format_num(dels, num_fmt));
    let add_aligned = format!("{:>w$}", add_s, w = add_w);
    let del_aligned = format!("{:>w$}", del_s, w = del_w);
    let lang_padded = format!("{:<w$}", lang, w = lang_w);
    format!(
        "{}{} {} ({} {})",
        prefix.dimmed(),
        lang_padded.cyan(),
        net_colored,
        add_aligned.dimmed(),
        del_aligned.dimmed(),
    )
}

fn aggregate_languages(
    stats: &[PeriodStats],
    sort: Option<&SortBy>,
) -> Vec<(String, u64, u64, u64)> {
    let mut map: HashMap<String, (u64, u64, u64)> = HashMap::new();
    for period in stats {
        for (lang, ls) in &period.by_language {
            let entry = map.entry(lang.clone()).or_default();
            entry.0 += ls.additions;
            entry.1 += ls.deletions;
            entry.2 += ls.files_changed;
        }
    }
    let mut rows: Vec<_> = map
        .into_iter()
        .map(|(lang, (a, d, f))| (lang, a, d, f))
        .collect();
    match sort.unwrap_or(&SortBy::Additions) {
        SortBy::Additions | SortBy::Commits => rows.sort_by_key(|b| std::cmp::Reverse(b.1)),
        SortBy::Deletions => rows.sort_by_key(|b| std::cmp::Reverse(b.2)),
        SortBy::Files => rows.sort_by_key(|b| std::cmp::Reverse(b.3)),
        SortBy::Name => rows.sort_by(|a, b| a.0.cmp(&b.0)),
    }
    rows
}

fn extract_name_email(author_key: &str) -> (&str, Option<&str>) {
    if let Some((name, rest)) = author_key.rsplit_once(" <")
        && let Some(email) = rest.strip_suffix('>')
    {
        return (name, Some(email));
    }
    (author_key, None)
}

struct AuthorRow {
    name: String,
    emails: Vec<String>,
    commits: u64,
    co_authored_commits: u64,
    additions: u64,
    co_authored_additions: u64,
    deletions: u64,
    co_authored_deletions: u64,
    top_lang: String,
    /// Per-language breakdown: (lang, additions, deletions, files)
    languages: Vec<(String, u64, u64, u64)>,
}

impl AuthorRow {
    fn total_commits(&self) -> u64 { self.commits + self.co_authored_commits }
    fn total_additions(&self) -> u64 { self.additions + self.co_authored_additions }
    fn total_deletions(&self) -> u64 { self.deletions + self.co_authored_deletions }
}

fn aggregate_authors(
    stats: &[PeriodStats],
    dedup: &DedupMode,
    _identity_map: &HashMap<String, String>,
    sort: Option<&SortBy>,
) -> Vec<AuthorRow> {
    let mut commits: HashMap<String, u64> = HashMap::new();
    let mut co_commits: HashMap<String, u64> = HashMap::new();
    let mut additions: HashMap<String, u64> = HashMap::new();
    let mut co_additions: HashMap<String, u64> = HashMap::new();
    let mut deletions: HashMap<String, u64> = HashMap::new();
    let mut co_deletions: HashMap<String, u64> = HashMap::new();
    let mut lang_data: HashMap<String, HashMap<String, (u64, u64, u64)>> = HashMap::new();
    let mut emails: HashMap<String, Vec<String>> = HashMap::new();
    let mut display_names: HashMap<String, String> = HashMap::new();

    for period in stats {
        for (author_key, as_) in &period.by_author {
            let (name, email) = extract_name_email(author_key);

            let merge_key = match dedup {
                DedupMode::None => author_key.to_string(),
                DedupMode::Name => name.to_string(),
                #[cfg(feature = "github")]
                DedupMode::Remote => {
                    email
                        .and_then(|e| _identity_map.get(e))
                        .cloned()
                        .unwrap_or_else(|| name.to_string())
                }
            };

            display_names.entry(merge_key.clone()).or_insert_with(|| name.to_string());

            *commits.entry(merge_key.clone()).or_default() += as_.commits;
            *co_commits.entry(merge_key.clone()).or_default() += as_.co_authored_commits;
            *additions.entry(merge_key.clone()).or_default() += as_.additions;
            *co_additions.entry(merge_key.clone()).or_default() += as_.co_authored_additions;
            *deletions.entry(merge_key.clone()).or_default() += as_.deletions;
            *co_deletions.entry(merge_key.clone()).or_default() += as_.co_authored_deletions;
            let author_langs = lang_data.entry(merge_key.clone()).or_default();
            for (lang, ls) in &as_.languages {
                let entry = author_langs.entry(lang.clone()).or_default();
                entry.0 += ls.additions;
                entry.1 += ls.deletions;
                entry.2 += ls.files_changed;
            }
            if let Some(email) = email {
                let entry = emails.entry(merge_key).or_default();
                if !entry.contains(&email.to_string()) {
                    entry.push(email.to_string());
                }
            }
        }
    }

    let mut rows: Vec<_> = commits
        .into_iter()
        .map(|(key, c)| {
            let co_c = co_commits.get(&key).copied().unwrap_or(0);
            let a = additions.get(&key).copied().unwrap_or(0);
            let co_a = co_additions.get(&key).copied().unwrap_or(0);
            let d = deletions.get(&key).copied().unwrap_or(0);
            let co_d = co_deletions.get(&key).copied().unwrap_or(0);

            let mut langs: Vec<_> = lang_data
                .remove(&key)
                .unwrap_or_default()
                .into_iter()
                .map(|(lang, (la, ld, lf))| (lang, la, ld, lf))
                .collect();
            langs.sort_by_key(|b| std::cmp::Reverse(b.1));

            let top_lang = langs.first().map(|(l, _, _, _)| l.clone()).unwrap_or_default();
            let author_emails = emails.remove(&key).unwrap_or_default();
            let name = display_names.remove(&key).unwrap_or(key);
            AuthorRow {
                name, emails: author_emails,
                commits: c, co_authored_commits: co_c,
                additions: a, co_authored_additions: co_a,
                deletions: d, co_authored_deletions: co_d,
                top_lang,
                languages: langs,
            }
        })
        .collect();

    match sort.unwrap_or(&SortBy::Commits) {
        SortBy::Commits => rows.sort_by_key(|r| std::cmp::Reverse(r.total_commits())),
        SortBy::Additions => rows.sort_by_key(|r| std::cmp::Reverse(r.total_additions())),
        SortBy::Deletions => rows.sort_by_key(|r| std::cmp::Reverse(r.total_deletions())),
        SortBy::Files | SortBy::Name => rows.sort_by(|a, b| a.name.cmp(&b.name)),
    }
    rows
}

fn render_language_table(
    stats: &[PeriodStats],
    totals: &PeriodStats,
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    cols: &[cli::Column],
    compact: bool,
) -> String {
    let mut out = String::new();
    let langs = aggregate_languages(stats, sort);
    let total_files = files_for_period(totals);
    let total_metric = RowMetric {
        commits: totals.total_commits,
        adds: totals.total_additions,
        dels: totals.total_deletions,
        files: total_files,
    };
    let mut layout_rows: Vec<RowMetric> = langs
        .iter()
        .map(|(_, a, d, f)| RowMetric {
            commits: 0,
            adds: *a,
            dels: *d,
            files: *f,
        })
        .collect();
    layout_rows.push(total_metric);
    let layout = ColLayout::build(cols, compact, &layout_rows, num_fmt);
    let line_w = table_line_width(NAME_WIDTH, &layout);

    let _ = writeln!(out, "{}", crate::output::column::header_row("Language", NAME_WIDTH, &layout));
    let _ = writeln!(out, "{}", heavy(line_w).bold());

    for (lang, a, d, f) in &langs {
        let metric = RowMetric {
            commits: 0,
            adds: *a,
            dels: *d,
            files: *f,
        };
        let _ = writeln!(
            out,
            "{}",
            crate::output::column::data_row(lang, NAME_WIDTH, &metric, &layout, num_fmt, "", false)
        );
    }

    let _ = writeln!(out, "{}", heavy(line_w).bold());
    let _ = writeln!(
        out,
        "{}",
        crate::output::column::data_row("Total", NAME_WIDTH, &total_metric, &layout, num_fmt, "", true)
    );
    out
}

fn format_author_display(row: &AuthorRow, mode: &EmailDisplay) -> String {
    match mode {
        EmailDisplay::None => row.name.clone(),
        EmailDisplay::Simple | EmailDisplay::Full => {
            if row.emails.is_empty() {
                return row.name.clone();
            }
            let formatted: Vec<String> = row.emails.iter().map(|email| {
                let full = format!("{} <{email}>", row.name);
                format_author_name(&full, mode).into_owned()
            }).collect();
            if formatted.len() == 1 {
                return formatted.into_iter().next().expect("non-empty");
            }
            let first = formatted[0].clone();
            let rest: Vec<String> = row.emails[1..].iter().enumerate().map(|(i, email)| {
                let full = format!("{} <{email}>", row.name);
                let display = format_author_name(&full, mode).into_owned();
                display.strip_prefix(&row.name).unwrap_or(&formatted[i + 1]).trim_start().to_string()
            }).collect();
            format!("{first}, {}", rest.join(", "))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_author_table(
    stats: &[PeriodStats],
    totals: &PeriodStats,
    email: &EmailDisplay,
    dedup: &DedupMode,
    identity_map: &HashMap<String, String>,
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    cols: &[cli::Column],
    compact: bool,
    inline_tree: bool,
) -> String {
    let mut out = String::new();
    let authors = aggregate_authors(stats, dedup, identity_map, sort);

    let max_author_len = authors
        .iter()
        .map(|row| format_author_display(row, email).len())
        .max()
        .unwrap_or(6)
        .max(6);
    let name_w = max_author_len.clamp(NAME_WIDTH, 60);

    let top_lang_w = authors
        .iter()
        .map(|r| r.top_lang.len())
        .max()
        .unwrap_or(0)
        .max("Top Language".len());

    let mut layout_rows: Vec<RowMetric> = authors
        .iter()
        .map(|r| RowMetric {
            commits: r.total_commits(),
            adds: r.total_additions(),
            dels: r.total_deletions(),
            files: r.languages.iter().map(|(_, _, _, f)| *f).sum(),
        })
        .collect();
    if !inline_tree {
        layout_rows.extend(authors.iter().flat_map(|r| {
            r.languages.iter().map(|(_, a, d, f)| RowMetric {
                commits: 0,
                adds: *a,
                dels: *d,
                files: *f,
            })
        }));
    }
    let total_metric = metric_for_period(totals);
    layout_rows.push(total_metric);
    let layout = ColLayout::build(cols, compact, &layout_rows, num_fmt);
    let line_w =
        1 + name_w + layout.widths.iter().sum::<usize>() + layout.cols.len() * COL_SEP + COL_SEP + top_lang_w;

    let (il_lang_w, il_net_w, il_add_w, il_del_w) = if inline_tree {
        let items: Vec<(&str, u64, u64)> = authors.iter()
            .flat_map(|r| r.languages.iter().map(|(lang, a, d, _)| (lang.as_str(), *a, *d)))
            .collect();
        max_inline_widths(&items, num_fmt)
    } else {
        (0, 0, 0, 0)
    };

    let mut header = crate::output::column::header_row("Author", name_w, &layout);
    header.push_str(&" ".repeat(COL_SEP));
    header.push_str(&format!("{:>w$}", "Top Language".bold(), w = top_lang_w));
    let _ = writeln!(out, "{header}");
    let _ = writeln!(out, "{}", heavy(line_w).bold());

    for row in &authors {
        let display_name = format_author_display(row, email);
        let metric = RowMetric {
            commits: row.total_commits(),
            adds: row.total_additions(),
            dels: row.total_deletions(),
            files: row.languages.iter().map(|(_, _, _, f)| *f).sum(),
        };
        let mut line = crate::output::column::data_row(
            &display_name,
            name_w,
            &metric,
            &layout,
            num_fmt,
            "",
            false,
        );
        line.push_str(&" ".repeat(COL_SEP));
        line.push_str(&format!("{:>w$}", row.top_lang.yellow(), w = top_lang_w));
        let _ = writeln!(out, "{line}");

        if row.languages.len() > 1 {
            if inline_tree {
                let offset = 1 + name_w + COL_SEP + layout.widths.first().copied().unwrap_or(0);
                let pad = " ".repeat(offset);
                for (i, (lang, la, ld, _lf)) in row.languages.iter().enumerate() {
                    let prefix = if i == row.languages.len() - 1 { "└── " } else { "├── " };
                    let entry = format_inline_entry(prefix, lang, *la, *ld, num_fmt, il_lang_w, il_net_w, il_add_w, il_del_w);
                    let _ = writeln!(out, "{}{}", pad, entry);
                }
            } else {
                for (i, (lang, la, ld, lf)) in row.languages.iter().enumerate() {
                    let prefix = if i == row.languages.len() - 1 { "└── " } else { "├── " };
                    let mut sub_line =
                        format!(" {:<w$}", format!("{prefix}{lang}").dimmed(), w = name_w);
                    let sub_metric = RowMetric {
                        commits: 0,
                        adds: *la,
                        dels: *ld,
                        files: *lf,
                    };
                    for (dc, width) in layout.cols.iter().zip(&layout.widths) {
                        sub_line.push_str(&" ".repeat(COL_SEP));
                        if matches!(dc, DisplayCol::Commits) {
                            sub_line.push_str(&format!("{:>w$}", "", w = *width));
                            continue;
                        }
                        sub_line.push_str(&crate::output::column::format_cell(
                            *dc,
                            &sub_metric,
                            num_fmt,
                            *width,
                            layout.change_add_w,
                            layout.change_del_w,
                            false,
                        ));
                    }
                    sub_line.push_str(&" ".repeat(COL_SEP));
                    sub_line.push_str(&format!("{:>w$}", "", w = top_lang_w));
                    let _ = writeln!(out, "{sub_line}");
                }
            }
        }
    }

    let _ = writeln!(out, "{}", heavy(line_w).bold());
    let mut total_line =
        crate::output::column::data_row("Total", name_w, &total_metric, &layout, num_fmt, "", true);
    total_line.push_str(&" ".repeat(COL_SEP));
    total_line.push_str(&format!("{:>w$}", "", w = top_lang_w));
    let _ = writeln!(out, "{total_line}");
    out
}

fn render_period_table(
    stats: &[PeriodStats],
    _totals: &PeriodStats,
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    cols: &[cli::Column],
    compact: bool,
    inline_tree: bool,
) -> String {
    let mut out = String::new();
    let total_langs = aggregate_languages(stats, sort);

    let include_period_rows = has_commits_col(&ColLayout::build(cols, compact, &[], num_fmt));

    let mut layout_rows: Vec<RowMetric> = stats
        .iter()
        .filter(|_| include_period_rows)
        .map(metric_for_period)
        .chain(stats.iter().flat_map(|p| {
            p.by_language.values().map(|ls| RowMetric {
                commits: 0,
                adds: ls.additions,
                dels: ls.deletions,
                files: ls.files_changed,
            })
        }))
        .collect();
    let total_metric = RowMetric {
        commits: stats.iter().map(|p| p.total_commits).sum(),
        adds: total_langs.iter().map(|(_, a, _, _)| *a).sum(),
        dels: total_langs.iter().map(|(_, _, d, _)| *d).sum(),
        files: total_langs.iter().map(|(_, _, _, f)| *f).sum(),
    };
    layout_rows.push(total_metric);
    let layout = ColLayout::build(cols, compact, &layout_rows, num_fmt);
    let line_w = table_line_width(NAME_WIDTH, &layout);
    let show_commit_suffix = !include_period_rows;

    let (il_lang_w, il_net_w, il_add_w, il_del_w) = if inline_tree {
        let items: Vec<(&str, u64, u64)> = stats.iter()
            .flat_map(|p| p.by_language.iter().map(|(lang, ls)| (lang.as_str(), ls.additions, ls.deletions)))
            .chain(total_langs.iter().map(|(lang, a, d, _)| (lang.as_str(), *a, *d)))
            .collect();
        max_inline_widths(&items, num_fmt)
    } else {
        (0, 0, 0, 0)
    };

    if !inline_tree {
        let _ = writeln!(
            out,
            "{}",
            crate::output::column::header_row("Language", NAME_WIDTH, &layout)
        );
    }
    let _ = writeln!(out, "{}", heavy(line_w).bold());

    for period in stats {
        if show_commit_suffix {
            let _ = writeln!(
                out,
                " {} ({})",
                period.period_label.bright_blue().bold(),
                format!("{} commits", period.total_commits).dimmed(),
            );
        } else {
            let _ = writeln!(out, " {}", period.period_label.bright_blue().bold());
        }

        let mut langs: Vec<_> = period.by_language.iter().collect();
        match sort.unwrap_or(&SortBy::Additions) {
            SortBy::Additions | SortBy::Commits => langs.sort_by_key(|b| std::cmp::Reverse(b.1.additions)),
            SortBy::Deletions => langs.sort_by_key(|b| std::cmp::Reverse(b.1.deletions)),
            SortBy::Files => langs.sort_by_key(|b| std::cmp::Reverse(b.1.files_changed)),
            SortBy::Name => langs.sort_by(|a, b| a.0.cmp(b.0)),
        }

        if inline_tree {
            let pad = " ".repeat(NAME_WIDTH + 1);
            for (i, (lang, ls)) in langs.iter().enumerate() {
                let prefix = if i == langs.len() - 1 { "└── " } else { "├── " };
                let entry = format_inline_entry(prefix, lang, ls.additions, ls.deletions, num_fmt, il_lang_w, il_net_w, il_add_w, il_del_w);
                let _ = writeln!(out, "{}{}", pad, entry);
            }
        } else {
            for (i, (lang, ls)) in langs.iter().enumerate() {
                let prefix = if i == langs.len() - 1 { "└── " } else { "├── " };
                let metric = RowMetric {
                    commits: 0,
                    adds: ls.additions,
                    dels: ls.deletions,
                    files: ls.files_changed,
                };
                let _ = writeln!(
                    out,
                    "{}",
                    crate::output::column::data_row(
                        &format!("{prefix}{lang}"),
                        NAME_WIDTH,
                        &metric,
                        &layout,
                        num_fmt,
                        "",
                        false,
                    )
                );
            }
        }
    }

    let _ = writeln!(out, "{}", heavy(line_w).bold());
    if show_commit_suffix {
        let _ = writeln!(
            out,
            " {} ({})",
            "Total".bold(),
            format!("{} commits", total_metric.commits).dimmed(),
        );
    } else {
        let _ = writeln!(out, " {}", "Total".bold());
    }

    if inline_tree {
        let pad = " ".repeat(NAME_WIDTH + 1);
        for (i, (lang, a, d, _f)) in total_langs.iter().enumerate() {
            let prefix = if i == total_langs.len() - 1 { "└── " } else { "├── " };
            let entry = format_inline_entry(prefix, lang, *a, *d, num_fmt, il_lang_w, il_net_w, il_add_w, il_del_w);
            let _ = writeln!(out, "{}{}", pad, entry);
        }
    } else {
        for (i, (lang, a, d, f)) in total_langs.iter().enumerate() {
            let prefix = if i == total_langs.len() - 1 { "└── " } else { "├── " };
            let metric = RowMetric {
                commits: 0,
                adds: *a,
                dels: *d,
                files: *f,
            };
            let _ = writeln!(
                out,
                "{}",
                crate::output::column::data_row(
                    &format!("{prefix}{lang}"),
                    NAME_WIDTH,
                    &metric,
                    &layout,
                    num_fmt,
                    "",
                    false,
                )
            );
        }
    }

    out
}

#[allow(clippy::too_many_arguments)]
pub fn render_stats_table(
    stats: &[PeriodStats],
    totals: &PeriodStats,
    group_by: &GroupBy,
    email_display: &EmailDisplay,
    dedup: &DedupMode,
    identity_map: &HashMap<String, String>,
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    cols: &[cli::Column],
    compact: bool,
    inline_tree: bool,
) -> String {
    if stats.is_empty() {
        return "No data to display".to_string();
    }

    match group_by {
        GroupBy::Language => render_language_table(stats, totals, sort, num_fmt, cols, compact),
        GroupBy::Author => render_author_table(stats, totals, email_display, dedup, identity_map, sort, num_fmt, cols, compact, inline_tree),
        GroupBy::Period => render_period_table(stats, totals, sort, num_fmt, cols, compact, inline_tree),
        GroupBy::Repo => render_period_table(stats, totals, sort, num_fmt, cols, compact, inline_tree),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_group_tree(
    nodes: &[GroupNode],
    _leaf_group: &GroupBy,
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    cols: &[cli::Column],
    compact: bool,
    _inline_tree: bool,
) -> String {
    if nodes.is_empty() {
        return "No data to display".to_string();
    }

    let mut out = String::new();
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }

        let mut layout_rows = Vec::new();
        if node.children.is_empty() {
            layout_rows.extend(node.stats.by_language.values().map(|ls| RowMetric {
                commits: 0,
                adds: ls.additions,
                dels: ls.deletions,
                files: ls.files_changed,
            }));
        } else {
            for child in &node.children {
                collect_group_tree_metrics(child, &mut layout_rows);
            }
        }
        let total_metric = metric_for_period(&node.stats);
        layout_rows.push(total_metric);
        let layout = ColLayout::build(cols, compact, &layout_rows, num_fmt);
        let line_w = table_line_width(NAME_WIDTH, &layout);
        let show_commit_suffix = !has_commits_col(&layout);

        let add_str = format!("+{}", format_num(node.stats.total_additions, num_fmt));
        let del_str = format!("-{}", format_num(node.stats.total_deletions, num_fmt));
        let _ = writeln!(
            out,
            "\n━━ {} ({} commits, {}/{})",
            node.label.bright_blue().bold(),
            node.stats.total_commits,
            add_str.green(),
            del_str.red(),
        );
        let _ = writeln!(
            out,
            "{}",
            crate::output::column::header_row("Group / Language", NAME_WIDTH, &layout)
        );
        let _ = writeln!(out, "{}", heavy(line_w).bold());

        if node.children.is_empty() {
            render_lang_leaves(&mut out, &node.stats, &[], sort, num_fmt, &layout);
        } else {
            let n = node.children.len();
            for (j, child) in node.children.iter().enumerate() {
                render_subgroup_node(
                    &mut out,
                    child,
                    &[],
                    j == n - 1,
                    sort,
                    num_fmt,
                    &layout,
                    show_commit_suffix,
                );
            }
        }

        let _ = writeln!(out, "{}", heavy(line_w).bold());
        let total_label = if show_commit_suffix {
            format!("Total ({} commits)", node.stats.total_commits)
        } else {
            "Total".to_string()
        };
        let _ = writeln!(
            out,
            "{}",
            crate::output::column::data_row(
                &total_label,
                NAME_WIDTH,
                &total_metric,
                &layout,
                num_fmt,
                "",
                true,
            )
        );
    }

    if nodes.len() > 1 {
        let grand = crate::stats::aggregator::aggregate_totals(
            &nodes.iter().map(|n| n.stats.clone()).collect::<Vec<_>>(),
        );
        let add_str = format!("+{}", format_num(grand.total_additions, num_fmt));
        let del_str = format!("-{}", format_num(grand.total_deletions, num_fmt));
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", "━".repeat(LINE_WIDTH));
        let _ = writeln!(
            out,
            "  Grand Total: {} commits, {}/{}",
            grand.total_commits,
            add_str.green(),
            del_str.red(),
        );
    }

    out
}

fn collect_group_tree_metrics(node: &GroupNode, rows: &mut Vec<RowMetric>) {
    rows.push(metric_for_period(&node.stats));
    if node.children.is_empty() {
        rows.extend(node.stats.by_language.values().map(|ls| RowMetric {
            commits: 0,
            adds: ls.additions,
            dels: ls.deletions,
            files: ls.files_changed,
        }));
    } else {
        for child in &node.children {
            collect_group_tree_metrics(child, rows);
        }
    }
}

fn build_tree_prefix(ancestors_last: &[bool], is_last: bool) -> String {
    let mut s = String::new();
    for &last in ancestors_last {
        s.push_str(if last { "    " } else { "│   " });
    }
    s.push_str(if is_last { "└── " } else { "├── " });
    s
}

#[allow(clippy::too_many_arguments)]
fn render_subgroup_node(
    out: &mut String,
    node: &GroupNode,
    ancestors_last: &[bool],
    is_last: bool,
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    layout: &ColLayout,
    show_commit_suffix: bool,
) {
    let prefix = build_tree_prefix(ancestors_last, is_last);
    let label = if show_commit_suffix {
        format!("{}{} ({} commits)", prefix, node.label, node.stats.total_commits)
    } else {
        format!("{}{}", prefix, node.label)
    };
    let metric = metric_for_period(&node.stats);
    let _ = writeln!(
        out,
        "{}",
        crate::output::column::data_row(
            &label,
            NAME_WIDTH,
            &metric,
            layout,
            num_fmt,
            "",
            false,
        )
    );

    let mut next_ancestors: Vec<bool> = ancestors_last.to_vec();
    next_ancestors.push(is_last);

    if node.children.is_empty() {
        render_lang_leaves(out, &node.stats, &next_ancestors, sort, num_fmt, layout);
    } else {
        let n = node.children.len();
        for (j, child) in node.children.iter().enumerate() {
            render_subgroup_node(
                out,
                child,
                &next_ancestors,
                j == n - 1,
                sort,
                num_fmt,
                layout,
                show_commit_suffix,
            );
        }
    }
}

fn render_lang_leaves(
    out: &mut String,
    stats: &PeriodStats,
    ancestors_last: &[bool],
    sort: Option<&SortBy>,
    num_fmt: NumberFormat,
    layout: &ColLayout,
) {
    let mut langs: Vec<_> = stats.by_language.iter().collect();
    match sort.unwrap_or(&SortBy::Additions) {
        SortBy::Additions | SortBy::Commits => {
            langs.sort_by_key(|b| std::cmp::Reverse(b.1.additions))
        }
        SortBy::Deletions => langs.sort_by_key(|b| std::cmp::Reverse(b.1.deletions)),
        SortBy::Files => langs.sort_by_key(|b| std::cmp::Reverse(b.1.files_changed)),
        SortBy::Name => langs.sort_by(|a, b| a.0.cmp(b.0)),
    }
    let n = langs.len();
    for (i, (lang, ls)) in langs.iter().enumerate() {
        let prefix = build_tree_prefix(ancestors_last, i == n - 1);
        let metric = RowMetric {
            commits: 0,
            adds: ls.additions,
            dels: ls.deletions,
            files: ls.files_changed,
        };
        let _ = writeln!(
            out,
            "{}",
            crate::output::column::data_row(
                &format!("{prefix}{lang}"),
                NAME_WIDTH,
                &metric,
                layout,
                num_fmt,
                "",
                false,
            )
        );
    }
}

pub fn render_scan_table(repos: &[PathBuf]) -> String {
    if repos.is_empty() {
        return "No data to display".to_string();
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["#", "Repository Path"]);

    for (i, repo) in repos.iter().enumerate() {
        table.add_row(vec![(i + 1).to_string(), repo.display().to_string()]);
    }

    if let Some(col) = table.column_mut(0) {
        col.set_cell_alignment(CellAlignment::Right);
    }
    if let Some(col) = table.column_mut(1) {
        col.set_cell_alignment(CellAlignment::Left);
    }

    table.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::cli::Column;
    use crate::stats::models::{AuthorStats, LangStats};

    fn default_cols() -> Vec<Column> { Column::default_set() }

    fn make_period(label: &str, langs: Vec<(&str, u64, u64, u64)>, commits: u64) -> PeriodStats {
        let mut by_language = HashMap::new();
        let mut total_add = 0u64;
        let mut total_del = 0u64;
        for (name, add, del, files) in &langs {
            total_add += add;
            total_del += del;
            by_language.insert(
                name.to_string(),
                LangStats {
                    additions: *add,
                    deletions: *del,
                    files_changed: *files,
                    ..Default::default()
                },
            );
        }
        PeriodStats {
            period_label: label.to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: commits,
            total_additions: total_add,
            total_deletions: total_del,
            total_net_modifications: 0,
            total_net_additions: 0,
        }
    }

    type AuthorRow<'a> = (&'a str, u64, u64, u64, Vec<(&'a str, u64, u64, u64)>);

    fn make_period_with_authors(
        label: &str,
        langs: Vec<(&str, u64, u64, u64)>,
        authors: Vec<AuthorRow>,
    ) -> PeriodStats {
        let mut period = make_period(label, langs, authors.iter().map(|(_, c, _, _, _)| c).sum());
        for (name, commits, additions, deletions, author_langs) in authors {
            let mut languages = HashMap::new();
            for (lang, a, d, f) in author_langs {
                languages.insert(
                    lang.to_string(),
                    LangStats {
                        additions: a,
                        deletions: d,
                        files_changed: f,
                        ..Default::default()
                    },
                );
            }
            period.by_author.insert(
                name.to_string(),
                AuthorStats {
                    commits,
                    co_authored_commits: 0,
                    additions,
                    co_authored_additions: 0,
                    deletions,
                    co_authored_deletions: 0,
                    languages,
                    ..Default::default()
                },
            );
        }
        period
    }

    #[test]
    fn test_group_by_language_format() {
        // Disable colors for deterministic test output
        colored::control::set_override(false);

        let period = make_period(
            "2025-01",
            vec![("Rust", 150, 30, 5), ("Python", 40, 10, 2)],
            3,
        );
        let totals = make_period(
            "Total",
            vec![("Rust", 150, 30, 5), ("Python", 40, 10, 2)],
            3,
        );

        let columns = default_cols();
        let output = render_stats_table(
            &[period],
            &totals,
            &GroupBy::Language,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        assert!(output.contains("━"));
        assert!(output.contains("Language"));
        assert!(output.contains("Changes"));
        assert!(output.contains("Files"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Python"));
        assert!(output.contains("150"));
        assert!(output.contains("30"));
        assert!(output.contains("40"));
        assert!(output.contains("10"));
        assert!(output.contains("Total"));

        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].contains("Language"));
        assert!(lines[1].contains("━"));
        // Rust has more additions, should appear first
        assert!(lines[2].contains("Rust"));
        assert!(lines[3].contains("Python"));
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
        let totals = make_period("Total", vec![("Rust", 200, 40, 7)], 7);

        let columns = default_cols();
        let output = render_stats_table(
            &[period],
            &totals,
            &GroupBy::Author,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        assert!(output.contains("━"));
        assert!(output.contains("Author"));
        assert!(output.contains("Commits"));
        assert!(output.contains("Changes"));
        assert!(output.contains("Top Language"));
        assert!(output.contains("alice"));
        assert!(output.contains("bob"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Total"));

        let lines: Vec<&str> = output.lines().collect();
        // alice has more commits, should appear first
        assert!(lines[2].contains("alice"));
        assert!(lines[3].contains("bob"));
    }

    #[test]
    fn test_group_by_author_language_tree() {
        colored::control::set_override(false);

        let period = make_period_with_authors(
            "2025-01",
            vec![("Rust", 200, 40, 7), ("Python", 50, 10, 3)],
            vec![
                ("alice", 5, 150, 30, vec![("Rust", 120, 25, 4), ("Python", 30, 5, 1)]),
                ("bob", 2, 50, 10, vec![("Rust", 50, 10, 2)]),
            ],
        );
        let totals = make_period("Total", vec![("Rust", 200, 40, 7), ("Python", 50, 10, 3)], 7);

        let columns = default_cols();
        let output = render_stats_table(
            &[period],
            &totals,
            &GroupBy::Author,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        // alice has 2 languages, should show tree
        assert!(output.contains("alice"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Python"));
        // bob has only 1 language, should NOT show tree
        // Check that language sub-rows appear (indented)
        let has_tree_rust = output.lines().any(|l| l.contains("├── Rust") || l.contains("└── Rust"));
        assert!(has_tree_rust, "Should have tree-style language sub-rows");
    }

    #[test]
    fn test_group_by_period_format() {
        colored::control::set_override(false);

        let p1 = make_period("2025-01", vec![("Rust", 100, 20, 5), ("Go", 50, 10, 3)], 5);
        let p2 = make_period("2025-02", vec![("Rust", 80, 15, 4)], 3);
        let totals = make_period("Total", vec![("Rust", 180, 35, 9), ("Go", 50, 10, 3)], 8);

        let columns = default_cols();
        let output = render_stats_table(
            &[p1, p2],
            &totals,
            &GroupBy::Period,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        assert!(output.contains("━"));
        assert!(output.contains("─"));
        assert!(output.contains("Commits"));
        assert!(output.contains("2025-01"));
        assert!(output.contains("2025-02"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Go"));
        assert!(output.contains("Total"));
        assert!(!output.contains("5 commits"));
        assert!(!output.contains("3 commits"));
        assert!(!output.contains("8 commits"));
    }

    #[test]
    fn test_non_compact_keeps_adds_and_dels_headers() {
        colored::control::set_override(false);

        let p = make_period("2025-01", vec![("Rust", 100, 20, 5)], 2);
        let totals = make_period("Total", vec![("Rust", 100, 20, 5)], 2);
        let columns = vec![Column::Commits, Column::Adds, Column::Dels, Column::Files];

        let output = render_stats_table(
            &[p],
            &totals,
            &GroupBy::Language,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            false,
            false,
        );

        assert!(output.contains("Additions"));
        assert!(output.contains("Deletions"));
        assert!(!output.contains("Changes"));
    }

    #[test]
    fn test_compact_changes_only_when_adds_dels_adjacent() {
        colored::control::set_override(false);

        let p = make_period("2025-01", vec![("Rust", 100, 20, 5)], 2);
        let totals = make_period("Total", vec![("Rust", 100, 20, 5)], 2);
        let columns = vec![Column::Adds, Column::Files, Column::Dels];

        let output = render_stats_table(
            &[p],
            &totals,
            &GroupBy::Language,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        assert!(output.contains("Additions"));
        assert!(output.contains("Deletions"));
        assert!(!output.contains("Changes"));
    }

    #[test]
    fn test_net_column_signed_colored_output_present() {
        colored::control::set_override(false);

        let p1 = make_period("2025-01", vec![("Rust", 100, 20, 5)], 2);
        let p2 = make_period("2025-02", vec![("Rust", 10, 40, 2)], 1);
        let totals = make_period("Total", vec![("Rust", 110, 60, 7)], 3);
        let columns = vec![Column::Commits, Column::Net, Column::Files];

        let output = render_stats_table(
            &[p1, p2],
            &totals,
            &GroupBy::Period,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        assert!(output.contains("Net"));
        assert!(output.contains("+80"));
        assert!(output.contains("-30"));
    }

    #[test]
    fn test_period_label_commit_suffix_drops_when_commits_column_selected() {
        colored::control::set_override(false);

        let p = make_period("2025-01", vec![("Rust", 100, 20, 5)], 5);
        let totals = make_period("Total", vec![("Rust", 100, 20, 5)], 5);
        let with_commits = vec![Column::Commits, Column::Adds, Column::Dels, Column::Files];
        let without_commits = vec![Column::Adds, Column::Dels, Column::Files];

        let with_out = render_stats_table(
            std::slice::from_ref(&p),
            &totals,
            &GroupBy::Period,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &with_commits,
            true,
            false,
        );
        let without_out = render_stats_table(
            &[p],
            &totals,
            &GroupBy::Period,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &without_commits,
            true,
            false,
        );

        assert!(!with_out.contains("2025-01 (5 commits)"));
        assert!(!with_out.contains("Total (5 commits)"));
        assert!(without_out.contains("2025-01 (5 commits)"));
        assert!(without_out.contains("Total (5 commits)"));
    }

    #[test]
    fn test_group_by_repo_format() {
        colored::control::set_override(false);

        let p1 = make_period(
            "owner/repo-a",
            vec![("Rust", 100, 20, 5), ("Go", 50, 10, 3)],
            5,
        );
        let p2 = make_period("owner/repo-b", vec![("Rust", 80, 15, 4)], 3);
        let totals = make_period("Total", vec![("Rust", 180, 35, 9), ("Go", 50, 10, 3)], 8);

        let columns = default_cols();
        let output = render_stats_table(
            &[p1, p2],
            &totals,
            &GroupBy::Repo,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            NumberFormat::Plain,
            &columns,
            true,
            false,
        );

        assert!(output.contains("owner/repo-a"));
        assert!(output.contains("owner/repo-b"));
        assert!(output.contains("Commits"));
        assert!(!output.contains("5 commits"));
        assert!(!output.contains("3 commits"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Go"));
        assert!(output.contains("Total"));
        assert!(!output.contains("8 commits"));
    }

    #[test]
    fn test_empty_returns_no_data() {
        let totals = make_period("Total", vec![], 0);

        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Language, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, NumberFormat::Plain, &default_cols(), true, false),
            "No data to display"
        );
        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Author, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, NumberFormat::Plain, &default_cols(), true, false),
            "No data to display"
        );
        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Period, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, NumberFormat::Plain, &default_cols(), true, false),
            "No data to display"
        );
        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Repo, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, NumberFormat::Plain, &default_cols(), true, false),
            "No data to display"
        );
    }

    #[test]
    fn test_short_numbers() {
        assert_eq!(format_num(999, NumberFormat::Short), "999");
        assert_eq!(format_num(1000, NumberFormat::Short), "1.0k");
        assert_eq!(format_num(1500, NumberFormat::Short), "1.5k");
        assert_eq!(format_num(1_000_000, NumberFormat::Short), "1.0M");
        assert_eq!(format_num(2_500_000, NumberFormat::Short), "2.5M");
        assert_eq!(format_num(42, NumberFormat::Plain), "42");
        assert_eq!(format_num(1500, NumberFormat::Plain), "1500");
    }

    #[test]
    fn test_separated_numbers() {
        assert_eq!(format_num(0, NumberFormat::Separated), "0");
        assert_eq!(format_num(999, NumberFormat::Separated), "999");
        assert_eq!(format_num(1_000, NumberFormat::Separated), "1,000");
        assert_eq!(format_num(12_345, NumberFormat::Separated), "12,345");
        assert_eq!(format_num(1_234_567, NumberFormat::Separated), "1,234,567");
        assert_eq!(format_num(1_000_000_000, NumberFormat::Separated), "1,000,000,000");
    }

    #[test]
    fn scan_table_contains_paths() {
        let repos = vec![
            PathBuf::from("/home/user/repo-a"),
            PathBuf::from("/home/user/repo-b"),
        ];

        let output = render_scan_table(&repos);

        assert!(output.contains("1"));
        assert!(output.contains("2"));
        assert!(output.contains("repo-a"));
        assert!(output.contains("repo-b"));
        assert!(output.contains("Repository Path"));
    }

    #[test]
    fn scan_table_empty_returns_no_data() {
        let output = render_scan_table(&[]);
        assert!(output.contains("No data to display"));
    }
}
