use serde::Serialize;
use tera::{Context, Tera};

use crate::cli::NumberFormat;
use crate::github::api::GithubUser;
use crate::github::ContributionSummary;
use crate::output::table::format_num;
use crate::stats::models::PeriodStats;

const TEMPLATE: &str = include_str!("../templates/profile_card.svg");
const MULTI_TEMPLATE: &str = include_str!("../templates/multi_card.svg");

const LANG_COLORS: &[(&str, &str)] = &[
    ("Rust", "#dea584"),
    ("Go", "#00ADD8"),
    ("Python", "#3572A5"),
    ("JavaScript", "#f1e05a"),
    ("TypeScript", "#3178c6"),
    ("Java", "#b07219"),
    ("C", "#555555"),
    ("C++", "#f34b7d"),
    ("C#", "#178600"),
    ("Ruby", "#701516"),
    ("PHP", "#4F5D95"),
    ("Swift", "#F05138"),
    ("Kotlin", "#A97BFF"),
    ("Dart", "#00B4AB"),
    ("Scala", "#c22d40"),
    ("Shell", "#89e051"),
    ("Lua", "#000080"),
    ("HTML", "#e34c26"),
    ("CSS", "#563d7c"),
    ("Vue", "#41b883"),
    ("Svelte", "#ff3e00"),
    ("Zig", "#ec915c"),
    ("Nix", "#7e7eff"),
    ("Dockerfile", "#384d54"),
    ("Makefile", "#427819"),
    ("YAML", "#cb171e"),
    ("TOML", "#9c4221"),
    ("Markdown", "#083fa1"),
];

const ICON_COMMITS: &str = "M10.5 7.75a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0Zm1.43.75a4.002 4.002 0 0 1-7.86 0H.75a.75.75 0 1 1 0-1.5h3.32a4.001 4.001 0 0 1 7.86 0h3.32a.75.75 0 1 1 0 1.5h-3.32Z";
const ICON_REPOS: &str = "M2 2.5A2.5 2.5 0 0 1 4.5 0h8.75a.75.75 0 0 1 .75.75v12.5a.75.75 0 0 1-.75.75h-2.5a.75.75 0 0 1 0-1.5h1.75v-2h-8a1 1 0 0 0-.714 1.7.75.75 0 1 1-1.072 1.05A2.495 2.495 0 0 1 2 11.5Zm10.5-1h-8a1 1 0 0 0-1 1v6.708A2.486 2.486 0 0 1 4.5 9h8ZM5 12.25a.25.25 0 0 1 .25-.25h3.5a.25.25 0 0 1 .25.25v3.25a.25.25 0 0 1-.4.2l-1.45-1.087a.25.25 0 0 1-.3 0L5.4 15.7a.25.25 0 0 1-.4-.2Z";
const ICON_PLUS: &str = "M7.75 2a.75.75 0 0 1 .75.75V7h4.25a.75.75 0 0 1 0 1.5H8.5v4.25a.75.75 0 0 1-1.5 0V8.5H2.75a.75.75 0 0 1 0-1.5H7V2.75A.75.75 0 0 1 7.75 2Z";
const ICON_MINUS: &str = "M2.75 7.75h10.5a.75.75 0 0 1 0 1.5H2.75a.75.75 0 0 1 0-1.5Z";
const ICON_PR: &str = "M1.5 3.25a2.25 2.25 0 1 1 3 2.122v5.256a2.251 2.251 0 1 1-1.5 0V5.372A2.25 2.25 0 0 1 1.5 3.25Zm5.677-.177L9.573.677A.25.25 0 0 1 10 .854V2.5h1A2.5 2.5 0 0 1 13.5 5v5.628a2.251 2.251 0 1 1-1.5 0V5a1 1 0 0 0-1-1h-1v1.646a.25.25 0 0 1-.427.177L7.177 3.427a.25.25 0 0 1 0-.354ZM3.75 2.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm0 9.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Zm8.25.75a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Z";
const ICON_ISSUE: &str = "M8 9.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0ZM1.5 8a6.5 6.5 0 1 0 13 0 6.5 6.5 0 0 0-13 0Z";
const ICON_NET_CHANGE: &str = "M8.235.044a.666.666 0 0 0-.47 0l-7.07 2.886a.333.333 0 0 0 .07.633l2.902.875-.404 3.152a.333.333 0 0 0 .467.343L8 5.869l4.27 2.064a.333.333 0 0 0 .467-.343l-.404-3.152 2.902-.875a.333.333 0 0 0 .07-.633Z";

const ICON_CALENDAR: &str = "M4.75 0a.75.75 0 0 1 .75.75V2h5V.75a.75.75 0 0 1 1.5 0V2h1.25c.966 0 1.75.784 1.75 1.75v10.5A1.75 1.75 0 0 1 13.25 16H2.75A1.75 1.75 0 0 1 1 14.25V3.75C1 2.784 1.784 2 2.75 2H4V.75A.75.75 0 0 1 4.75 0ZM2.5 7.5v6.75c0 .138.112.25.25.25h10.5a.25.25 0 0 0 .25-.25V7.5Z";

fn lang_color(name: &str) -> &'static str {
    LANG_COLORS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, c)| *c)
        .unwrap_or("#858585")
}

#[derive(Serialize)]
struct LangBar {
    name: String,
    pct: String,
    color: String,
    bar_x: f64,
    bar_w: f64,
    dot_cx: usize,
    dot_cy: usize,
    text_x: usize,
    text_y: usize,
    pct_x: usize,
}

#[derive(Serialize)]
struct StatItem {
    icon: String,
    label: String,
    value: String,
    value_class: String,
    value_x: usize,
    x: usize,
    y: usize,
}

pub struct MultiColumnData {
    pub days: u64,
    pub stats: PeriodStats,
    pub active_repos: usize,
}

#[derive(Serialize)]
struct MultiColumn {
    title: String,
    title_x: usize,
    divider_x: usize,
    stat_items: Vec<StatItem>,
    bar_x: usize,
    bar_y: usize,
    bar_width: usize,
    languages: Vec<LangBar>,
}

#[allow(clippy::too_many_arguments)]
pub fn render_profile_card(
    username: &str,
    user: &GithubUser,
    stats: Option<&PeriodStats>,
    active_repos: usize,
    summary: &ContributionSummary,
    days: u64,
    short: bool,
    num_fmt: NumberFormat,
    num_fmt_lines: Option<NumberFormat>,
    lang_rows: usize,
    custom_title: Option<&str>,
) -> anyhow::Result<String> {
    let mut tera = Tera::default();
    tera.add_raw_template("card", TEMPLATE)?;

    let total_commits = stats.map_or(0, |s| s.total_commits);
    let total_additions = stats.map_or(0, |s| s.total_additions);
    let total_deletions = stats.map_or(0, |s| s.total_deletions);
    let total_net_additions = stats.map_or(0, |s| s.total_net_additions);

    let card_width: usize = if short { 495 } else { 700 };
    let bar_width: usize = card_width - 50;

    let stat_items = build_stat_items(
        user,
        total_commits,
        total_additions,
        total_deletions,
        total_net_additions,
        active_repos,
        summary,
        short,
        num_fmt,
        num_fmt_lines,
        card_width,
    );

    let last_stat_y = 62 + 2 * 26;
    let divider_y = last_stat_y + 15;
    let lang_title_y = divider_y + 20;
    let lang_bar_y = lang_title_y + 15;
    let legend_start_y = lang_bar_y + 26;

    let lang_col_w = bar_width / 3;
    let max_langs = lang_rows * 3;
    let languages = build_lang_bars(stats, lang_bar_y, legend_start_y, lang_col_w, max_langs);
    let legend_rows = languages.len().div_ceil(3);
    let card_height = if languages.is_empty() {
        last_stat_y + 25
    } else {
        legend_start_y + (legend_rows - 1) * 22 + 15
    };

    let title_period = custom_title
        .map(|title| title.to_string())
        .unwrap_or_else(|| format!("Recent {days} days"));

    let mut ctx = Context::new();
    ctx.insert("username", username);
    ctx.insert("title_period", &title_period);
    ctx.insert("stat_items", &stat_items);
    ctx.insert("languages", &languages);
    ctx.insert("card_width", &card_width);
    ctx.insert("card_height", &card_height);
    ctx.insert("bar_width", &bar_width);
    ctx.insert("divider_y", &divider_y);
    ctx.insert("lang_title_y", &lang_title_y);
    ctx.insert("lang_bar_y", &lang_bar_y);

    Ok(tera.render("card", &ctx)?)
}

pub fn render_multi_card(
    data: &[MultiColumnData],
    num_fmt: NumberFormat,
    num_fmt_lines: Option<NumberFormat>,
) -> anyhow::Result<String> {
    let mut tera = Tera::default();
    tera.add_raw_template("multi", MULTI_TEMPLATE)?;

    let margin = 25usize;
    let col_width = 200usize;
    let col_pad = 10usize;
    let card_width = 2 * margin + data.len() * col_width;

    let stat_row_start = 40usize;
    let stat_row_spacing = 24usize;
    let value_x = 115usize;

    let bar_y = 160usize;
    let legend_start_y = bar_y + 24;
    let lang_spacing = 20usize;
    let max_langs = 3usize;

    let line_fmt = num_fmt_lines.unwrap_or(num_fmt);

    let mut columns: Vec<MultiColumn> = Vec::new();

    for (i, col_data) in data.iter().enumerate() {
        let col_start = margin + i * col_width;
        let stat_x = col_start + col_pad;

        let stats = &col_data.stats;
        let total_commits = stats.total_commits;
        let total_additions = stats.total_additions;
        let total_deletions = stats.total_deletions;
        let total_net_additions = stats.total_net_additions;

        let stat_items = vec![
            StatItem {
                icon: ICON_REPOS.to_string(),
                label: "Repos:".to_string(),
                value: format_num(col_data.active_repos as u64, num_fmt),
                value_class: "stat-val".to_string(),
                value_x,
                x: stat_x,
                y: stat_row_start,
            },
            StatItem {
                icon: ICON_COMMITS.to_string(),
                label: "Commits:".to_string(),
                value: format_num(total_commits, num_fmt),
                value_class: "stat-val".to_string(),
                value_x,
                x: stat_x,
                y: stat_row_start + stat_row_spacing,
            },
            StatItem {
                icon: ICON_PLUS.to_string(),
                label: "Added:".to_string(),
                value: format!("+{}", format_num(total_additions, line_fmt)),
                value_class: "stat-add".to_string(),
                value_x,
                x: stat_x,
                y: stat_row_start + 2 * stat_row_spacing,
            },
            StatItem {
                icon: ICON_MINUS.to_string(),
                label: "Deleted:".to_string(),
                value: format!("-{}", format_num(total_deletions, line_fmt)),
                value_class: "stat-del".to_string(),
                value_x,
                x: stat_x,
                y: stat_row_start + 3 * stat_row_spacing,
            },
            StatItem {
                icon: ICON_NET_CHANGE.to_string(),
                label: "Net Adds:".to_string(),
                value: format!("+{}", format_num(total_net_additions, line_fmt)),
                value_class: "stat-add".to_string(),
                value_x,
                x: stat_x,
                y: stat_row_start + 4 * stat_row_spacing,
            },
        ];

        let bar_x = col_start + col_pad;
        let bar_width = col_width - 2 * col_pad;

        let languages = build_multi_lang_bars(
            Some(stats),
            bar_x,
            bar_y,
            bar_width,
            legend_start_y,
            col_start + col_pad,
            col_start + col_pad + 110,
            max_langs,
            lang_spacing,
        );

        columns.push(MultiColumn {
            title: format!("Recent {}d", col_data.days),
            title_x: col_start + col_width / 2,
            divider_x: col_start,
            stat_items,
            bar_x,
            bar_y,
            bar_width,
            languages,
        });
    }

    let has_any_langs = columns.iter().any(|c| !c.languages.is_empty());
    let card_height = if has_any_langs {
        legend_start_y + (max_langs - 1) * lang_spacing + 15
    } else {
        stat_row_start + 4 * stat_row_spacing + 15
    };

    let mut ctx = Context::new();
    ctx.insert("card_width", &card_width);
    ctx.insert("card_height", &card_height);
    ctx.insert("columns", &columns);

    Ok(tera.render("multi", &ctx)?)
}

#[allow(clippy::too_many_arguments)]
fn build_multi_lang_bars(
    stats: Option<&PeriodStats>,
    bar_x: usize,
    _bar_y: usize,
    bar_width: usize,
    legend_start_y: usize,
    legend_x: usize,
    pct_x: usize,
    max_langs: usize,
    lang_spacing: usize,
) -> Vec<LangBar> {
    let Some(stats) = stats else {
        return Vec::new();
    };

    let mut langs: Vec<(&String, u64)> = stats
        .by_language
        .iter()
        .map(|(name, ls)| (name, ls.additions))
        .filter(|(_, total)| *total > 0)
        .collect();

    langs.sort_by(|a, b| b.1.cmp(&a.1));

    let total: u64 = langs.iter().map(|(_, v)| v).sum();
    if total == 0 {
        return Vec::new();
    }

    let top: Vec<_> = langs.into_iter().take(max_langs).collect();

    let mut x = bar_x as f64;
    let mut bars = Vec::new();

    for (i, (name, val)) in top.iter().enumerate() {
        let pct = *val as f64 / total as f64 * 100.0;
        let w = (pct / 100.0 * bar_width as f64).max(2.0);

        let dot_cx = legend_x + 5;
        bars.push(LangBar {
            name: (*name).clone(),
            pct: format!("{pct:.1}"),
            color: lang_color(name).to_string(),
            bar_x: x,
            bar_w: w,
            dot_cx,
            dot_cy: legend_start_y + i * lang_spacing - 4,
            text_x: dot_cx + 12,
            text_y: legend_start_y + i * lang_spacing,
            pct_x,
        });
        x += w;
    }

    bars
}

#[allow(clippy::too_many_arguments)]
fn build_stat_items(
    user: &GithubUser,
    total_commits: u64,
    total_additions: u64,
    total_deletions: u64,
    total_net_additions: u64,
    active_repos: usize,
    summary: &ContributionSummary,
    short: bool,
    num_fmt: NumberFormat,
    num_fmt_lines: Option<NumberFormat>,
    card_width: usize,
) -> Vec<StatItem> {
    let mut items = Vec::new();
    let member_since = user
        .created_at
        .split('-')
        .next()
        .unwrap_or("Unknown")
        .to_string();

    let row_start = 62usize;
    let row_spacing = 26usize;
    let margin = 25usize;
    let col_width = (card_width - 2 * margin) / if short { 2 } else { 3 };
    let col_x: [usize; 3] = [margin, margin + col_width, margin + 2 * col_width];
    let value_x: usize = 130;
    let line_fmt = num_fmt_lines.unwrap_or(num_fmt);

    let mut push =
        |col: usize, row: usize, icon: &str, label: &str, value: String, value_class: &str| {
            items.push(StatItem {
                icon: icon.to_string(),
                label: label.to_string(),
                value,
                value_class: value_class.to_string(),
                value_x,
                x: col_x[col],
                y: row_start + row * row_spacing,
            });
        };

    if short {
        push(
            0,
            0,
            ICON_COMMITS,
            "Total Commits:",
            format_num(total_commits, num_fmt),
            "stat-val",
        );
        push(
            1,
            0,
            ICON_REPOS,
            "Active Repos:",
            format_num(active_repos as u64, num_fmt),
            "stat-val",
        );

        push(
            0,
            1,
            ICON_PLUS,
            "Lines Added:",
            format!("+{}", format_num(total_additions, line_fmt)),
            "stat-add",
        );
        push(
            1,
            1,
            ICON_MINUS,
            "Lines Deleted:",
            format!("-{}", format_num(total_deletions, line_fmt)),
            "stat-del",
        );

        push(
            0,
            2,
            ICON_PR,
            "Pull Requests:",
            format_num(summary.total_prs as u64, num_fmt),
            "stat-val",
        );
        push(
            1,
            2,
            ICON_ISSUE,
            "Issues:",
            format_num(summary.total_issues as u64, num_fmt),
            "stat-val",
        );

        return items;
    }

    let net_adds_value = format!("+{}", format_num(total_net_additions, line_fmt));

    push(
        0,
        0,
        ICON_COMMITS,
        "Total Commits:",
        format_num(total_commits, num_fmt),
        "stat-val",
    );
    push(
        1,
        0,
        ICON_REPOS,
        "Active Repos:",
        format_num(active_repos as u64, num_fmt),
        "stat-val",
    );
    push(
        2,
        0,
        ICON_PR,
        "Pull Requests:",
        format_num(summary.total_prs as u64, num_fmt),
        "stat-val",
    );

    push(
        0,
        1,
        ICON_PLUS,
        "Lines Added:",
        format!("+{}", format_num(total_additions, line_fmt)),
        "stat-add",
    );
    push(
        1,
        1,
        ICON_MINUS,
        "Lines Deleted:",
        format!("-{}", format_num(total_deletions, line_fmt)),
        "stat-del",
    );
    push(
        2,
        1,
        ICON_NET_CHANGE,
        "Net Adds:",
        net_adds_value,
        "stat-add",
    );

    push(
        0,
        2,
        ICON_PR,
        "PR Reviews:",
        format_num(summary.total_reviews as u64, num_fmt),
        "stat-val",
    );
    push(
        1,
        2,
        ICON_ISSUE,
        "Issues:",
        format_num(summary.total_issues as u64, num_fmt),
        "stat-val",
    );
    push(
        2,
        2,
        ICON_CALENDAR,
        "Member Since:",
        member_since,
        "stat-val",
    );

    items
}

fn build_lang_bars(
    stats: Option<&PeriodStats>,
    _bar_y: usize,
    legend_start_y: usize,
    lang_col_w: usize,
    max_langs: usize,
) -> Vec<LangBar> {
    let Some(stats) = stats else {
        return Vec::new();
    };

    let mut langs: Vec<(&String, u64)> = stats
        .by_language
        .iter()
        .map(|(name, ls)| (name, ls.additions))
        .filter(|(_, total)| *total > 0)
        .collect();

    langs.sort_by(|a, b| b.1.cmp(&a.1));

    let total: u64 = langs.iter().map(|(_, v)| v).sum();
    if total == 0 {
        return Vec::new();
    }

    let top: Vec<_> = langs.into_iter().take(max_langs).collect();

    let margin = 25usize;
    let bar_total = (lang_col_w * 3) as f64;
    let mut x = margin as f64;
    let mut bars = Vec::new();
    let cols = 3;
    let rows = top.len().div_ceil(cols);

    for (i, (name, val)) in top.iter().enumerate() {
        let pct = *val as f64 / total as f64 * 100.0;
        let w = (pct / 100.0 * bar_total).max(2.0);
        let col = i / rows;
        let row = i % rows;

        let dot_cx = margin + 5 + col * lang_col_w;
        bars.push(LangBar {
            name: (*name).clone(),
            pct: format!("{pct:.1}"),
            color: lang_color(name).to_string(),
            bar_x: x,
            bar_w: w,
            dot_cx,
            dot_cy: legend_start_y + row * 22 - 4,
            text_x: dot_cx + 12,
            text_y: legend_start_y + row * 22,
            pct_x: dot_cx + lang_col_w / 2,
        });
        x += w;
    }

    bars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::models::LangStats;
    use std::collections::HashMap;

    fn make_user(public_repos: u64) -> GithubUser {
        GithubUser {
            login: "octocat".to_string(),
            name: Some("The Octocat".to_string()),
            bio: None,
            public_repos,
            followers: 100,
            following: 10,
            avatar_url: "https://avatars.githubusercontent.com/u/583231".to_string(),
            html_url: "https://github.com/octocat".to_string(),
            created_at: "2011-01-25T18:44:36Z".to_string(),
            node_id: "MDQ6VXNlcjU4MzIzMQ==".to_string(),
        }
    }

    fn make_stats() -> PeriodStats {
        let mut by_language = HashMap::new();
        by_language.insert(
            "Rust".to_string(),
            LangStats {
                additions: 500,
                deletions: 100,
                files_changed: 20,
                net_modifications: 500,
                net_additions: 400,
            },
        );
        by_language.insert(
            "Python".to_string(),
            LangStats {
                additions: 200,
                deletions: 50,
                files_changed: 10,
                net_modifications: 200,
                net_additions: 150,
            },
        );

        PeriodStats {
            period_label: "2025-W03".to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: 42,
            total_additions: 700,
            total_deletions: 150,
            total_net_modifications: 700,
            total_net_additions: 550,
        }
    }

    #[test]
    fn render_with_known_data() {
        let user = make_user(8);
        let stats = make_stats();
        let summary = ContributionSummary {
            total_prs: 9,
            total_reviews: 7,
            total_issues: 5,
        };

        let svg = render_profile_card(
            "octocat",
            &user,
            Some(&stats),
            5,
            &summary,
            30,
            false,
            NumberFormat::Plain,
            None,
            2,
            None,
        )
        .unwrap();

        assert!(svg.contains("octocat&#x27;s Git Stats") || svg.contains("octocat's Git Stats"));
        assert!(svg.contains("Recent 30 days"));
        assert!(svg.contains("Top Languages (Recent 30 days)"));
        assert!(svg.contains(">42<"));
        assert!(svg.contains(">+700<"));
        assert!(svg.contains(">-150<"));
        assert!(svg.contains(">+550<"));
        assert!(svg.contains("Rust"));
        assert!(svg.contains("Python"));
        assert_eq!(svg.matches("class=\"stat-label\"").count(), 9);
    }

    #[test]
    fn output_is_valid_svg() {
        let user = make_user(5);
        let summary = ContributionSummary::default();
        let svg = render_profile_card(
            "testuser",
            &user,
            None,
            5,
            &summary,
            30,
            false,
            NumberFormat::Plain,
            None,
            2,
            None,
        )
        .unwrap();
        let trimmed = svg.trim();

        assert!(trimmed.starts_with("<svg"));
        assert!(trimmed.ends_with("</svg>"));
    }

    #[test]
    fn render_without_stats() {
        let user = make_user(3);
        let summary = ContributionSummary::default();
        let svg = render_profile_card(
            "ghostuser",
            &user,
            None,
            5,
            &summary,
            30,
            false,
            NumberFormat::Plain,
            None,
            2,
            None,
        )
        .unwrap();

        assert!(
            svg.contains("ghostuser&#x27;s Git Stats") || svg.contains("ghostuser's Git Stats")
        );
        assert!(svg.contains("Recent 30 days"));
        assert!(svg.contains(">0<"));
    }

    #[test]
    fn short_mode_renders_eight_stats() {
        let user = make_user(3);
        let summary = ContributionSummary {
            total_prs: 2,
            total_reviews: 1,
            total_issues: 4,
        };
        let svg = render_profile_card(
            "ghostuser",
            &user,
            None,
            2,
            &summary,
            30,
            true,
            NumberFormat::Plain,
            None,
            2,
            None,
        )
        .unwrap();

        assert_eq!(svg.matches("class=\"stat-label\"").count(), 6);
        assert!(svg.contains("Pull Requests:"));
        assert!(!svg.contains("Net Adds:"));
    }

    #[test]
    fn lang_bars_sorted_by_total() {
        let mut by_language = HashMap::new();
        by_language.insert(
            "Go".to_string(),
            LangStats {
                additions: 10,
                deletions: 5,
                files_changed: 2,
                ..Default::default()
            },
        );
        by_language.insert(
            "TypeScript".to_string(),
            LangStats {
                additions: 999,
                deletions: 1,
                files_changed: 50,
                ..Default::default()
            },
        );
        by_language.insert(
            "Rust".to_string(),
            LangStats {
                additions: 500,
                deletions: 100,
                files_changed: 20,
                ..Default::default()
            },
        );
        by_language.insert(
            "Python".to_string(),
            LangStats {
                additions: 300,
                deletions: 50,
                files_changed: 20,
                ..Default::default()
            },
        );
        by_language.insert(
            "Java".to_string(),
            LangStats {
                additions: 200,
                deletions: 10,
                files_changed: 20,
                ..Default::default()
            },
        );

        let stats = PeriodStats {
            period_label: "test".to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: 0,
            total_additions: 0,
            total_deletions: 0,
            total_net_modifications: 0,
            total_net_additions: 0,
        };

        let lang_col_w = 148;
        let bars = build_lang_bars(Some(&stats), 178, 202, lang_col_w, 6);
        assert_eq!(bars.len(), 5);
        // Sorted by additions only: TypeScript(999) > Rust(500) > Python(300) > Java(200) > Go(10)
        assert_eq!(bars[0].name, "TypeScript");
        assert_eq!(bars[1].name, "Rust");
        assert_eq!(bars[2].name, "Python");
        assert_eq!(bars[3].name, "Java");
        assert_eq!(bars[4].name, "Go");

        let rows = bars.len().div_ceil(3);
        for (i, bar) in bars.iter().enumerate() {
            let col = i / rows;
            let row = i % rows;
            let dot_cx = 30 + col * lang_col_w;
            assert_eq!(bar.dot_cx, dot_cx);
            assert_eq!(bar.dot_cy, 202 + row * 22 - 4);
            assert_eq!(bar.text_x, dot_cx + 12);
            assert_eq!(bar.text_y, 202 + row * 22);
            assert_eq!(bar.pct_x, dot_cx + lang_col_w / 2);
        }
    }

    #[test]
    fn render_with_custom_title() {
        let user = make_user(8);
        let stats = make_stats();
        let summary = ContributionSummary::default();

        let svg = render_profile_card(
            "octocat",
            &user,
            Some(&stats),
            5,
            &summary,
            30,
            false,
            NumberFormat::Plain,
            None,
            2,
            Some("Custom"),
        )
        .unwrap();

        assert!(svg.contains("Custom"));
        assert!(!svg.contains("Recent 30 days"));
    }
}
