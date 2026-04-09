use serde::Serialize;
use tera::{Context, Tera};

use crate::github::api::GithubUser;
use crate::stats::models::PeriodStats;

const TEMPLATE: &str = include_str!("../templates/profile_card.svg");

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

pub fn render_profile_card(
    username: &str,
    user: &GithubUser,
    stats: Option<&PeriodStats>,
) -> anyhow::Result<String> {
    let mut tera = Tera::default();
    tera.add_raw_template("card", TEMPLATE)?;
    let mut ctx = Context::new();
    ctx.insert("username", username);
    ctx.insert("total_commits", &stats.map_or(0, |s| s.total_commits));
    ctx.insert("total_additions", &stats.map_or(0, |s| s.total_additions));
    ctx.insert("total_deletions", &stats.map_or(0, |s| s.total_deletions));
    ctx.insert("public_repos", &user.public_repos);

    let languages = build_lang_bars(stats);
    let lang_rows = languages.len().div_ceil(3);
    let card_height = if languages.is_empty() {
        115
    } else {
        170 + lang_rows * 20 + 15
    };
    ctx.insert("languages", &languages);
    ctx.insert("card_height", &card_height);

    Ok(tera.render("card", &ctx)?)
}

fn build_lang_bars(stats: Option<&PeriodStats>) -> Vec<LangBar> {
    let Some(stats) = stats else {
        return Vec::new();
    };

    let mut langs: Vec<(&String, u64)> = stats
        .by_language
        .iter()
        .map(|(name, ls)| (name, ls.additions + ls.deletions))
        .filter(|(_, total)| *total > 0)
        .collect();

    langs.sort_by(|a, b| b.1.cmp(&a.1));

    let total: u64 = langs.iter().map(|(_, v)| v).sum();
    if total == 0 {
        return Vec::new();
    }

    let max_langs = 9;
    let top: Vec<_> = langs.into_iter().take(max_langs).collect();

    let bar_total = 445.0;
    let mut x = 25.0;
    let mut bars = Vec::new();

    for (i, (name, val)) in top.iter().enumerate() {
        let pct = *val as f64 / total as f64 * 100.0;
        let w = (pct / 100.0 * bar_total).max(2.0);
        let col = i % 3;
        let row = i / 3;
        bars.push(LangBar {
            name: (*name).clone(),
            pct: format!("{:.1}", pct),
            color: lang_color(name).to_string(),
            bar_x: x,
            bar_w: w,
            dot_cx: 30 + col * 155,
            dot_cy: 170 + row * 20,
            text_x: 38 + col * 155,
            text_y: 174 + row * 20,
            pct_x: 110 + col * 155,
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
            },
        );
        by_language.insert(
            "Python".to_string(),
            LangStats {
                additions: 200,
                deletions: 50,
                files_changed: 10,
            },
        );

        PeriodStats {
            period_label: "2025-W03".to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: 42,
            total_additions: 700,
            total_deletions: 150,
        }
    }

    #[test]
    fn render_with_known_data() {
        let user = make_user(8);
        let stats = make_stats();
        let svg = render_profile_card("octocat", &user, Some(&stats)).unwrap();

        assert!(svg.contains("octocat&#x27;s Git Stats") || svg.contains("octocat's Git Stats"));
        assert!(svg.contains(">42<"));
        assert!(svg.contains(">+700<"));
        assert!(svg.contains(">-150<"));
        assert!(svg.contains("Rust"));
        assert!(svg.contains("Python"));
        assert!(svg.contains(">8<"));
    }

    #[test]
    fn output_is_valid_svg() {
        let user = make_user(5);
        let svg = render_profile_card("testuser", &user, None).unwrap();
        let trimmed = svg.trim();

        assert!(trimmed.starts_with("<svg"));
        assert!(trimmed.ends_with("</svg>"));
    }

    #[test]
    fn render_without_stats() {
        let user = make_user(3);
        let svg = render_profile_card("ghostuser", &user, None).unwrap();

        assert!(
            svg.contains("ghostuser&#x27;s Git Stats") || svg.contains("ghostuser's Git Stats")
        );
        assert!(svg.contains(">0<"));
        assert!(svg.contains(">3<"));
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
            },
        );
        by_language.insert(
            "TypeScript".to_string(),
            LangStats {
                additions: 999,
                deletions: 1,
                files_changed: 50,
            },
        );
        by_language.insert(
            "Rust".to_string(),
            LangStats {
                additions: 500,
                deletions: 100,
                files_changed: 20,
            },
        );

        let stats = PeriodStats {
            period_label: "test".to_string(),
            by_language,
            by_author: HashMap::new(),
            total_commits: 0,
            total_additions: 0,
            total_deletions: 0,
        };

        let bars = build_lang_bars(Some(&stats));
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].name, "TypeScript");
        assert_eq!(bars[1].name, "Rust");
        assert_eq!(bars[2].name, "Go");
    }
}
