use std::path::PathBuf;

use crate::stats::models::{GroupNode, PeriodStats};

pub fn render_stats_json(stats: &[PeriodStats], totals: &PeriodStats) -> anyhow::Result<String> {
    let output = serde_json::json!({
        "periods": stats,
        "totals": totals,
    });
    Ok(serde_json::to_string_pretty(&output)?)
}

pub fn render_group_tree_json(nodes: &[GroupNode]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(nodes)?)
}

#[cfg(feature = "github")]
pub fn render_github_stats_json(
    metadata: serde_json::Value,
    user: &crate::github::api::GithubUser,
    stats: &[PeriodStats],
    totals: &PeriodStats,
    summary: &crate::github::api::ContributionSummary,
) -> anyhow::Result<String> {
    let output = serde_json::json!({
        "metadata": metadata,
        "user": user,
        "periods": stats,
        "totals": {
            "total_commits": totals.total_commits,
            "total_additions": totals.total_additions,
            "total_deletions": totals.total_deletions,
            "total_net_modifications": totals.total_net_modifications,
            "total_net_additions": totals.total_net_additions,
            "by_language": totals.by_language,
        },
        "summary": summary,
    });
    Ok(serde_json::to_string_pretty(&output)?)
}

#[cfg(feature = "github")]
pub fn render_github_group_tree_json(
    metadata: serde_json::Value,
    user: &crate::github::api::GithubUser,
    summary: &crate::github::api::ContributionSummary,
    groups: &[GroupNode],
) -> anyhow::Result<String> {
    let output = serde_json::json!({
        "metadata": metadata,
        "user": user,
        "summary": summary,
        "groups": groups,
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
                net_modifications: 12,
                net_additions: 9,
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
                net_modifications: 12,
                co_authored_net_modifications: 0,
                net_additions: 9,
                co_authored_net_additions: 0,
                languages: by_language.clone(),
                co_authored_languages: HashMap::new(),
            },
        );

        PeriodStats {
            period_label: label.to_string(),
            by_language,
            by_author,
            total_commits: 4,
            total_additions: 12,
            total_deletions: 3,
            total_net_modifications: 12,
            total_net_additions: 9,
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

    #[test]
    fn local_json_keeps_flat_and_hierarchical_shapes_and_all_metrics() {
        let period = sample_period_stats("2025-01");
        let totals = sample_period_stats("Total");
        let flat: Value = serde_json::from_str(
            &render_stats_json(std::slice::from_ref(&period), &totals).unwrap(),
        )
        .unwrap();
        assert!(flat["periods"].is_array());
        assert!(flat.get("totals").is_some());
        assert_eq!(flat["periods"][0]["total_commits"], 4);
        assert_eq!(flat["periods"][0]["total_additions"], 12);
        assert_eq!(flat["periods"][0]["total_deletions"], 3);

        let groups = vec![GroupNode {
            label: "repo-a".to_string(),
            stats: period,
            children: vec![],
        }];
        let hierarchical: Value =
            serde_json::from_str(&render_group_tree_json(&groups).unwrap()).unwrap();
        assert!(hierarchical.is_array());
        assert_eq!(hierarchical[0]["label"], "repo-a");
        assert_eq!(hierarchical[0]["stats"]["total_commits"], 4);
        assert_eq!(hierarchical[0]["stats"]["total_additions"], 12);
        assert_eq!(hierarchical[0]["stats"]["total_deletions"], 3);
    }

    #[cfg(feature = "github")]
    #[test]
    fn github_json_keeps_flat_and_hierarchical_compatibility_shapes() {
        let user = crate::github::api::GithubUser {
            login: "octocat".to_string(),
            name: None,
            bio: None,
            public_repos: 1,
            followers: 0,
            following: 0,
            avatar_url: "https://example.test/avatar".to_string(),
            html_url: "https://example.test/octocat".to_string(),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            node_id: "node".to_string(),
        };
        let period = sample_period_stats("2025-01");
        let totals = sample_period_stats("Total");
        let summary = crate::github::api::ContributionSummary::default();
        let metadata = serde_json::json!({ "username": "octocat" });

        let flat: Value = serde_json::from_str(
            &render_github_stats_json(
                metadata.clone(),
                &user,
                std::slice::from_ref(&period),
                &totals,
                &summary,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(flat.get("metadata").is_some());
        assert!(flat.get("user").is_some());
        assert!(flat["periods"].is_array());
        assert!(flat.get("totals").is_some());
        assert!(flat.get("summary").is_some());
        assert_eq!(flat["periods"][0]["total_additions"], 12);

        let groups = vec![GroupNode {
            label: "repo-a".to_string(),
            stats: period,
            children: vec![],
        }];
        let hierarchical: Value = serde_json::from_str(
            &render_github_group_tree_json(metadata, &user, &summary, &groups).unwrap(),
        )
        .unwrap();
        assert!(hierarchical.get("metadata").is_some());
        assert!(hierarchical.get("user").is_some());
        assert!(hierarchical.get("summary").is_some());
        assert!(hierarchical["groups"].is_array());
        assert!(hierarchical.get("periods").is_none());
        assert!(hierarchical.get("totals").is_none());
        assert_eq!(hierarchical["groups"][0]["stats"]["total_deletions"], 3);
    }
}
