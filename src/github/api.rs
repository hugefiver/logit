use std::collections::HashMap;

use chrono::Utc;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct GithubUser {
    pub login: String,
    pub name: Option<String>,
    pub bio: Option<String>,
    pub public_repos: u64,
    pub followers: u64,
    pub following: u64,
    pub avatar_url: String,
    pub html_url: String,
    pub created_at: String,
}

pub struct GithubClient {
    client: Client,
}

impl GithubClient {
    pub fn new() -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("logit-cli"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github.v3+json"),
        );

        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            let auth_value = HeaderValue::from_str(&format!("Bearer {token}"))?;
            headers.insert(AUTHORIZATION, auth_value);
        }

        let client = Client::builder().default_headers(headers).build()?;

        Ok(Self { client })
    }

    pub fn get_user(&self, username: &str) -> anyhow::Result<GithubUser> {
        let url = format!("https://api.github.com/users/{username}");
        let resp = self.client.get(&url).send()?;

        match resp.status().as_u16() {
            403 => anyhow::bail!(
                "GitHub API rate limit exceeded. Set GITHUB_TOKEN env var for higher limits."
            ),
            404 => anyhow::bail!("GitHub user '{username}' not found."),
            200..=299 => {}
            _ => {
                resp.error_for_status_ref()?;
            }
        }

        let user: GithubUser = resp.json()?;
        Ok(user)
    }

    fn resolve_single_email(&self, owner: &str, repo: &str, email: &str) -> Option<String> {
        if let Some(login) = extract_noreply_username(email) {
            return Some(login);
        }
        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/commits?author={}&per_page=1",
            email
        );
        let resp = self.client.get(&url).send().ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let commits: Vec<CommitResponse> = resp.json().ok()?;
        commits
            .first()
            .and_then(|c| c.author.as_ref())
            .map(|a| a.login.clone())
    }

    /// List all repos for a user (paginated, up to 300).
    pub fn list_user_repos(&self, username: &str) -> anyhow::Result<Vec<GithubRepo>> {
        let mut all_repos = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "https://api.github.com/users/{username}/repos?per_page=100&page={page}&sort=pushed"
            );
            let resp = self.client.get(&url).send()?;
            Self::check_rate_limit(&resp)?;
            resp.error_for_status_ref()?;
            let repos: Vec<GithubRepo> = resp.json()?;
            if repos.is_empty() {
                break;
            }
            all_repos.extend(repos);
            if all_repos.len() >= 300 || page >= 3 {
                break;
            }
            page += 1;
        }
        Ok(all_repos)
    }

    /// Get contributor stats for a repo (weekly add/del per contributor).
    /// Returns empty vec on 202 (stats computing) or error.
    pub fn get_contributor_stats(
        &self,
        owner: &str,
        repo: &str,
    ) -> anyhow::Result<Vec<ContributorStat>> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/stats/contributors");
        // GitHub may return 202 if stats are being computed; retry once after a short wait
        for attempt in 0..2 {
            let resp = self.client.get(&url).send()?;
            match resp.status().as_u16() {
                200 => {
                    let stats: Vec<ContributorStat> = resp.json()?;
                    return Ok(stats);
                }
                202 => {
                    if attempt == 0 {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        continue;
                    }
                    return Ok(Vec::new());
                }
                204 => return Ok(Vec::new()),
                403 => {
                    anyhow::bail!("GitHub API rate limit exceeded");
                }
                _ => {
                    resp.error_for_status()?;
                }
            }
        }
        Ok(Vec::new())
    }

    /// Get language breakdown (bytes) for a repo.
    pub fn get_repo_languages(
        &self,
        owner: &str,
        repo: &str,
    ) -> anyhow::Result<HashMap<String, u64>> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/languages");
        let resp = self.client.get(&url).send()?;
        if resp.status().as_u16() == 403 {
            anyhow::bail!("GitHub API rate limit exceeded");
        }
        resp.error_for_status_ref()?;
        let langs: HashMap<String, u64> = resp.json()?;
        Ok(langs)
    }

    fn check_rate_limit(resp: &reqwest::blocking::Response) -> anyhow::Result<()> {
        if resp.status().as_u16() == 403 {
            anyhow::bail!(
                "GitHub API rate limit exceeded. Set GITHUB_TOKEN env var for higher limits."
            );
        }
        Ok(())
    }

    pub fn resolve_emails(
        &self,
        owner: &str,
        repo: &str,
        emails: &[String],
    ) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for email in emails {
            if map.values().any(|v: &String| v == email) {
                continue;
            }
            if let Some(login) = self.resolve_single_email(owner, repo, email) {
                map.insert(email.clone(), login);
            }
        }
        map
    }
}

pub struct RepoContribution {
    #[allow(dead_code)]
    pub repo_name: String,
    #[allow(dead_code)]
    pub total_commits: u64,
    #[allow(dead_code)]
    pub total_additions: u64,
    #[allow(dead_code)]
    pub total_deletions: u64,
    pub weeks: Vec<ContributorWeek>,
    pub languages: HashMap<String, u64>,
}

pub fn fetch_user_stats(
    client: &GithubClient,
    username: &str,
    include_forks: bool,
    since: Option<i64>,
) -> anyhow::Result<Vec<RepoContribution>> {
    eprintln!("Fetching repos for {username}...");
    let repos = client.list_user_repos(username)?;
    let repos: Vec<_> = repos
        .into_iter()
        .filter(|r| include_forks || !r.fork)
        .collect();
    eprintln!(
        "Found {} repos (excluding forks: {})",
        repos.len(),
        !include_forks
    );

    let mut contributions = Vec::new();

    for (i, repo) in repos.iter().enumerate() {
        let owner = &repo.owner.login;
        let name = &repo.name;
        eprint!("\r[{}/{}] {}/{name}...", i + 1, repos.len(), owner);

        let stats = match client.get_contributor_stats(owner, name) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(" skip ({e})");
                continue;
            }
        };

        let user_stat = stats.into_iter().find(|s| {
            s.author
                .as_ref()
                .is_some_and(|a| a.login.eq_ignore_ascii_case(username))
        });

        let Some(user_stat) = user_stat else {
            continue;
        };

        let weeks: Vec<ContributorWeek> = if let Some(since_ts) = since {
            user_stat
                .weeks
                .into_iter()
                .filter(|w| w.w >= since_ts)
                .collect()
        } else {
            user_stat.weeks
        };

        let total_commits: u64 = weeks.iter().map(|w| w.c).sum();
        let total_additions: u64 = weeks.iter().map(|w| w.a).sum();
        let total_deletions: u64 = weeks.iter().map(|w| w.d).sum();

        if total_commits == 0 && total_additions == 0 && total_deletions == 0 {
            continue;
        }

        let languages = client.get_repo_languages(owner, name).unwrap_or_default();

        contributions.push(RepoContribution {
            repo_name: repo.full_name.clone(),
            total_commits,
            total_additions,
            total_deletions,
            weeks,
            languages,
        });
    }
    eprintln!(
        "\rDone. {} repos with contributions.                    ",
        contributions.len()
    );

    Ok(contributions)
}

pub fn contributions_to_period_stats(
    contributions: &[RepoContribution],
    period: &crate::cli::Period,
) -> Vec<crate::stats::models::PeriodStats> {
    use crate::stats::models::PeriodStats;
    use chrono::TimeZone;

    let mut buckets: HashMap<String, PeriodStats> = HashMap::new();

    for contrib in contributions {
        let lang_total_bytes: u64 = contrib.languages.values().sum();

        for week in &contrib.weeks {
            if week.c == 0 && week.a == 0 && week.d == 0 {
                continue;
            }

            let ts = Utc.timestamp_opt(week.w, 0).single().unwrap_or_default();
            let label = crate::stats::aggregator::bucket_timestamp(&ts, period);

            let entry = buckets.entry(label.clone()).or_insert_with(|| PeriodStats {
                period_label: label,
                by_language: HashMap::new(),
                by_author: HashMap::new(),
                total_commits: 0,
                total_additions: 0,
                total_deletions: 0,
            });

            entry.total_commits += week.c;
            entry.total_additions += week.a;
            entry.total_deletions += week.d;

            if lang_total_bytes > 0 {
                for (lang, &bytes) in &contrib.languages {
                    let ratio = bytes as f64 / lang_total_bytes as f64;
                    let lang_adds = (week.a as f64 * ratio).round() as u64;
                    let lang_dels = (week.d as f64 * ratio).round() as u64;

                    let lang_entry = entry.by_language.entry(lang.clone()).or_default();
                    lang_entry.additions += lang_adds;
                    lang_entry.deletions += lang_dels;
                    lang_entry.files_changed += 1;
                }
            } else if week.a > 0 || week.d > 0 {
                let lang_entry = entry.by_language.entry("Other".to_string()).or_default();
                lang_entry.additions += week.a;
                lang_entry.deletions += week.d;
                lang_entry.files_changed += 1;
            }
        }
    }

    let mut result: Vec<PeriodStats> = buckets.into_values().collect();
    result.sort_by(|a, b| a.period_label.cmp(&b.period_label));
    result
}

fn extract_noreply_username(email: &str) -> Option<String> {
    if !email.ends_with("noreply.github.com") {
        return None;
    }
    let local = email.split('@').next()?;
    if let Some((_, username)) = local.split_once('+') {
        Some(username.to_string())
    } else {
        Some(local.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct CommitResponse {
    author: Option<CommitAuthor>,
}

#[derive(Debug, Deserialize)]
struct CommitAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubRepo {
    pub name: String,
    pub full_name: String,
    pub fork: bool,
    pub owner: RepoOwner,
}

#[derive(Debug, Deserialize)]
pub struct RepoOwner {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct ContributorStat {
    pub author: Option<ContributorAuthor>,
    #[allow(dead_code)]
    pub total: u64,
    pub weeks: Vec<ContributorWeek>,
}

#[derive(Debug, Deserialize)]
pub struct ContributorAuthor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContributorWeek {
    pub w: i64,
    pub a: u64,
    pub d: u64,
    pub c: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "login": "octocat",
            "name": "The Octocat",
            "bio": "GitHub mascot",
            "public_repos": 8,
            "followers": 10000,
            "following": 5,
            "avatar_url": "https://avatars.githubusercontent.com/u/583231",
            "html_url": "https://github.com/octocat",
            "created_at": "2011-01-25T18:44:36Z"
        }"#
    }

    #[test]
    fn deserialize_github_user() {
        let user: GithubUser = serde_json::from_str(sample_json()).unwrap();
        assert_eq!(user.login, "octocat");
        assert_eq!(user.name, Some("The Octocat".to_string()));
        assert_eq!(user.bio, Some("GitHub mascot".to_string()));
        assert_eq!(user.public_repos, 8);
        assert_eq!(user.followers, 10000);
        assert_eq!(user.following, 5);
        assert_eq!(
            user.avatar_url,
            "https://avatars.githubusercontent.com/u/583231"
        );
        assert_eq!(user.html_url, "https://github.com/octocat");
        assert_eq!(user.created_at, "2011-01-25T18:44:36Z");
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let user: GithubUser = serde_json::from_str(sample_json()).unwrap();
        let serialized = serde_json::to_string(&user).unwrap();
        let deserialized: GithubUser = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.login, user.login);
        assert_eq!(deserialized.name, user.name);
        assert_eq!(deserialized.bio, user.bio);
        assert_eq!(deserialized.public_repos, user.public_repos);
        assert_eq!(deserialized.followers, user.followers);
        assert_eq!(deserialized.following, user.following);
        assert_eq!(deserialized.avatar_url, user.avatar_url);
        assert_eq!(deserialized.html_url, user.html_url);
        assert_eq!(deserialized.created_at, user.created_at);
    }

    #[test]
    fn optional_fields_can_be_null() {
        let json = r#"{
            "login": "ghost",
            "name": null,
            "bio": null,
            "public_repos": 0,
            "followers": 0,
            "following": 0,
            "avatar_url": "https://avatars.githubusercontent.com/u/0",
            "html_url": "https://github.com/ghost",
            "created_at": "2020-01-01T00:00:00Z"
        }"#;
        let user: GithubUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.login, "ghost");
        assert!(user.name.is_none());
        assert!(user.bio.is_none());
    }

    #[test]
    fn extract_noreply_with_id() {
        assert_eq!(
            extract_noreply_username("18693500+hugefiver@users.noreply.github.com"),
            Some("hugefiver".to_string())
        );
    }

    #[test]
    fn extract_noreply_without_id() {
        assert_eq!(
            extract_noreply_username("hugefiver@users.noreply.github.com"),
            Some("hugefiver".to_string())
        );
    }

    #[test]
    fn extract_noreply_regular_email_returns_none() {
        assert_eq!(extract_noreply_username("user@example.com"), None);
    }
}
