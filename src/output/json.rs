use std::path::PathBuf;

use crate::stats::models::PeriodStats;

pub fn render_stats_json(stats: &[PeriodStats], totals: &PeriodStats) -> anyhow::Result<String> {
    let output = serde_json::json!({
        "periods": stats,
        "totals": totals,
    });
    Ok(serde_json::to_string_pretty(&output)?)
}

pub fn render_scan_json(repos: &[PathBuf]) -> anyhow::Result<String> {
    let output = serde_json::json!({
        "repositories": repos.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "count": repos.len(),
    });
    Ok(serde_json::to_string_pretty(&output)?)
}

pub fn print_stats_json(stats: &[PeriodStats], totals: &PeriodStats) -> anyhow::Result<()> {
    println!("{}", render_stats_json(stats, totals)?);
    Ok(())
}

pub fn print_scan_json(repos: &[PathBuf]) -> anyhow::Result<()> {
    println!("{}", render_scan_json(repos)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::models::{AuthorStats, LangStats};
    use serde_json::Value;
    use std::collections::HashMap;

    fn sample_period_stats(label: &str) -> PeriodStats {
        let mut by_language = HashMap::new();
        by_language.insert(
            "Rust".to_string(),
            LangStats {
                additions: 12,
                deletions: 3,
                files_changed: 2,
            },
        );

        let mut by_author = HashMap::new();
        by_author.insert(
            "alice@example.com".to_string(),
            AuthorStats {
                commits: 4,
                co_authored_commits: 0,
                additions: 12,
                co_authored_additions: 0,
                deletions: 3,
                co_authored_deletions: 0,
                languages: by_language.clone(),
            },
        );

        PeriodStats {
            period_label: label.to_string(),
            by_language,
            by_author,
            total_commits: 4,
            total_additions: 12,
            total_deletions: 3,
        }
    }

    #[test]
    fn render_stats_json_contains_expected_keys() {
        let periods = vec![sample_period_stats("2025-W01")];
        let totals = sample_period_stats("totals");

        let parsed: Value =
            serde_json::from_str(&render_stats_json(&periods, &totals).unwrap()).unwrap();

        assert!(parsed.get("periods").is_some());
        assert!(parsed.get("totals").is_some());
        assert!(parsed["periods"][0].get("period_label").is_some());
    }

    #[test]
    fn render_scan_json_reports_correct_count() {
        let repos = vec![PathBuf::from("repo-a"), PathBuf::from("repo-b")];

        let parsed: Value = serde_json::from_str(&render_scan_json(&repos).unwrap()).unwrap();

        assert_eq!(parsed["count"], 2);
        assert_eq!(parsed["repositories"][0], "repo-a");
        assert_eq!(parsed["repositories"][1], "repo-b");
    }

    #[test]
    fn rendered_json_is_valid() {
        let periods = vec![sample_period_stats("2025-W01")];
        let totals = sample_period_stats("totals");
        let stats_json = render_stats_json(&periods, &totals).unwrap();
        let scan_json = render_scan_json(&[PathBuf::from("repo")]).unwrap();

        serde_json::from_str::<Value>(&stats_json).unwrap();
        serde_json::from_str::<Value>(&scan_json).unwrap();
    }
}
