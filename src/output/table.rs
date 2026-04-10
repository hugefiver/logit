use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;

use colored::Colorize;
use comfy_table::{CellAlignment, ContentArrangement, Table};

use crate::cli::{DedupMode, EmailDisplay, GroupBy, SortBy};
use crate::stats::models::{GroupNode, PeriodStats};

const LINE_WIDTH: usize = 81;
const NAME_WIDTH: usize = 20;

fn format_num(n: u64, short: bool) -> String {
    if !short {
        return n.to_string();
    }
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Format a value with optional co-author count, aligned so that `(` is at a
/// consistent column across all rows. `main_w` = max width of the main number
/// across all rows in the column. `total_w` = overall column width.
fn format_co_aligned(
    val: u64,
    co_val: u64,
    short: bool,
    main_w: usize,
    total_w: usize,
) -> String {
    let total = val + co_val;
    let main = format!("{:>main_w$}", format_num(total, short));
    let suffix = if co_val > 0 {
        format!(" ({})", format_num(co_val, short))
    } else {
        String::new()
    };
    let content = format!("{main}{suffix}");
    let pad = total_w.saturating_sub(content.len());
    format!("{}{}", " ".repeat(pad), content)
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

fn col_widths_3() -> [usize; 3] {
    let remaining = LINE_WIDTH - NAME_WIDTH - 1;
    let col = remaining / 3;
    [col, col, remaining - 2 * col]
}

fn format_row_3_colored(name: &str, vals: [&str; 3], indent: usize, color_vals: bool) -> String {
    let widths = col_widths_3();
    let prefix = " ".repeat(indent);
    let name_w = NAME_WIDTH - indent;

    let v0 = format!("{:>w$}", vals[0], w = widths[0]);
    let v1 = format!("{:>w$}", vals[1], w = widths[1]);
    let v2 = format!("{:>w$}", vals[2], w = widths[2]);

    if color_vals {
        format!(
            " {prefix}{:<name_w$}{}{}{}",
            name.cyan(),
            v0.green(),
            v1.red(),
            v2.yellow(),
        )
    } else {
        format!(
            " {prefix}{:<name_w$}{}{}{}",
            name, v0, v1, v2,
        )
    }
}

fn col_widths_compact() -> (usize, usize) {
    let widths = col_widths_3();
    (widths[0] + widths[1], widths[2])
}

fn max_change_widths(items: &[(u64, u64)], short: bool) -> (usize, usize) {
    let max_add = items
        .iter()
        .map(|(a, _)| format!("+{}", format_num(*a, short)).len())
        .max()
        .unwrap_or(2);
    let max_del = items
        .iter()
        .map(|(_, d)| format!("-{}", format_num(*d, short)).len())
        .max()
        .unwrap_or(2);
    (max_add, max_del)
}

fn format_changes_aligned(
    adds: u64,
    dels: u64,
    width: usize,
    short: bool,
    bold: bool,
    add_w: usize,
    del_w: usize,
) -> String {
    let add_s = format!("+{}", format_num(adds, short));
    let del_s = format!("-{}", format_num(dels, short));
    let add_f = format!("{:>w$}", add_s, w = add_w);
    let del_f = format!("{:>w$}", del_s, w = del_w);
    let plain_len = add_w + 1 + del_w;
    let pad = width.saturating_sub(plain_len);
    if bold {
        format!(
            "{}{} {}",
            " ".repeat(pad),
            add_f.green().bold(),
            del_f.red().bold()
        )
    } else {
        format!("{}{} {}", " ".repeat(pad), add_f.green(), del_f.red())
    }
}

fn max_inline_widths(items: &[(&str, u64, u64)], short: bool) -> (usize, usize, usize, usize) {
    let lang_w = items.iter().map(|(l, _, _)| l.len()).max().unwrap_or(1);
    let net_w = items.iter().map(|(_, a, d)| {
        let net = *a as i64 - *d as i64;
        format_num(net.unsigned_abs(), short).len() + 1
    }).max().unwrap_or(2);
    let add_w = items.iter().map(|(_, a, _)| {
        format!("+{}", format_num(*a, short)).len()
    }).max().unwrap_or(2);
    let del_w = items.iter().map(|(_, _, d)| {
        format!("-{}", format_num(*d, short)).len()
    }).max().unwrap_or(2);
    (lang_w, net_w, add_w, del_w)
}

#[allow(clippy::too_many_arguments)]
fn format_inline_entry(
    prefix: &str,
    lang: &str,
    adds: u64,
    dels: u64,
    short: bool,
    lang_w: usize,
    net_w: usize,
    add_w: usize,
    del_w: usize,
) -> String {
    let net = adds as i64 - dels as i64;
    let net_s = format_num(net.unsigned_abs(), short);
    let net_display = if net >= 0 { format!("+{net_s}") } else { format!("-{net_s}") };
    let net_aligned = format!("{:>w$}", net_display, w = net_w);
    let net_colored = if net >= 0 {
        net_aligned.green().to_string()
    } else {
        net_aligned.red().to_string()
    };
    let add_s = format!("+{}", format_num(adds, short));
    let del_s = format!("-{}", format_num(dels, short));
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

fn format_header_3(headers: [&str; 4]) -> String {
    let widths = col_widths_3();
    format!(
        " {:<name_w$}{:>w0$}{:>w1$}{:>w2$}",
        headers[0].bold(),
        headers[1].bold(),
        headers[2].bold(),
        headers[3].bold(),
        name_w = NAME_WIDTH,
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
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
        SortBy::Additions | SortBy::Commits => rows.sort_by(|a, b| b.1.cmp(&a.1)),
        SortBy::Deletions => rows.sort_by(|a, b| b.2.cmp(&a.2)),
        SortBy::Files => rows.sort_by(|a, b| b.3.cmp(&a.3)),
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
            langs.sort_by(|a, b| b.1.cmp(&a.1));

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
    short: bool,
    compact: bool,
) -> String {
    let mut out = String::new();
    let langs = aggregate_languages(stats, sort);
    let total_files: u64 = totals.by_language.values().map(|ls| ls.files_changed).sum();

    if compact {
        let (change_w, file_w) = col_widths_compact();
        let change_items: Vec<(u64, u64)> = langs.iter().map(|(_, a, d, _)| (*a, *d))
            .chain(std::iter::once((totals.total_additions, totals.total_deletions)))
            .collect();
        let (add_w, del_w) = max_change_widths(&change_items, short);
        let _ = writeln!(out, " {:<name_w$}{:>cw$}{:>fw$}",
            "Language".bold(), "Changes".bold(), "Files".bold(),
            name_w = NAME_WIDTH, cw = change_w, fw = file_w);
        let _ = writeln!(out, "{}", heavy(LINE_WIDTH).bold());

        for (lang, a, d, f) in &langs {
            let fs = format!("{:>w$}", format_num(*f, short), w = file_w);
            let _ = writeln!(out, " {:<nw$}{}{}",
                lang.cyan(),
                format_changes_aligned(*a, *d, change_w, short, false, add_w, del_w),
                fs.yellow(),
                nw = NAME_WIDTH);
        }

        let _ = writeln!(out, "{}", heavy(LINE_WIDTH).bold());
        let fs = format!("{:>w$}", format_num(total_files, short), w = file_w);
        let _ = writeln!(out, " {:<nw$}{}{}",
            "Total".bold(),
            format_changes_aligned(totals.total_additions, totals.total_deletions, change_w, short, true, add_w, del_w),
            fs.bold(),
            nw = NAME_WIDTH);
    } else {
        let _ = writeln!(out, "{}", format_header_3(["Language", "Additions", "Deletions", "Files"]));
        let _ = writeln!(out, "{}", heavy(LINE_WIDTH).bold());


        for (lang, a, d, f) in &langs {
            let _ = writeln!(out, "{}", format_row_3_colored(
                lang,
                [&format_num(*a, short), &format_num(*d, short), &format_num(*f, short)],
                0, true,
            ));
        }

        let _ = writeln!(out, "{}", heavy(LINE_WIDTH).bold());
        let _ = writeln!(out, "{}", format_row_3_colored(
            "Total",
            [&format_num(totals.total_additions, short), &format_num(totals.total_deletions, short), &format_num(total_files, short)],
            0, false,
        ).bold());
    }
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
    short: bool,
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
    let num_w = 15;
    let change_w = num_w * 2;
    let line_w = if compact {
        name_w + 1 + num_w + change_w + num_w
    } else {
        name_w + 1 + num_w * 4
    };

    // Pre-compute max main-number width per column so `(` aligns across rows
    let max_commit_main = authors
        .iter()
        .map(|r| format_num(r.total_commits(), short).len())
        .chain(std::iter::once(format_num(totals.total_commits, short).len()))
        .max()
        .unwrap_or(1);
    let max_add_main = authors
        .iter()
        .map(|r| format_num(r.total_additions(), short).len())
        .chain(std::iter::once(format_num(totals.total_additions, short).len()))
        .max()
        .unwrap_or(1);
    let max_del_main = authors
        .iter()
        .map(|r| format_num(r.total_deletions(), short).len())
        .chain(std::iter::once(format_num(totals.total_deletions, short).len()))
        .max()
        .unwrap_or(1);

    let change_items: Vec<(u64, u64)> = authors.iter()
        .flat_map(|r| {
            let main = (r.total_additions(), r.total_deletions());
            std::iter::once(main).chain(r.languages.iter().map(|(_, a, d, _)| (*a, *d)))
        })
        .chain(std::iter::once((totals.total_additions, totals.total_deletions)))
        .collect();
    let (chg_add_w, chg_del_w) = max_change_widths(&change_items, short);

    let (il_lang_w, il_net_w, il_add_w, il_del_w) = if inline_tree {
        let items: Vec<(&str, u64, u64)> = authors.iter()
            .flat_map(|r| r.languages.iter().map(|(lang, a, d, _)| (lang.as_str(), *a, *d)))
            .collect();
        max_inline_widths(&items, short)
    } else {
        (0, 0, 0, 0)
    };

    if compact {
        let _ = writeln!(
            out,
            " {:<name_w$}{:>num_w$}{:>change_w$}{:>num_w$}",
            "Author".bold(), "Commits".bold(), "Changes".bold(), "Top Language".bold(),
        );
    } else {
        let _ = writeln!(
            out,
            " {:<name_w$}{:>num_w$}{:>num_w$}{:>num_w$}{:>num_w$}",
            "Author".bold(), "Commits".bold(), "Additions".bold(), "Deletions".bold(), "Top Language".bold(),
        );
    }
    let _ = writeln!(out, "{}", heavy(line_w).bold());

    for row in &authors {
        let display_name = format_author_display(row, email);
        let c = format_co_aligned(row.commits, row.co_authored_commits, short, max_commit_main, num_w);

        if compact {
            let total_adds = row.additions + row.co_authored_additions;
            let total_dels = row.deletions + row.co_authored_deletions;
            let _ = writeln!(
                out,
                " {:<name_w$}{}{}{:>num_w$}",
                display_name.bright_white(),
                c.bright_cyan(),
                format_changes_aligned(total_adds, total_dels, change_w, short, false, chg_add_w, chg_del_w),
                row.top_lang.yellow(),
            );
        } else {
            let a = format_co_aligned(row.additions, row.co_authored_additions, short, max_add_main, num_w);
            let d = format_co_aligned(row.deletions, row.co_authored_deletions, short, max_del_main, num_w);
            let _ = writeln!(
                out,
                " {:<name_w$}{}{}{}{:>num_w$}",
                display_name.bright_white(),
                c.bright_cyan(),
                a.green(),
                d.red(),
                row.top_lang.yellow(),
            );
        }

        if row.languages.len() > 1 {
            if inline_tree {
                let offset = name_w + num_w + 1;
                let pad = " ".repeat(offset);
                for (i, (lang, la, ld, _lf)) in row.languages.iter().enumerate() {
                    let prefix = if i == row.languages.len() - 1 { "└── " } else { "├── " };
                    let entry = format_inline_entry(prefix, lang, *la, *ld, short, il_lang_w, il_net_w, il_add_w, il_del_w);
                    let _ = writeln!(out, "{}{}", pad, entry);
                }
            } else if compact {
                for (i, (lang, la, ld, _lf)) in row.languages.iter().enumerate() {
                    let prefix = if i == row.languages.len() - 1 { "└── " } else { "├── " };
                    let _ = writeln!(
                        out,
                        " {:<name_w$}{:>num_w$}{}{:>num_w$}",
                        format!("{prefix}{lang}").dimmed(),
                        "",
                        format_changes_aligned(*la, *ld, change_w, short, false, chg_add_w, chg_del_w),
                        "",
                    );
                }
            } else {
                for (i, (lang, la, ld, _lf)) in row.languages.iter().enumerate() {
                    let prefix = if i == row.languages.len() - 1 { "└── " } else { "├── " };
                    let la_s = format_num(*la, short);
                    let ld_s = format_num(*ld, short);
                    let _ = writeln!(
                        out,
                        " {:<name_w$}{:>num_w$}{:>num_w$}{:>num_w$}{:>num_w$}",
                        format!("{prefix}{lang}").dimmed(),
                        "",
                        la_s.dimmed(),
                        ld_s.dimmed(),
                        "",
                    );
                }
            }
        }
    }

    let _ = writeln!(out, "{}", heavy(line_w).bold());
    if compact {
        let _ = writeln!(
            out,
            " {:<name_w$}{}{}{:>num_w$}",
            "Total".bold(),
            format_co_aligned(totals.total_commits, 0, short, max_commit_main, num_w).bold(),
            format_changes_aligned(totals.total_additions, totals.total_deletions, change_w, short, true, chg_add_w, chg_del_w),
            "",
        );
    } else {
        let _ = writeln!(
            out,
            " {:<name_w$}{}{}{}{:>num_w$}",
            "Total".bold(),
            format_co_aligned(totals.total_commits, 0, short, max_commit_main, num_w).bold(),
            format_co_aligned(totals.total_additions, 0, short, max_add_main, num_w).bold(),
            format_co_aligned(totals.total_deletions, 0, short, max_del_main, num_w).bold(),
            "",
        );
    }
    out
}

fn render_period_table(
    stats: &[PeriodStats],
    _totals: &PeriodStats,
    sort: Option<&SortBy>,
    short: bool,
    compact: bool,
    inline_tree: bool,
) -> String {
    let mut out = String::new();
    let total_langs = aggregate_languages(stats, sort);

    let change_items: Vec<(u64, u64)> = stats.iter()
        .flat_map(|p| p.by_language.values().map(|ls| (ls.additions, ls.deletions)))
        .chain(total_langs.iter().map(|(_, a, d, _)| (*a, *d)))
        .collect();
    let (period_add_w, period_del_w) = max_change_widths(&change_items, short);

    let (il_lang_w, il_net_w, il_add_w, il_del_w) = if inline_tree {
        let items: Vec<(&str, u64, u64)> = stats.iter()
            .flat_map(|p| p.by_language.iter().map(|(lang, ls)| (lang.as_str(), ls.additions, ls.deletions)))
            .chain(total_langs.iter().map(|(lang, a, d, _)| (lang.as_str(), *a, *d)))
            .collect();
        max_inline_widths(&items, short)
    } else {
        (0, 0, 0, 0)
    };

    if !inline_tree {
        if compact {
            let (change_w, file_w) = col_widths_compact();
            let _ = writeln!(out, " {:<name_w$}{:>cw$}{:>fw$}",
                "Language".bold(), "Changes".bold(), "Files".bold(),
                name_w = NAME_WIDTH, cw = change_w, fw = file_w);
        } else {
            let _ = writeln!(out, "{}", format_header_3(["Language", "Additions", "Deletions", "Files"]));
        }
    }
    let _ = writeln!(out, "{}", heavy(LINE_WIDTH).bold());

    for period in stats {
        let _ = writeln!(out, " {} ({})",
            period.period_label.bright_blue().bold(),
            format!("{} commits", period.total_commits).dimmed(),
        );

        let mut langs: Vec<_> = period.by_language.iter().collect();
        match sort.unwrap_or(&SortBy::Additions) {
            SortBy::Additions | SortBy::Commits => langs.sort_by(|a, b| b.1.additions.cmp(&a.1.additions)),
            SortBy::Deletions => langs.sort_by(|a, b| b.1.deletions.cmp(&a.1.deletions)),
            SortBy::Files => langs.sort_by(|a, b| b.1.files_changed.cmp(&a.1.files_changed)),
            SortBy::Name => langs.sort_by(|a, b| a.0.cmp(b.0)),
        }

        if inline_tree {
            let pad = " ".repeat(NAME_WIDTH + 1);
            for (i, (lang, ls)) in langs.iter().enumerate() {
                let prefix = if i == langs.len() - 1 { "└── " } else { "├── " };
                let entry = format_inline_entry(prefix, lang, ls.additions, ls.deletions, short, il_lang_w, il_net_w, il_add_w, il_del_w);
                let _ = writeln!(out, "{}{}", pad, entry);
            }
        } else if compact {
            let (change_w, file_w) = col_widths_compact();
            for (i, (lang, ls)) in langs.iter().enumerate() {
                let prefix = if i == langs.len() - 1 { "└── " } else { "├── " };
                let fs = format!("{:>w$}", format_num(ls.files_changed, short), w = file_w);
                let _ = writeln!(out, " {:<nw$}{}{}",
                    format!("{prefix}{lang}").cyan(),
                    format_changes_aligned(ls.additions, ls.deletions, change_w, short, false, period_add_w, period_del_w),
                    fs.yellow(),
                    nw = NAME_WIDTH);
            }
        } else {
            for (i, (lang, ls)) in langs.iter().enumerate() {
                let prefix = if i == langs.len() - 1 { "└── " } else { "├── " };
                let _ = writeln!(out, "{}", format_row_3_colored(
                    &format!("{prefix}{lang}"),
                    [&format_num(ls.additions, short), &format_num(ls.deletions, short), &format_num(ls.files_changed, short)],
                    0, true,
                ));
            }
        }
    }

    let _ = writeln!(out, "{}", heavy(LINE_WIDTH).bold());
    let total_commits: u64 = stats.iter().map(|p| p.total_commits).sum();
    let _ = writeln!(out, " {} ({})",
        "Total".bold(),
        format!("{total_commits} commits").dimmed(),
    );

    if inline_tree {
        let pad = " ".repeat(NAME_WIDTH + 1);
        for (i, (lang, a, d, _f)) in total_langs.iter().enumerate() {
            let prefix = if i == total_langs.len() - 1 { "└── " } else { "├── " };
            let entry = format_inline_entry(prefix, lang, *a, *d, short, il_lang_w, il_net_w, il_add_w, il_del_w);
            let _ = writeln!(out, "{}{}", pad, entry);
        }
    } else if compact {
        let (change_w, file_w) = col_widths_compact();
        for (i, (lang, a, d, f)) in total_langs.iter().enumerate() {
            let prefix = if i == total_langs.len() - 1 { "└── " } else { "├── " };
            let fs = format!("{:>w$}", format_num(*f, short), w = file_w);
            let _ = writeln!(out, " {:<nw$}{}{}",
                format!("{prefix}{lang}").cyan(),
                format_changes_aligned(*a, *d, change_w, short, false, period_add_w, period_del_w),
                fs.yellow(),
                nw = NAME_WIDTH);
        }
    } else {
        for (i, (lang, a, d, f)) in total_langs.iter().enumerate() {
            let prefix = if i == total_langs.len() - 1 { "└── " } else { "├── " };
            let _ = writeln!(out, "{}", format_row_3_colored(
                &format!("{prefix}{lang}"),
                [&format_num(*a, short), &format_num(*d, short), &format_num(*f, short)],
                0, true,
            ));
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
    short: bool,
    compact: bool,
    inline_tree: bool,
) -> String {
    if stats.is_empty() {
        return "No data to display".to_string();
    }

    match group_by {
        GroupBy::Language => render_language_table(stats, totals, sort, short, compact),
        GroupBy::Author => render_author_table(stats, totals, email_display, dedup, identity_map, sort, short, compact, inline_tree),
        GroupBy::Period => render_period_table(stats, totals, sort, short, compact, inline_tree),
        GroupBy::Repo => render_period_table(stats, totals, sort, short, compact, inline_tree),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_group_tree(
    nodes: &[GroupNode],
    leaf_group: &GroupBy,
    sort: Option<&SortBy>,
    short: bool,
    compact: bool,
    inline_tree: bool,
) -> String {
    render_group_tree_inner(
        nodes,
        leaf_group,
        sort,
        short,
        compact,
        inline_tree,
        0,
    )
}

fn render_group_tree_inner(
    nodes: &[GroupNode],
    leaf_group: &GroupBy,
    sort: Option<&SortBy>,
    short: bool,
    compact: bool,
    inline_tree: bool,
    depth: usize,
) -> String {
    if nodes.is_empty() {
        return "No data to display".to_string();
    }

    let all_leaves = nodes.iter().all(|n| n.children.is_empty());

    if all_leaves {
        let rows: Vec<PeriodStats> = nodes.iter().map(|n| n.stats.clone()).collect();
        let totals = crate::stats::aggregator::aggregate_totals(&rows);
        return match leaf_group {
            GroupBy::Language => render_language_table(&rows, &totals, sort, short, compact),
            GroupBy::Author | GroupBy::Period | GroupBy::Repo => {
                render_period_table(&rows, &totals, sort, short, compact, inline_tree)
            }
        };
    }

    let mut out = String::new();
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 && !compact {
            out.push('\n');
        }

        let indent = "  ".repeat(depth);
        let add_str = format!("+{}", format_num(node.stats.total_additions, short));
        let del_str = format!("-{}", format_num(node.stats.total_deletions, short));
        let _ = writeln!(
            out,
            "\n{}━━ {} ({} commits, {}/{})",
            indent,
            node.label.bright_blue().bold(),
            node.stats.total_commits,
            add_str.green(),
            del_str.red(),
        );

        let child_out = render_group_tree_inner(
            &node.children,
            leaf_group,
            sort,
            short,
            compact,
            inline_tree,
            depth + 1,
        );
        out.push_str(&child_out);
    }

    if depth == 0 {
        let grand_total = crate::stats::aggregator::aggregate_totals(
            &nodes.iter().map(|n| n.stats.clone()).collect::<Vec<_>>(),
        );
        let add_str = format!("+{}", format_num(grand_total.total_additions, short));
        let del_str = format!("-{}", format_num(grand_total.total_deletions, short));
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", "━".repeat(LINE_WIDTH));
        let _ = writeln!(
            out,
            "  Grand Total: {} commits, {}/{}",
            grand_total.total_commits,
            add_str.green(),
            del_str.red(),
        );
    }

    out
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

    use crate::stats::models::{AuthorStats, LangStats};

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
        }
    }

    fn make_period_with_authors(
        label: &str,
        langs: Vec<(&str, u64, u64, u64)>,
        authors: Vec<(&str, u64, u64, u64, Vec<(&str, u64, u64, u64)>)>,
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

        let output = render_stats_table(&[period], &totals, &GroupBy::Language, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false);

        assert!(output.contains("━"));
        assert!(output.contains("Language"));
        assert!(output.contains("Additions"));
        assert!(output.contains("Deletions"));
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

        let output = render_stats_table(&[period], &totals, &GroupBy::Author, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false);

        assert!(output.contains("━"));
        assert!(output.contains("Author"));
        assert!(output.contains("Commits"));
        assert!(output.contains("Additions"));
        assert!(output.contains("Deletions"));
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

        let output = render_stats_table(&[period], &totals, &GroupBy::Author, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false);

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

        let output = render_stats_table(&[p1, p2], &totals, &GroupBy::Period, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false);

        assert!(output.contains("━"));
        assert!(output.contains("─"));
        assert!(output.contains("2025-01"));
        assert!(output.contains("5 commits"));
        assert!(output.contains("2025-02"));
        assert!(output.contains("3 commits"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Go"));
        assert!(output.contains("Total"));
        assert!(output.contains("8 commits"));
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

        let output = render_stats_table(
            &[p1, p2],
            &totals,
            &GroupBy::Repo,
            &EmailDisplay::None,
            &DedupMode::Name,
            &HashMap::new(),
            None,
            false,
            false,
            false,
        );

        assert!(output.contains("owner/repo-a"));
        assert!(output.contains("owner/repo-b"));
        assert!(output.contains("5 commits"));
        assert!(output.contains("3 commits"));
        assert!(output.contains("Rust"));
        assert!(output.contains("Go"));
        assert!(output.contains("Total"));
        assert!(output.contains("8 commits"));
    }

    #[test]
    fn test_empty_returns_no_data() {
        let totals = make_period("Total", vec![], 0);

        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Language, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false),
            "No data to display"
        );
        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Author, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false),
            "No data to display"
        );
        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Period, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false),
            "No data to display"
        );
        assert_eq!(
            render_stats_table(&[], &totals, &GroupBy::Repo, &EmailDisplay::None, &DedupMode::Name, &HashMap::new(), None, false, false, false),
            "No data to display"
        );
    }

    #[test]
    fn test_short_numbers() {
        assert_eq!(format_num(999, true), "999");
        assert_eq!(format_num(1000, true), "1.0k");
        assert_eq!(format_num(1500, true), "1.5k");
        assert_eq!(format_num(1_000_000, true), "1.0M");
        assert_eq!(format_num(2_500_000, true), "2.5M");
        assert_eq!(format_num(42, false), "42");
        assert_eq!(format_num(1500, false), "1500");
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
