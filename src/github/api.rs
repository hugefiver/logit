use anyhow::Context;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

use super::cache::DiskCache;
use crate::cli::{GroupBy, Period};
use crate::git::author::canonical_email_key;
use crate::stats::models::{GroupNode, PeriodStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CachePolicy {
    ReadOnly,
    Refresh,
    Disabled,
}

impl CachePolicy {
    pub fn from_flags(no_cache: bool, refresh_cache: bool) -> (Self, bool) {
        if no_cache {
            (Self::Disabled, refresh_cache)
        } else if refresh_cache {
            (Self::Refresh, false)
        } else {
            (Self::ReadOnly, false)
        }
    }

    pub fn can_read(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::Refresh)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheWindowScope {
    Fixed {
        from: DateTime<Utc>,
        until_exclusive: DateTime<Utc>,
    },
    Rolling {
        lookback_nanoseconds: i64,
    },
    Anchored {
        from: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryWindow {
    pub scope: CacheWindowScope,
    pub requested_from: DateTime<Utc>,
    pub until_exclusive: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub completed: bool,
}

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
    #[serde(skip)]
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityLookupFailure {
    pub repository: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityResolutionReport {
    pub login: String,
    pub emails: BTreeSet<String>,
    pub repositories_examined: usize,
    pub logical_requests: usize,
    pub truncated_repositories: bool,
    pub truncated_commits: bool,
    pub failures: Vec<IdentityLookupFailure>,
}

impl IdentityResolutionReport {
    #[allow(dead_code)] // Task 7 consumes report completeness at the command layer.
    pub fn is_partial(&self) -> bool {
        self.truncated_repositories || self.truncated_commits || !self.failures.is_empty()
    }

    #[allow(dead_code)] // Task 7 prints this report-level warning at the command layer.
    pub fn warning(&self) -> Option<String> {
        if !self.is_partial() {
            return None;
        }

        let known_emails = if self.emails.is_empty() {
            "none".to_string()
        } else {
            self.emails.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        Some(format!(
            "Identity resolution for '{}' is partial; known emails: {known_emails}; results may miss others, so retry later or verify repository access.",
            self.login
        ))
    }
}

pub struct GithubClient {
    client: Client,
    has_token: bool,
    graphql_base_url: String,
    rest_base_url: String,
    retry_delays: Vec<std::time::Duration>,
}

const GITHUB_GRAPHQL_BASE_URL: &str = "https://api.github.com/graphql";
const GITHUB_REST_BASE_URL: &str = "https://api.github.com";
const DEFAULT_RETRY_DELAYS: [std::time::Duration; 6] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(15),
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(60),
];
const MAX_SERVER_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, PartialEq, Eq)]
enum RetryDecision {
    Return,
    RetryAfter(std::time::Duration),
    Fail(String),
}

fn retry_decision(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    fallback: std::time::Duration,
    now: DateTime<Utc>,
    max_server_wait: std::time::Duration,
) -> RetryDecision {
    if status.is_success() {
        return RetryDecision::Return;
    }

    let server_wait = match status.as_u16() {
        429 | 403 => retry_after_wait(headers, now).or_else(|| rate_limit_reset_wait(headers, now)),
        _ => None,
    };

    if let Some(wait) = server_wait {
        if wait > max_server_wait {
            return RetryDecision::Fail(format!(
                "server requested retry wait of {}s exceeds maximum {}s",
                wait.as_secs(),
                max_server_wait.as_secs()
            ));
        }
        return RetryDecision::RetryAfter(wait);
    }

    match status.as_u16() {
        403 => RetryDecision::Fail(
            "permission denied or rate-limit headers were not provided".to_string(),
        ),
        408 | 429 | 500..=599 => RetryDecision::RetryAfter(fallback),
        400..=499 => RetryDecision::Fail("request was rejected by GitHub".to_string()),
        _ => RetryDecision::Return,
    }
}

fn retry_after_wait(headers: &HeaderMap, now: DateTime<Utc>) -> Option<std::time::Duration> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(std::time::Duration::from_secs(seconds));
    }

    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    let seconds = retry_at.timestamp().saturating_sub(now.timestamp()).max(0) as u64;
    Some(std::time::Duration::from_secs(seconds))
}

fn rate_limit_reset_wait(headers: &HeaderMap, now: DateTime<Utc>) -> Option<std::time::Duration> {
    let remaining = headers.get("x-ratelimit-remaining")?.to_str().ok()?.trim();
    if remaining != "0" {
        return None;
    }

    let reset = headers
        .get("x-ratelimit-reset")?
        .to_str()
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()?;
    let seconds = reset.saturating_sub(now.timestamp()).max(1) as u64;
    Some(std::time::Duration::from_secs(seconds))
}

const USER_QUERY: &str = r#"
query($login: String!) {
  user(login: $login) {
    id
    login
    name
    bio
    publicRepositories: repositories(privacy: PUBLIC) { totalCount }
    followers { totalCount }
    following { totalCount }
    avatarUrl
    url
    createdAt
  }
}
"#;

const CONTRIBUTIONS_QUERY: &str = r#"
query($login: String!, $from: DateTime!, $to: DateTime!) {
  rateLimit { cost remaining resetAt }
  user(login: $login) {
    contributionsCollection(from: $from, to: $to) {
      totalPullRequestContributions
      totalPullRequestReviewContributions
      totalIssueContributions
      commitContributionsByRepository(maxRepositories: 100) {
        repository {
          name
          owner { login }
          isFork
          languages(first: 10, orderBy: { field: SIZE, direction: DESC }) {
            edges { size node { name } }
          }
        }
        contributions { totalCount }
      }
    }
  }
}
"#;

#[allow(dead_code)]
const USER_REPOS_QUERY: &str = r#"
query($login: String!, $after: String) {
  rateLimit { cost remaining resetAt }
  user(login: $login) {
    repositories(
      first: 20,
      after: $after,
      ownerAffiliations: [OWNER],
      orderBy: { field: PUSHED_AT, direction: DESC }
    ) {
      pageInfo { hasNextPage endCursor }
      nodes {
        name
        owner { login }
        isFork
        languages(first: 10, orderBy: { field: SIZE, direction: DESC }) {
          edges { size node { name } }
        }
      }
    }
  }
}
"#;

#[allow(dead_code)]
const CONTRIBUTED_REPOS_QUERY: &str = r#"
query($login: String!, $after: String) {
  rateLimit { cost remaining resetAt }
  user(login: $login) {
    repositoriesContributedTo(
      first: 20,
      after: $after,
      contributionTypes: [COMMIT, PULL_REQUEST],
      includeUserRepositories: false
    ) {
      pageInfo { hasNextPage endCursor }
      nodes {
        name
        owner { login }
        isFork
        languages(first: 10, orderBy: { field: SIZE, direction: DESC }) {
          edges { size node { name } }
        }
      }
    }
  }
}
"#;

/// Enumerate the token holder's own PRIVATE repos directly via `viewer`.
/// Used to bypass the long-standing GitHub limitation where
/// `contributionsCollection.commitContributionsByRepository` hides private data
/// for fine-grained PATs. Combined with `batch_commit_history` (which works
/// against fine-grained tokens with Contents:read), this surfaces commit/line
/// stats for owner-private repos. PR/Issue/Review counts remain public-only.
const VIEWER_PRIVATE_REPOS_QUERY: &str = r#"
query($after: String) {
  rateLimit { cost remaining resetAt }
  viewer {
    login
    repositories(
      first: 100,
      after: $after,
      privacy: PRIVATE,
      ownerAffiliations: [OWNER],
      orderBy: { field: PUSHED_AT, direction: DESC }
    ) {
      pageInfo { hasNextPage endCursor }
      nodes {
        name
        owner { login }
        isFork
        languages(first: 10, orderBy: { field: SIZE, direction: DESC }) {
          edges { size node { name } }
        }
      }
    }
  }
}
"#;

impl GithubClient {
    pub fn new() -> anyhow::Result<Self> {
        let (client, has_token) = Self::build_client(std::time::Duration::from_secs(30))?;

        Ok(Self {
            client,
            has_token,
            graphql_base_url: GITHUB_GRAPHQL_BASE_URL.to_string(),
            rest_base_url: GITHUB_REST_BASE_URL.to_string(),
            retry_delays: DEFAULT_RETRY_DELAYS.to_vec(),
        })
    }

    fn build_client(timeout: std::time::Duration) -> anyhow::Result<(Client, bool)> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("logit-cli"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github.v3+json"),
        );

        let mut has_token = false;
        if let Ok(token) = std::env::var("GITHUB_TOKEN")
            && !token.is_empty()
        {
            let auth_value = HeaderValue::from_str(&format!("Bearer {token}"))?;
            headers.insert(AUTHORIZATION, auth_value);
            has_token = true;
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .build()?;

        Ok((client, has_token))
    }

    #[cfg(test)]
    fn for_test(
        base_url: &str,
        retry_delays: Vec<std::time::Duration>,
        timeout: std::time::Duration,
    ) -> Self {
        let (client, _) = Self::build_client(timeout).expect("test GitHub client should build");

        Self {
            client,
            has_token: true,
            graphql_base_url: format!("{}/graphql", base_url.trim_end_matches('/')),
            rest_base_url: base_url.trim_end_matches('/').to_string(),
            retry_delays,
        }
    }

    pub fn has_token(&self) -> bool {
        self.has_token
    }

    fn send_with_retry<F>(
        &self,
        request_factory: F,
        scope: &str,
    ) -> anyhow::Result<reqwest::blocking::Response>
    where
        F: Fn() -> reqwest::blocking::RequestBuilder,
    {
        let max_attempts = self.retry_delays.len() + 1;

        for attempt in 0..max_attempts {
            match request_factory().send() {
                Ok(response) => {
                    let status = response.status();
                    let fallback = self.retry_delays.get(attempt).copied().unwrap_or_default();
                    match retry_decision(
                        status,
                        response.headers(),
                        fallback,
                        Utc::now(),
                        MAX_SERVER_RETRY_WAIT,
                    ) {
                        RetryDecision::Return => return Ok(response),
                        RetryDecision::Fail(reason) => {
                            anyhow::bail!(
                                "GitHub {scope} request failed with status {status}: {reason}."
                            );
                        }
                        RetryDecision::RetryAfter(delay) => {
                            if attempt + 1 == max_attempts {
                                anyhow::bail!(
                                    "GitHub {scope} request failed after {n} retries. Last status: {status}.",
                                    n = max_attempts - 1
                                );
                            }

                            eprintln!(
                                "\nGitHub {scope} request returned {status}. Retrying in {}s (attempt {}/{})...",
                                delay.as_secs(),
                                attempt + 1,
                                max_attempts,
                            );
                            std::thread::sleep(delay);
                        }
                    }
                }
                // Reqwest classifies a peer that accepts then closes before a response as a
                // SendRequest (`is_request`) error instead of `is_connect()`.
                Err(error) if error.is_timeout() || error.is_connect() || error.is_request() => {
                    if attempt + 1 == max_attempts {
                        let reason = if error.is_timeout() {
                            format!("timed out: {error}")
                        } else {
                            format!("connection failed: {error}")
                        };
                        anyhow::bail!(
                            "GitHub {scope} request failed after {n} retries. Last transport error: {reason}",
                            n = max_attempts - 1
                        );
                    }

                    let delay = self.retry_delays[attempt];
                    let reason = if error.is_timeout() {
                        "timed out"
                    } else {
                        "connection failed"
                    };
                    eprintln!(
                        "\nGitHub {scope} request {reason}. Retrying in {}s (attempt {}/{})...",
                        delay.as_secs(),
                        attempt + 1,
                        max_attempts,
                    );
                    std::thread::sleep(delay);
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("GitHub {scope} request failed"));
                }
            }
        }

        unreachable!("retry loop always returns on its final attempt")
    }

    fn graphql_query(
        &self,
        query: &str,
        variables: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        if !self.has_token {
            anyhow::bail!("GITHUB_TOKEN is required for this operation");
        }

        let body = serde_json::json!({ "query": query, "variables": variables });

        let resp = self.send_with_retry(
            || self.client.post(&self.graphql_base_url).json(&body),
            "GraphQL",
        )?;

        match resp.status().as_u16() {
            401 => anyhow::bail!("GitHub GraphQL authentication failed. Check GITHUB_TOKEN."),
            200..=299 => {
                let payload: serde_json::Value = resp.json()?;
                let data = parse_graphql_response_payload(&payload)?;

                // Check remaining budget and warn/pre-emptively wait
                if let Some(rate_limit) = payload.get("data").and_then(|d| d.get("rateLimit")) {
                    let remaining = rate_limit
                        .get("remaining")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(u64::MAX);
                    let cost = rate_limit.get("cost").and_then(|v| v.as_u64()).unwrap_or(0);
                    if remaining == 0 {
                        if let Some(reset_at) = rate_limit.get("resetAt").and_then(|v| v.as_str()) {
                            let wait = parse_reset_at_wait(reset_at).unwrap_or(60).min(120);
                            eprintln!(
                                "\nGraphQL budget exhausted (cost={cost}, remaining=0). Waiting {wait}s for reset...",
                            );
                            std::thread::sleep(std::time::Duration::from_secs(wait));
                        }
                    } else if remaining < cost * 2 {
                        eprintln!(
                            "\nWarning: GraphQL budget low (remaining={remaining}, last cost={cost})."
                        );
                    }
                }

                Ok(data)
            }
            status => anyhow::bail!("GitHub GraphQL request failed with status {status}."),
        }
    }

    pub fn get_user(&self, username: &str) -> anyhow::Result<GithubUser> {
        let variables = serde_json::json!({ "login": username });
        let data = self.graphql_query(USER_QUERY, &variables)?;
        parse_graphql_user_data(data, username)
    }

    pub fn get_contribution_repos(
        &self,
        username: &str,
        from: DateTime<Utc>,
        until_exclusive: DateTime<Utc>,
        include_forks: bool,
        include_contributed: bool,
    ) -> anyhow::Result<(Vec<(RepoWithLangs, u64)>, ContributionSummary)> {
        let (payload, completeness) = self.get_contribution_payload(
            username,
            from,
            until_exclusive,
            include_forks,
            include_contributed,
        )?;
        if !completeness.is_complete()
            && let Some(warning) = completeness.visible_warning()
        {
            eprintln!("Warning: {warning}");
        }
        Ok((payload.repos, payload.summary))
    }

    fn get_contribution_payload(
        &self,
        username: &str,
        from: DateTime<Utc>,
        until_exclusive: DateTime<Utc>,
        include_forks: bool,
        include_contributed: bool,
    ) -> anyhow::Result<(CachedContributionPayload, Completeness)> {
        let windows = contribution_windows(from, until_exclusive);
        let mut merged: HashMap<String, (RepoWithLangs, u64)> = HashMap::new();
        let mut total_summary = ContributionSummary::default();
        let mut incomplete_reasons = Vec::new();

        for (from, to) in windows {
            let variables = contribution_query_variables(username, &from, &to)?;
            let data = self.graphql_query(CONTRIBUTIONS_QUERY, &variables)?;
            let (repos, summary) = parse_contributions_collection_data(data, username)?;
            if contribution_window_is_saturated(&repos) {
                incomplete_reasons
                    .push(IncompleteReason::ContributionRepositoryLimit { limit: 100 });
            }
            total_summary.total_prs += summary.total_prs;
            total_summary.total_reviews += summary.total_reviews;
            total_summary.total_issues += summary.total_issues;

            for (repo, commit_count) in repos {
                let key = repo_key(&repo.owner, &repo.name);
                if let Some((existing, total)) = merged.get_mut(&key) {
                    *total += commit_count;
                    if existing.languages.is_empty() && !repo.languages.is_empty() {
                        existing.languages = repo.languages;
                    }
                } else {
                    merged.insert(key, (repo, commit_count));
                }
            }
        }

        let mut repos: Vec<(RepoWithLangs, u64)> = merged.into_values().collect();

        if !include_forks {
            repos.retain(|(repo, _)| !repo.is_fork);
        }

        if !include_contributed {
            repos.retain(|(repo, _)| repo.owner.eq_ignore_ascii_case(username));
        }

        repos.sort_by(|(a, _), (b, _)| {
            a.owner
                .to_lowercase()
                .cmp(&b.owner.to_lowercase())
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        incomplete_reasons.sort();
        incomplete_reasons.dedup();
        let completeness = if incomplete_reasons.is_empty() {
            Completeness::Complete
        } else {
            Completeness::Incomplete(incomplete_reasons)
        };
        Ok((
            CachedContributionPayload {
                repos,
                summary: total_summary,
            },
            completeness,
        ))
    }

    fn batch_commit_history(
        &self,
        user_node_id: &str,
        repos: &[RepoHistoryRequest],
    ) -> anyhow::Result<BatchCommitHistory> {
        let mut all_commits: HashMap<String, Vec<CommitData>> = HashMap::new();
        let mut capped_repos = HashSet::new();

        for repo_batch in repos.chunks(5) {
            let mut active: Vec<PageRequest> = repo_batch
                .iter()
                .enumerate()
                .map(|(i, repo)| PageRequest {
                    batch_index: i,
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    since: repo.since.clone(),
                    until_exclusive: repo.until_exclusive.clone(),
                    after: None,
                })
                .collect();

            let mut pages_fetched: HashMap<usize, usize> = HashMap::new();

            while !active.is_empty() {
                let query = build_batch_history_query(&active);
                let variables = build_batch_history_variables(user_node_id, &active);

                let data = self.graphql_query(&query, &variables)?;
                let parsed = parse_batch_history_data(data, &active)?;

                let mut next_active = Vec::new();
                for req in active {
                    let repo_name = format!("{}/{}", req.owner, req.name);
                    let pages = pages_fetched.entry(req.batch_index).or_insert(0);
                    *pages += 1;

                    let Some(parsed_repo) = parsed.get(&req.batch_index) else {
                        continue;
                    };
                    let commits = filter_commits_to_range(
                        &parsed_repo.commits,
                        req.since.as_deref(),
                        req.until_exclusive.as_deref(),
                    )
                    .with_context(|| {
                        format!(
                            "failed to filter fetched commit history for {}/{}",
                            req.owner, req.name
                        )
                    })?;

                    all_commits
                        .entry(repo_name.clone())
                        .or_default()
                        .extend(commits);

                    let scope = format!("history for {repo_name}");
                    match pagination_decision(
                        parsed_repo.has_next_page,
                        parsed_repo.end_cursor.as_deref(),
                        *pages,
                        HISTORY_MAX_PAGES_PER_REPO,
                        &scope,
                    )? {
                        PaginationDecision::Stop => {}
                        PaginationDecision::Continue(cursor) => {
                            next_active.push(PageRequest {
                                batch_index: req.batch_index,
                                owner: req.owner,
                                name: req.name,
                                since: req.since,
                                until_exclusive: req.until_exclusive,
                                after: Some(cursor),
                            });
                        }
                        PaginationDecision::Capped(warning) => {
                            capped_repos.insert(repo_key(&req.owner, &req.name));
                            eprintln!(
                                "Warning: {warning} (totalCount={})",
                                parsed_repo.total_count
                            )
                        }
                    }
                }
                active = next_active;
            }
        }

        Ok(BatchCommitHistory {
            commits: all_commits,
            capped_repos,
        })
    }

    pub(crate) fn resolve_single_email_result(
        &self,
        owner: &str,
        repo: &str,
        email: &str,
    ) -> anyhow::Result<Option<String>> {
        if let Some(login) = extract_noreply_username(email) {
            return Ok(Some(login));
        }
        let mut url = reqwest::Url::parse(&format!(
            "{}/repos/{owner}/{repo}/commits",
            self.rest_base_url.trim_end_matches('/')
        ))
        .context("failed to build GitHub REST commit identity URL")?;
        url.query_pairs_mut()
            .append_pair("author", email)
            .append_pair("per_page", "1");
        let resp = self.send_with_retry(|| self.client.get(url.clone()), "REST commit identity")?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "GitHub REST commit identity request failed with status {}.",
                resp.status()
            );
        }
        let commits: Vec<CommitResponse> = resp
            .json()
            .context("failed to parse GitHub REST commit identity response")?;
        Ok(commits
            .first()
            .and_then(|c| c.author.as_ref())
            .map(|a| a.login.clone()))
    }

    pub fn resolve_user_identity(&self, login: &str) -> IdentityResolutionReport {
        let mut report = IdentityResolutionReport {
            login: login.to_string(),
            emails: BTreeSet::new(),
            repositories_examined: 0,
            logical_requests: 1,
            truncated_repositories: false,
            truncated_commits: false,
            failures: Vec::new(),
        };

        let data = match self.graphql_query(
            USER_REPOS_QUERY,
            &serde_json::json!({ "login": login, "after": serde_json::Value::Null }),
        ) {
            Ok(data) => data,
            Err(error) => {
                report.failures.push(IdentityLookupFailure {
                    repository: None,
                    message: format!("failed to list owned repositories: {error}"),
                });
                return report;
            }
        };
        let (repos, page_info, _) =
            match parse_repo_connection_data(data, login, RepoConnectionKind::Owned, false) {
                Ok(parsed) => parsed,
                Err(error) => {
                    report.failures.push(IdentityLookupFailure {
                        repository: None,
                        message: format!("failed to parse owned repositories: {error}"),
                    });
                    return report;
                }
            };

        report.truncated_repositories = page_info.has_next_page || repos.len() > 8;

        for repo in repos.into_iter().take(8) {
            report.repositories_examined += 1;
            let repository = format!("{}/{}", repo.owner, repo.name);
            let url = reqwest::Url::parse(&format!(
                "{}/repos/{}/{}/commits",
                self.rest_base_url.trim_end_matches('/'),
                repo.owner,
                repo.name
            ))
            .with_context(|| {
                format!("failed to build GitHub REST commit email URL for {repository}")
            });
            let mut url = match url {
                Ok(url) => url,
                Err(error) => {
                    report.failures.push(IdentityLookupFailure {
                        repository: Some(repository),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            url.query_pairs_mut()
                .append_pair("author", login)
                .append_pair("per_page", "20");
            report.logical_requests += 1;
            let response = match self
                .send_with_retry(|| self.client.get(url.clone()), "REST commit email lookup")
            {
                Ok(response) => response,
                Err(error) => {
                    report.failures.push(IdentityLookupFailure {
                        repository: Some(repository),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if !response.status().is_success() {
                report.failures.push(IdentityLookupFailure {
                    repository: Some(repository),
                    message: format!(
                        "GitHub REST commit email lookup failed with status {}",
                        response.status()
                    ),
                });
                continue;
            }

            if response
                .headers()
                .get("link")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("rel=\"next\""))
            {
                report.truncated_commits = true;
            }
            match response.json::<Vec<serde_json::Value>>() {
                Ok(commits) => {
                    for commit in commits {
                        if let Some(email) = commit
                            .pointer("/commit/author/email")
                            .and_then(|value| value.as_str())
                        {
                            let email = canonical_email_key(email);
                            if !email.is_empty() {
                                report.emails.insert(email);
                            }
                        }
                    }
                }
                Err(error) => report.failures.push(IdentityLookupFailure {
                    repository: Some(repository),
                    message: format!("failed to parse commit emails: {error}"),
                }),
            }
        }

        report
    }

    #[allow(dead_code)]
    pub fn list_user_repos_graphql(
        &self,
        username: &str,
        include_forks: bool,
    ) -> anyhow::Result<Vec<RepoWithLangs>> {
        let mut all_repos = Vec::new();
        let mut fetched_count = 0usize;
        let mut after: Option<String> = None;

        loop {
            let variables = serde_json::json!({ "login": username, "after": after });
            let data = self.graphql_query(USER_REPOS_QUERY, &variables)?;
            let (repos, page_info, page_node_count) = parse_repo_connection_data(
                data,
                username,
                RepoConnectionKind::Owned,
                include_forks,
            )?;

            all_repos.extend(repos);

            fetched_count += page_node_count;
            match pagination_decision(
                page_info.has_next_page,
                page_info.end_cursor.as_deref(),
                fetched_count,
                300,
                "owned repositories",
            )? {
                PaginationDecision::Stop => break,
                PaginationDecision::Continue(cursor) => after = Some(cursor),
                PaginationDecision::Capped(warning) => {
                    eprintln!("Warning: {warning}");
                    break;
                }
            }
        }

        Ok(all_repos)
    }

    #[allow(dead_code)]
    pub fn list_contributed_repos_graphql(
        &self,
        username: &str,
    ) -> anyhow::Result<Vec<RepoWithLangs>> {
        let mut all_repos = Vec::new();
        let mut fetched_count = 0usize;
        let mut after: Option<String> = None;

        loop {
            let variables = serde_json::json!({ "login": username, "after": after });
            let data = self.graphql_query(CONTRIBUTED_REPOS_QUERY, &variables)?;
            let (repos, page_info, page_node_count) =
                parse_repo_connection_data(data, username, RepoConnectionKind::Contributed, true)?;

            all_repos.extend(repos);

            fetched_count += page_node_count;
            match pagination_decision(
                page_info.has_next_page,
                page_info.end_cursor.as_deref(),
                fetched_count,
                300,
                "contributed repositories",
            )? {
                PaginationDecision::Stop => break,
                PaginationDecision::Continue(cursor) => after = Some(cursor),
                PaginationDecision::Capped(warning) => {
                    eprintln!("Warning: {warning}");
                    break;
                }
            }
        }

        Ok(all_repos)
    }

    pub fn list_viewer_private_repos(&self) -> anyhow::Result<(String, Vec<RepoWithLangs>)> {
        let mut all_repos = Vec::new();
        let mut after: Option<String> = None;
        let mut viewer_login = String::new();
        let mut pages = 0usize;
        const MAX_PAGES: usize = 10; // 10 * 100 = 1000 private repos cap

        loop {
            let variables = serde_json::json!({ "after": after });
            let data = self.graphql_query(VIEWER_PRIVATE_REPOS_QUERY, &variables)?;
            let response: GraphqlViewerPrivateReposData = serde_json::from_value(data)
                .context("failed to parse GraphQL viewer private repositories data")?;
            if viewer_login.is_empty() {
                viewer_login = response.viewer.login.clone();
            }
            let connection = response.viewer.repositories;
            for node in connection.nodes {
                all_repos.push(graphql_repo_node_to_repo_with_langs(node));
            }
            pages += 1;
            match pagination_decision(
                connection.page_info.has_next_page,
                connection.page_info.end_cursor.as_deref(),
                pages,
                MAX_PAGES,
                "private repositories",
            )? {
                PaginationDecision::Stop => break,
                PaginationDecision::Continue(cursor) => after = Some(cursor),
                PaginationDecision::Capped(warning) => {
                    eprintln!("Warning: {warning}");
                    break;
                }
            }
        }

        Ok((viewer_login, all_repos))
    }
}

fn parse_reset_at_wait(reset_at: &str) -> Option<u64> {
    let reset_time = chrono::DateTime::parse_from_rfc3339(reset_at).ok()?;
    let now = Utc::now();
    let wait = (reset_time.timestamp() - now.timestamp()).max(1) as u64;
    Some(wait)
}

#[derive(Clone)]
pub struct RepoContribution {
    #[allow(dead_code)]
    pub repo_name: String,
    #[allow(dead_code)]
    pub total_commits: u64,
    #[allow(dead_code)]
    pub total_additions: u64,
    #[allow(dead_code)]
    pub total_deletions: u64,
    pub commits: Vec<CommitData>,
    #[allow(dead_code)]
    pub weeks: Vec<ContributorWeek>,
    pub languages: HashMap<String, u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContributionSummary {
    pub total_prs: u64,
    pub total_reviews: u64,
    pub total_issues: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoWithLangs {
    pub owner: String,
    pub name: String,
    #[allow(dead_code)]
    pub is_fork: bool,
    pub languages: HashMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlUserResponse {
    user: Option<GraphqlUserNode>,
}

#[derive(Debug, Deserialize)]
struct GraphqlUserNode {
    #[serde(rename = "id")]
    node_id: String,
    login: String,
    name: Option<String>,
    bio: Option<String>,
    #[serde(rename = "publicRepositories")]
    public_repositories: GraphqlTotalCount,
    followers: GraphqlTotalCount,
    following: GraphqlTotalCount,
    #[serde(rename = "avatarUrl")]
    avatar_url: String,
    #[serde(rename = "url")]
    html_url: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlTotalCount {
    #[serde(rename = "totalCount")]
    total_count: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphqlOwnedReposData {
    user: Option<GraphqlOwnedReposUser>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphqlOwnedReposUser {
    repositories: GraphqlRepoConnection,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphqlContributedReposData {
    user: Option<GraphqlContributedReposUser>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphqlContributedReposUser {
    #[serde(rename = "repositoriesContributedTo")]
    repositories_contributed_to: GraphqlRepoConnection,
}

#[derive(Debug, Deserialize)]
struct GraphqlViewerPrivateReposData {
    viewer: GraphqlViewerPrivateReposUser,
}

#[derive(Debug, Deserialize)]
struct GraphqlViewerPrivateReposUser {
    login: String,
    repositories: GraphqlRepoConnection,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphqlRepoConnection {
    #[serde(rename = "pageInfo")]
    page_info: GraphqlPageInfo,
    nodes: Vec<GraphqlRepoNode>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphqlPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphqlRepoNode {
    name: String,
    owner: GraphqlOwner,
    #[serde(rename = "isFork")]
    is_fork: bool,
    languages: GraphqlLanguages,
}

#[derive(Debug, Deserialize)]
struct GraphqlOwner {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlLanguages {
    edges: Vec<GraphqlLanguageEdge>,
}

#[derive(Debug, Deserialize)]
struct GraphqlLanguageEdge {
    size: u64,
    node: GraphqlLanguageNode,
}

#[derive(Debug, Deserialize)]
struct GraphqlLanguageNode {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlContributionsData {
    user: Option<GraphqlContributionsUser>,
}

#[derive(Debug, Deserialize)]
struct GraphqlContributionsUser {
    #[serde(rename = "contributionsCollection")]
    contributions_collection: GraphqlContributionsCollection,
}

#[derive(Debug, Deserialize)]
struct GraphqlContributionsCollection {
    #[serde(rename = "commitContributionsByRepository")]
    commit_contributions_by_repository: Vec<GraphqlContributionByRepository>,
    #[serde(rename = "totalPullRequestContributions", default)]
    total_pull_request_contributions: u64,
    #[serde(rename = "totalPullRequestReviewContributions", default)]
    total_pull_request_review_contributions: u64,
    #[serde(rename = "totalIssueContributions", default)]
    total_issue_contributions: u64,
}

#[derive(Debug, Deserialize)]
struct GraphqlContributionByRepository {
    repository: GraphqlRepoNode,
    contributions: GraphqlTotalCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitData {
    #[serde(default)]
    pub oid: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub committed_date: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlHistoryNode {
    #[serde(default)]
    oid: Option<String>,
    additions: u64,
    deletions: u64,
    #[serde(rename = "committedDate")]
    committed_date: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlHistoryConnection {
    nodes: Option<Vec<GraphqlHistoryNode>>,
    #[serde(rename = "totalCount")]
    total_count: u64,
    #[serde(rename = "pageInfo", default)]
    page_info: GraphqlHistoryPageInfo,
}

#[derive(Debug, Deserialize, Default)]
struct GraphqlHistoryPageInfo {
    #[serde(rename = "hasNextPage", default)]
    has_next_page: bool,
    #[serde(rename = "endCursor", default)]
    end_cursor: Option<String>,
}

struct ParsedBatchHistoryRepo {
    commits: Vec<CommitData>,
    total_count: u64,
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RepoHistoryRequest {
    pub owner: String,
    pub name: String,
    pub since: Option<String>,
    pub until_exclusive: Option<String>,
}

struct PageRequest {
    batch_index: usize,
    owner: String,
    name: String,
    since: Option<String>,
    until_exclusive: Option<String>,
    after: Option<String>,
}

struct BatchCommitHistory {
    commits: HashMap<String, Vec<CommitData>>,
    capped_repos: HashSet<String>,
}

#[allow(dead_code)]
enum RepoConnectionKind {
    Owned,
    Contributed,
}

#[derive(Debug, PartialEq, Eq)]
enum PaginationDecision {
    Stop,
    Continue(String),
    Capped(String),
}

fn pagination_decision(
    has_next_page: bool,
    end_cursor: Option<&str>,
    fetched: usize,
    cap: usize,
    scope: &str,
) -> anyhow::Result<PaginationDecision> {
    if !has_next_page {
        return Ok(PaginationDecision::Stop);
    }

    let cursor = end_cursor.with_context(|| {
        format!(
            "GitHub pagination response incomplete for {scope}: hasNextPage=true but endCursor is missing"
        )
    })?;

    if fetched >= cap {
        return Ok(PaginationDecision::Capped(format!(
            "GitHub pagination cap reached for {scope}: limit {cap}; returning partial data."
        )));
    }

    Ok(PaginationDecision::Continue(cursor.to_string()))
}

fn parse_graphql_response_payload(
    payload: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if let Some(errors) = payload.get("errors") {
        let err: Vec<GraphqlError> = serde_json::from_value(errors.clone())
            .context("failed to parse GraphQL errors response")?;
        let message = err
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!("GitHub GraphQL error: {message}");
    }

    payload
        .get("data")
        .cloned()
        .context("GitHub GraphQL response missing data field")
}

fn parse_graphql_user_data(data: serde_json::Value, username: &str) -> anyhow::Result<GithubUser> {
    let response: GraphqlUserResponse =
        serde_json::from_value(data).context("failed to parse GraphQL user data")?;

    let user = response
        .user
        .with_context(|| format!("GitHub user '{username}' not found."))?;

    Ok(GithubUser {
        node_id: user.node_id,
        login: user.login,
        name: user.name,
        bio: user.bio,
        public_repos: user.public_repositories.total_count,
        followers: user.followers.total_count,
        following: user.following.total_count,
        avatar_url: user.avatar_url,
        html_url: user.html_url,
        created_at: user.created_at,
    })
}

#[allow(dead_code)]
fn parse_repo_connection_data(
    data: serde_json::Value,
    username: &str,
    kind: RepoConnectionKind,
    include_forks: bool,
) -> anyhow::Result<(Vec<RepoWithLangs>, GraphqlPageInfo, usize)> {
    let connection = match kind {
        RepoConnectionKind::Owned => {
            let response: GraphqlOwnedReposData = serde_json::from_value(data)
                .context("failed to parse GraphQL repositories data")?;
            response
                .user
                .with_context(|| format!("GitHub user '{username}' not found."))?
                .repositories
        }
        RepoConnectionKind::Contributed => {
            let response: GraphqlContributedReposData = serde_json::from_value(data)
                .context("failed to parse GraphQL contributed repositories data")?;
            response
                .user
                .with_context(|| format!("GitHub user '{username}' not found."))?
                .repositories_contributed_to
        }
    };

    let page_node_count = connection.nodes.len();
    let repos = connection
        .nodes
        .into_iter()
        .filter(|node| include_forks || !node.is_fork)
        .map(graphql_repo_node_to_repo_with_langs)
        .collect();

    Ok((repos, connection.page_info, page_node_count))
}

fn graphql_repo_node_to_repo_with_langs(node: GraphqlRepoNode) -> RepoWithLangs {
    let mut languages = HashMap::new();
    for edge in node.languages.edges {
        *languages.entry(edge.node.name).or_insert(0) += edge.size;
    }

    RepoWithLangs {
        owner: node.owner.login,
        name: node.name,
        is_fork: node.is_fork,
        languages,
    }
}

fn parse_contributions_collection_data(
    data: serde_json::Value,
    username: &str,
) -> anyhow::Result<(Vec<(RepoWithLangs, u64)>, ContributionSummary)> {
    let response: GraphqlContributionsData =
        serde_json::from_value(data).context("failed to parse GraphQL contributions data")?;

    let user = response
        .user
        .with_context(|| format!("GitHub user '{username}' not found."))?;

    let GraphqlContributionsCollection {
        commit_contributions_by_repository,
        total_pull_request_contributions,
        total_pull_request_review_contributions,
        total_issue_contributions,
    } = user.contributions_collection;

    let summary = ContributionSummary {
        total_prs: total_pull_request_contributions,
        total_reviews: total_pull_request_review_contributions,
        total_issues: total_issue_contributions,
    };

    let repos = commit_contributions_by_repository
        .into_iter()
        .map(|entry| {
            (
                graphql_repo_node_to_repo_with_langs(entry.repository),
                entry.contributions.total_count,
            )
        })
        .collect();

    Ok((repos, summary))
}

fn parse_batch_history_data(
    data: serde_json::Value,
    active: &[PageRequest],
) -> anyhow::Result<HashMap<usize, ParsedBatchHistoryRepo>> {
    let obj = data
        .as_object()
        .context("batch history data should be a JSON object")?;

    let mut result = HashMap::new();
    for req in active {
        let alias = format!("repo{}", req.batch_index);
        let mut commits = Vec::new();
        let mut total_count = 0;
        let mut has_next_page = false;
        let mut end_cursor = None;

        if let Some(repo_value) = obj.get(&alias)
            && !repo_value.is_null()
        {
            let history_value = repo_value
                .pointer("/defaultBranchRef/target/history")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            if !history_value.is_null() {
                let history: GraphqlHistoryConnection = serde_json::from_value(history_value)
                    .with_context(|| {
                        format!(
                            "failed to parse commit history for {}/{}",
                            req.owner, req.name
                        )
                    })?;
                total_count = history.total_count;
                has_next_page = history.page_info.has_next_page;
                end_cursor = history.page_info.end_cursor;
                if let Some(nodes) = history.nodes {
                    commits = nodes
                        .into_iter()
                        .map(|node| CommitData {
                            oid: node.oid,
                            additions: node.additions,
                            deletions: node.deletions,
                            committed_date: node.committed_date,
                        })
                        .collect();
                }
            }
        }

        result.insert(
            req.batch_index,
            ParsedBatchHistoryRepo {
                commits,
                total_count,
                has_next_page,
                end_cursor,
            },
        );
    }

    Ok(result)
}

fn build_batch_history_query(active: &[PageRequest]) -> String {
    let mut query = String::from("query($userId: ID!");
    for req in active {
        query.push_str(&format!(
            ", $since{}: GitTimestamp, $until{}: GitTimestamp, $after{}: String",
            req.batch_index, req.batch_index, req.batch_index
        ));
    }
    query.push_str(") {\n  rateLimit { cost remaining resetAt }\n");

    for req in active {
        let owner_literal =
            serde_json::to_string(&req.owner).unwrap_or_else(|_| format!("\"{}\"", req.owner));
        let name_literal =
            serde_json::to_string(&req.name).unwrap_or_else(|_| format!("\"{}\"", req.name));
        let i = req.batch_index;
        query.push_str(&format!(
            "  repo{i}: repository(owner: {owner_literal}, name: {name_literal}) {{\n    defaultBranchRef {{\n      target {{\n        ... on Commit {{\n          history(author: {{id: $userId}}, since: $since{i}, until: $until{i}, first: 100, after: $after{i}) {{\n            pageInfo {{ hasNextPage endCursor }}\n            nodes {{ oid additions deletions committedDate }}\n            totalCount\n          }}\n        }}\n      }}\n    }}\n  }}\n"
        ));
    }

    query.push('}');
    query
}

fn build_batch_history_variables(user_node_id: &str, active: &[PageRequest]) -> serde_json::Value {
    let mut variables = serde_json::Map::new();
    variables.insert(
        "userId".to_string(),
        serde_json::Value::String(user_node_id.to_string()),
    );

    for req in active {
        variables.insert(
            format!("since{}", req.batch_index),
            req.since.as_ref().map_or(serde_json::Value::Null, |value| {
                serde_json::Value::String(value.clone())
            }),
        );
        variables.insert(
            format!("until{}", req.batch_index),
            req.until_exclusive
                .as_ref()
                .map_or(serde_json::Value::Null, |value| {
                    serde_json::Value::String(value.clone())
                }),
        );
        variables.insert(
            format!("after{}", req.batch_index),
            req.after.as_ref().map_or(serde_json::Value::Null, |value| {
                serde_json::Value::String(value.clone())
            }),
        );
    }

    serde_json::Value::Object(variables)
}

fn contribution_windows(
    from: DateTime<Utc>,
    until_exclusive: DateTime<Utc>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    if from >= until_exclusive {
        return Vec::new();
    }

    let mut windows = Vec::new();
    let mut window_start = from;

    while window_start < until_exclusive {
        let candidate_end = window_start + Duration::days(365);
        let window_end = if candidate_end < until_exclusive {
            candidate_end
        } else {
            until_exclusive
        };
        windows.push((window_start, window_end));
        window_start = window_end;
    }

    windows
}

fn contribution_query_variables(
    username: &str,
    from: &DateTime<Utc>,
    until_exclusive: &DateTime<Utc>,
) -> anyhow::Result<serde_json::Value> {
    let api_to = until_exclusive
        .checked_sub_signed(Duration::nanoseconds(1))
        .context("contribution window end cannot be represented as an inclusive API boundary")?;

    Ok(serde_json::json!({
        "login": username,
        "from": from.to_rfc3339(),
        "to": api_to.to_rfc3339(),
    }))
}

fn repo_key(owner: &str, name: &str) -> String {
    format!("{}/{}", owner.to_lowercase(), name.to_lowercase())
}

const CONTRIBUTION_CACHE_SCHEMA_VERSION: &str = "v4";
const HISTORY_CACHE_SCHEMA_VERSION: &str = "v4";
const HISTORY_MAX_PAGES_PER_REPO: usize = 20;

#[derive(Default)]
struct CacheWarnings {
    messages: Vec<String>,
}

impl CacheWarnings {
    fn push(&mut self, message: impl Into<String>) {
        self.messages.push(message.into());
    }
}

fn emit_cache_warnings(warnings: &mut CacheWarnings) {
    let mut emitted = HashSet::new();
    for warning in warnings.messages.drain(..) {
        if emitted.insert(warning.clone()) {
            eprintln!("Warning: {warning}");
        }
    }
}

#[cfg(test)]
fn cache_init_or_warn(
    cache: anyhow::Result<DiskCache>,
    warnings: &mut CacheWarnings,
) -> Option<DiskCache> {
    match cache {
        Ok(cache) => Some(cache),
        Err(error) => {
            warnings.push(format!("GitHub cache initialization failed: {error}"));
            None
        }
    }
}

pub fn initialize_cache_for_policy<F, E>(policy: CachePolicy, factory: F) -> Option<DiskCache>
where
    F: FnOnce() -> Result<DiskCache, E>,
{
    if matches!(policy, CachePolicy::Disabled) {
        None
    } else {
        factory().ok()
    }
}

fn initialize_cache_for_policy_or_warn(
    policy: CachePolicy,
    warnings: &mut CacheWarnings,
) -> Option<DiskCache> {
    initialize_cache_for_policy(policy, || {
        DiskCache::new().map_err(|error| {
            warnings.push(format!("GitHub cache initialization failed: {error}"));
        })
    })
}

fn cache_get_or_warn<T: serde::de::DeserializeOwned>(
    cache: &DiskCache,
    key: &str,
    warnings: &mut CacheWarnings,
) -> Option<T> {
    match cache.get(key) {
        Ok(cached) => cached,
        Err(error) => {
            warnings.push(format!("GitHub cache read failed for key '{key}': {error}"));
            None
        }
    }
}

fn cache_set_or_warn<T: Serialize>(
    cache: &DiskCache,
    key: &str,
    value: &T,
    warnings: &mut CacheWarnings,
) {
    if let Err(error) = cache.set(key, value) {
        warnings.push(format!(
            "GitHub cache write failed for key '{key}': {error}"
        ));
    }
}

fn cache_string_component(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn cache_window_scope_component(scope: &CacheWindowScope) -> String {
    match scope {
        CacheWindowScope::Fixed {
            from,
            until_exclusive,
        } => format!(
            "fixed_{}_{}",
            cache_string_component(&from.to_rfc3339()),
            cache_string_component(&until_exclusive.to_rfc3339())
        ),
        CacheWindowScope::Rolling {
            lookback_nanoseconds,
        } => format!("rolling_{lookback_nanoseconds}"),
        CacheWindowScope::Anchored { from } => {
            format!("anchored_{}", cache_string_component(&from.to_rfc3339()))
        }
    }
}

fn contribution_cache_key(
    user_node_id: &str,
    username: &str,
    include_forks: bool,
    include_contributed: bool,
    include_private: bool,
    scope: &CacheWindowScope,
) -> String {
    let scope = cache_window_scope_component(scope);
    format!(
        "{CONTRIBUTION_CACHE_SCHEMA_VERSION}_contribution_{}_{}_forks_{}_contributed_{}_private_{}_{}",
        cache_string_component(user_node_id),
        cache_string_component(&username.to_ascii_lowercase()),
        include_forks as u8,
        include_contributed as u8,
        include_private as u8,
        scope,
    )
}

fn history_cache_key(
    user_node_id: &str,
    owner: &str,
    name: &str,
    include_private: bool,
    scope: &CacheWindowScope,
) -> String {
    format!(
        "{HISTORY_CACHE_SCHEMA_VERSION}_history_{}_{}_{}_private_{}_{}",
        cache_string_component(user_node_id),
        cache_string_component(&owner.to_ascii_lowercase()),
        cache_string_component(&name.to_ascii_lowercase()),
        include_private as u8,
        cache_window_scope_component(scope),
    )
}

fn normalize_week_start(ts: i64) -> Option<i64> {
    let dt = Utc.timestamp_opt(ts, 0).single()?;
    let weekday = i64::from(dt.weekday().num_days_from_monday());
    let monday_date = dt.date_naive() - Duration::days(weekday);
    let monday = monday_date.and_hms_opt(0, 0, 0)?.and_utc();
    Some(monday.timestamp())
}

fn commits_to_weekly_buckets(commits: &[CommitData]) -> Vec<ContributorWeek> {
    let mut buckets: HashMap<i64, ContributorWeek> = HashMap::new();

    for commit in commits {
        let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&commit.committed_date) else {
            continue;
        };
        let ts = dt.with_timezone(&Utc).timestamp();
        let Some(week_start) = normalize_week_start(ts) else {
            continue;
        };

        let bucket = buckets.entry(week_start).or_insert(ContributorWeek {
            w: week_start,
            a: 0,
            d: 0,
            c: 0,
            net_modifications: 0,
            net_additions: 0,
        });
        bucket.a += commit.additions;
        bucket.d += commit.deletions;
        bucket.c += 1;
        bucket.net_modifications += commit.additions.max(commit.deletions);
        bucket.net_additions += commit.additions.saturating_sub(commit.deletions);
    }

    let mut weeks: Vec<ContributorWeek> = buckets.into_values().collect();
    weeks.sort_by_key(|w| w.w);
    weeks
}

fn dedup_commits(mut commits: Vec<CommitData>) -> Vec<CommitData> {
    commits.sort_by(|a, b| a.committed_date.cmp(&b.committed_date));
    let mut seen_oids = HashSet::new();
    commits.retain(|commit| {
        commit
            .oid
            .as_deref()
            .filter(|oid| !oid.is_empty())
            .is_none_or(|oid| seen_oids.insert(oid.to_string()))
    });
    commits
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEnvelope<T> {
    pub requested_from: DateTime<Utc>,
    pub checked_until: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub completeness: Completeness,
    pub payload: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Completeness {
    Complete,
    Incomplete(Vec<IncompleteReason>),
}

impl Completeness {
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }

    fn visible_warning(&self) -> Option<String> {
        let Self::Incomplete(reasons) = self else {
            return None;
        };
        let reasons = reasons
            .iter()
            .map(IncompleteReason::description)
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!("GitHub data may be incomplete: {reasons}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IncompleteReason {
    ContributionRepositoryLimit { limit: usize },
    HistoryPageLimit { repository: String, pages: usize },
}

impl IncompleteReason {
    fn description(&self) -> String {
        match self {
            Self::ContributionRepositoryLimit { limit } => {
                format!("commitContributionsByRepository reached its {limit}-repository limit")
            }
            Self::HistoryPageLimit { repository, pages } => {
                format!("commit history for {repository} reached its {pages}-page limit")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedContributionPayload {
    pub repos: Vec<(RepoWithLangs, u64)>,
    pub summary: ContributionSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContributionCacheDecision {
    Hit,
    FullFetch,
}

#[derive(Debug, Clone)]
enum HistoryFetchPlan {
    Hit {
        commits: Vec<CommitData>,
    },
    Full {
        request: RepoHistoryRequest,
    },
    Gap {
        retained: Vec<CommitData>,
        request: RepoHistoryRequest,
    },
}

fn filter_commits_to_range(
    commits: &[CommitData],
    since: Option<&str>,
    until_exclusive: Option<&str>,
) -> anyhow::Result<Vec<CommitData>> {
    let since = since
        .map(|value| parse_rfc3339_instant(value, "history range start"))
        .transpose()?;
    let until_exclusive = until_exclusive
        .map(|value| parse_rfc3339_instant(value, "history range end"))
        .transpose()?;
    let mut filtered = Vec::new();

    for commit in commits {
        let committed_at = parse_rfc3339_instant(&commit.committed_date, "commit committedDate")?;
        if since.as_ref().is_none_or(|start| committed_at >= *start)
            && until_exclusive
                .as_ref()
                .is_none_or(|end| committed_at < *end)
        {
            filtered.push(commit.clone());
        }
    }

    Ok(filtered)
}

fn parse_rfc3339_instant(value: &str, context: &str) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("{context} '{value}' is not a valid RFC3339 timestamp"))
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn contribution_window_is_saturated(repos: &[(RepoWithLangs, u64)]) -> bool {
    repos.len() >= 100
}

fn validate_query_window_scope(query_window: &QueryWindow) -> anyhow::Result<()> {
    if query_window.requested_from > query_window.until_exclusive {
        anyhow::bail!("contribution query start is after its end");
    }

    match &query_window.scope {
        CacheWindowScope::Fixed {
            from,
            until_exclusive,
        } => {
            if !query_window.completed
                || *from != query_window.requested_from
                || *until_exclusive != query_window.until_exclusive
                || *until_exclusive > query_window.observed_at
            {
                anyhow::bail!(
                    "fixed contribution cache scope is incompatible with its query bounds"
                );
            }
        }
        CacheWindowScope::Rolling {
            lookback_nanoseconds,
        } => {
            if query_window.completed
                || query_window.until_exclusive != query_window.observed_at
                || *lookback_nanoseconds < 0
            {
                anyhow::bail!(
                    "rolling contribution cache scope is incompatible with its query bounds"
                );
            }
            let lookback = Duration::nanoseconds(*lookback_nanoseconds);
            let expected_from = query_window
                .observed_at
                .checked_sub_signed(lookback)
                .context("rolling contribution cache scope overflows its query bounds")?;
            if expected_from != query_window.requested_from {
                anyhow::bail!("rolling contribution cache scope does not match its query start");
            }
        }
        CacheWindowScope::Anchored { from } => {
            if query_window.completed
                || *from != query_window.requested_from
                || query_window.until_exclusive != query_window.observed_at
            {
                anyhow::bail!(
                    "anchored contribution cache scope is incompatible with its query bounds"
                );
            }
        }
    }

    Ok(())
}

fn validate_envelope_bounds<T>(
    envelope: &CacheEnvelope<T>,
    query_window: &QueryWindow,
) -> anyhow::Result<ContributionCacheDecision> {
    validate_query_window_scope(query_window)?;
    if envelope.requested_from > envelope.checked_until {
        anyhow::bail!("cached contribution start is after its checked end");
    }
    if envelope.checked_until > envelope.observed_at {
        anyhow::bail!("cached contribution checked end is after its observation time");
    }

    if !query_window.completed {
        if envelope.observed_at > query_window.observed_at {
            anyhow::bail!(
                "cached contribution observation time is after the current open-window clock"
            );
        }
        if envelope.checked_until != envelope.observed_at {
            anyhow::bail!(
                "cached open-window contribution payload extends beyond its checked coverage"
            );
        }
    }

    if envelope.requested_from == query_window.requested_from
        && envelope.checked_until == query_window.until_exclusive
    {
        Ok(ContributionCacheDecision::Hit)
    } else {
        Ok(ContributionCacheDecision::FullFetch)
    }
}

fn contribution_cache_get_or_warn(
    cache: &DiskCache,
    key: &str,
    query_window: &QueryWindow,
    warnings: &mut CacheWarnings,
) -> Option<(
    ContributionCacheDecision,
    CacheEnvelope<CachedContributionPayload>,
)> {
    let cached = cache_get_or_warn(cache, key, warnings)?;
    match validate_envelope_bounds(&cached, query_window) {
        Ok(decision) => Some((decision, cached)),
        Err(error) => {
            warnings.push(format!(
                "GitHub contribution cache entry for key '{key}' is invalid; treating it as a cache miss: {error}"
            ));
            None
        }
    }
}

fn validate_cached_history_envelope(
    envelope: &CacheEnvelope<Vec<CommitData>>,
    query_window: &QueryWindow,
) -> anyhow::Result<()> {
    validate_query_window_scope(query_window)?;
    if envelope.requested_from > envelope.checked_until {
        anyhow::bail!("cached history start is after its checked end");
    }
    if envelope.checked_until > envelope.observed_at {
        anyhow::bail!("cached history checked end is after its observation time");
    }

    match &query_window.scope {
        CacheWindowScope::Fixed {
            from,
            until_exclusive,
        } => {
            if envelope.requested_from != *from || envelope.checked_until != *until_exclusive {
                anyhow::bail!("cached fixed history bounds are incompatible with its scope");
            }
        }
        CacheWindowScope::Rolling {
            lookback_nanoseconds,
        } => {
            let lookback = Duration::nanoseconds(*lookback_nanoseconds);
            let expected_from = envelope
                .observed_at
                .checked_sub_signed(lookback)
                .context("rolling history cache scope overflows its query bounds")?;
            if envelope.requested_from != expected_from {
                anyhow::bail!("cached rolling history scope does not match its query start");
            }
        }
        CacheWindowScope::Anchored { from } => {
            if envelope.requested_from != *from {
                anyhow::bail!("cached anchored history scope does not match its query start");
            }
        }
    }

    if !query_window.completed && envelope.observed_at > query_window.observed_at {
        anyhow::bail!("cached history observation time is after the current open-window clock");
    }

    for commit in &envelope.payload {
        let committed_at =
            parse_rfc3339_instant(&commit.committed_date, "cached commit committedDate")?;
        if committed_at < envelope.requested_from || committed_at >= envelope.checked_until {
            anyhow::bail!("cached commit committedDate is outside the envelope coverage");
        }
    }

    Ok(())
}

fn history_cache_get_or_warn(
    cache: &DiskCache,
    key: &str,
    query_window: &QueryWindow,
    warnings: &mut CacheWarnings,
) -> Option<CacheEnvelope<Vec<CommitData>>> {
    let cached = cache_get_or_warn(cache, key, warnings)?;
    if let Err(error) = validate_cached_history_envelope(&cached, query_window) {
        warnings.push(format!(
            "GitHub history cache entry for key '{key}' is invalid; treating it as a cache miss: {error}"
        ));
        return None;
    }
    if let Some(warning) = cached.completeness.visible_warning() {
        warnings.push(warning);
    }
    Some(cached)
}

fn history_full_fetch_request(
    query_window: &QueryWindow,
    owner: &str,
    name: &str,
) -> RepoHistoryRequest {
    RepoHistoryRequest {
        owner: owner.to_string(),
        name: name.to_string(),
        since: Some(query_window.requested_from.to_rfc3339()),
        until_exclusive: Some(query_window.until_exclusive.to_rfc3339()),
    }
}

fn plan_history_refresh(
    cached: Option<CacheEnvelope<Vec<CommitData>>>,
    query_window: &QueryWindow,
    cache_policy: CachePolicy,
    owner: &str,
    name: &str,
) -> anyhow::Result<HistoryFetchPlan> {
    let full_fetch = || HistoryFetchPlan::Full {
        request: history_full_fetch_request(query_window, owner, name),
    };

    if !cache_policy.can_read() {
        return Ok(full_fetch());
    }
    let Some(cached) = cached else {
        return Ok(full_fetch());
    };
    if !cached.completeness.is_complete()
        || validate_cached_history_envelope(&cached, query_window).is_err()
    {
        return Ok(full_fetch());
    }

    if query_window.completed {
        if cached.requested_from != query_window.requested_from
            || cached.checked_until != query_window.until_exclusive
        {
            return Ok(full_fetch());
        }
        return Ok(HistoryFetchPlan::Hit {
            commits: dedup_commits(filter_commits_to_range(
                &cached.payload,
                Some(&query_window.requested_from.to_rfc3339()),
                Some(&query_window.until_exclusive.to_rfc3339()),
            )?),
        });
    }

    if cached.requested_from > query_window.requested_from {
        return Ok(full_fetch());
    }

    let retained = dedup_commits(filter_commits_to_range(
        &cached.payload,
        Some(&query_window.requested_from.to_rfc3339()),
        Some(&query_window.until_exclusive.to_rfc3339()),
    )?);
    if cached.checked_until < query_window.until_exclusive {
        return Ok(HistoryFetchPlan::Gap {
            retained,
            request: RepoHistoryRequest {
                owner: owner.to_string(),
                name: name.to_string(),
                since: Some(cached.checked_until.to_rfc3339()),
                until_exclusive: Some(query_window.until_exclusive.to_rfc3339()),
            },
        });
    }

    Ok(HistoryFetchPlan::Hit { commits: retained })
}

fn finish_history_fetch(
    plan: HistoryFetchPlan,
    fetched: Vec<CommitData>,
    query_window: &QueryWindow,
    completeness: Completeness,
) -> anyhow::Result<CacheEnvelope<Vec<CommitData>>> {
    let (mut retained, request) = match plan {
        HistoryFetchPlan::Full { request } => (Vec::new(), request),
        HistoryFetchPlan::Gap { retained, request } => (retained, request),
        HistoryFetchPlan::Hit { .. } => anyhow::bail!("cannot finish a history cache hit"),
    };
    let fetched = filter_commits_to_range(
        &fetched,
        request.since.as_deref(),
        request.until_exclusive.as_deref(),
    )?;
    retained.extend(fetched);
    let payload = dedup_commits(filter_commits_to_range(
        &retained,
        Some(&query_window.requested_from.to_rfc3339()),
        Some(&query_window.until_exclusive.to_rfc3339()),
    )?);

    Ok(CacheEnvelope {
        requested_from: query_window.requested_from,
        checked_until: query_window.until_exclusive,
        observed_at: query_window.observed_at,
        completeness,
        payload,
    })
}

fn history_cache_write_allowed(
    request: &RepoHistoryRequest,
    capped_repos: &HashSet<String>,
) -> bool {
    !capped_repos.contains(&repo_key(&request.owner, &request.name))
}

struct ContributionCacheRequest<'a> {
    user_node_id: &'a str,
    username: &'a str,
    include_forks: bool,
    include_contributed: bool,
    include_private: bool,
    query_window: &'a QueryWindow,
    cache_policy: CachePolicy,
}

fn get_contribution_repos_cached(
    client: &GithubClient,
    cache: &DiskCache,
    request: ContributionCacheRequest<'_>,
    warnings: &mut CacheWarnings,
) -> anyhow::Result<(Vec<(RepoWithLangs, u64)>, ContributionSummary)> {
    validate_query_window_scope(request.query_window)?;
    let key = contribution_cache_key(
        request.user_node_id,
        request.username,
        request.include_forks,
        request.include_contributed,
        request.include_private,
        &request.query_window.scope,
    );
    let cached = request
        .cache_policy
        .can_read()
        .then(|| contribution_cache_get_or_warn(cache, &key, request.query_window, warnings))
        .flatten();

    let (payload, completeness) = match cached {
        Some((ContributionCacheDecision::Hit, envelope)) => {
            (envelope.payload, envelope.completeness)
        }
        Some((ContributionCacheDecision::FullFetch, _)) | None => {
            let (payload, completeness) = client.get_contribution_payload(
                request.username,
                request.query_window.requested_from,
                request.query_window.until_exclusive,
                request.include_forks,
                request.include_contributed,
            )?;
            if request.cache_policy.can_write() {
                cache_set_or_warn(
                    cache,
                    &key,
                    &CacheEnvelope {
                        requested_from: request.query_window.requested_from,
                        checked_until: request.query_window.until_exclusive,
                        observed_at: request.query_window.observed_at,
                        completeness: completeness.clone(),
                        payload: payload.clone(),
                    },
                    warnings,
                );
            }
            (payload, completeness)
        }
    };

    if !completeness.is_complete()
        && let Some(warning) = completeness.visible_warning()
    {
        warnings.push(warning);
    }
    Ok((payload.repos, payload.summary))
}

#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub fn fetch_user_stats(
    client: &GithubClient,
    user_node_id: &str,
    username: &str,
    include_forks: bool,
    include_contributed: bool,
    include_private: bool,
    query_window: &QueryWindow,
    cache_policy: CachePolicy,
) -> anyhow::Result<(Vec<RepoContribution>, ContributionSummary)> {
    let mut cache_warnings = CacheWarnings::default();
    let cache = initialize_cache_for_policy_or_warn(cache_policy, &mut cache_warnings);
    let now = query_window.until_exclusive.min(query_window.observed_at);
    let from = query_window.requested_from;

    let (mut repo_rows, contribution_summary) = if let Some(cache) = cache.as_ref() {
        get_contribution_repos_cached(
            client,
            cache,
            ContributionCacheRequest {
                user_node_id,
                username,
                include_forks,
                include_contributed,
                include_private,
                query_window,
                cache_policy,
            },
            &mut cache_warnings,
        )?
    } else {
        client.get_contribution_repos(username, from, now, include_forks, include_contributed)?
    };

    if include_private {
        match client.list_viewer_private_repos() {
            Ok((viewer_login, private_repos)) => {
                if !viewer_login.eq_ignore_ascii_case(username) {
                    eprintln!(
                        "Warning: --include-private uses the token holder ('{viewer_login}'), but you queried '{username}'. Skipping private-repo merge.",
                    );
                } else {
                    let existing_keys: std::collections::HashSet<String> = repo_rows
                        .iter()
                        .map(|(r, _)| repo_key(&r.owner, &r.name))
                        .collect();
                    let mut added = 0usize;
                    for repo in private_repos {
                        if !include_forks && repo.is_fork {
                            continue;
                        }
                        let key = repo_key(&repo.owner, &repo.name);
                        if existing_keys.contains(&key) {
                            continue;
                        }
                        repo_rows.push((repo, 0));
                        added += 1;
                    }
                    if added > 0 {
                        eprintln!("Added {added} private repo(s) via --include-private");
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to enumerate private repos: {e}");
            }
        }
    }

    eprintln!("Found {} repos with contributions", repo_rows.len());

    let mut commit_history_by_repo: HashMap<String, Vec<CommitData>> = HashMap::new();
    let mut to_fetch: Vec<RepoHistoryRequest> = Vec::new();
    let mut history_fetch_plans: HashMap<String, (String, HistoryFetchPlan)> = HashMap::new();

    for (repo, _) in &repo_rows {
        let history_key = history_cache_key(
            user_node_id,
            &repo.owner,
            &repo.name,
            include_private,
            &query_window.scope,
        );
        let repo_name = format!("{}/{}", repo.owner, repo.name);

        let cached: Option<CacheEnvelope<Vec<CommitData>>> = if cache_policy.can_read() {
            cache.as_ref().and_then(|c| {
                history_cache_get_or_warn(c, &history_key, query_window, &mut cache_warnings)
            })
        } else {
            None
        };

        let plan =
            plan_history_refresh(cached, query_window, cache_policy, &repo.owner, &repo.name)?;
        match plan {
            HistoryFetchPlan::Hit { commits } => {
                commit_history_by_repo.insert(repo_name, commits);
            }
            plan => {
                let request = match &plan {
                    HistoryFetchPlan::Full { request } | HistoryFetchPlan::Gap { request, .. } => {
                        request.clone()
                    }
                    HistoryFetchPlan::Hit { .. } => {
                        unreachable!("history cache hits are handled above")
                    }
                };
                to_fetch.push(request);
                history_fetch_plans.insert(repo_name, (history_key, plan));
            }
        }
    }

    if !to_fetch.is_empty() {
        for batch in to_fetch.chunks(5) {
            let fetched = client.batch_commit_history(user_node_id, batch)?;

            for request in batch {
                let repo_name = format!("{}/{}", request.owner, request.name);
                let (history_key, plan) =
                    history_fetch_plans.remove(&repo_name).with_context(|| {
                        format!(
                            "missing history refresh plan for fetched repository {}/{}",
                            request.owner, request.name
                        )
                    })?;
                let completeness = if fetched
                    .capped_repos
                    .contains(&repo_key(&request.owner, &request.name))
                {
                    Completeness::Incomplete(vec![IncompleteReason::HistoryPageLimit {
                        repository: repo_name.clone(),
                        pages: HISTORY_MAX_PAGES_PER_REPO,
                    }])
                } else {
                    Completeness::Complete
                };
                let envelope = finish_history_fetch(
                    plan,
                    fetched.commits.get(&repo_name).cloned().unwrap_or_default(),
                    query_window,
                    completeness,
                )?;

                if cache_policy.can_write()
                    && history_cache_write_allowed(request, &fetched.capped_repos)
                    && envelope.completeness.is_complete()
                    && let Some(c) = &cache
                {
                    cache_set_or_warn(c, &history_key, &envelope, &mut cache_warnings);
                }
                commit_history_by_repo.insert(repo_name, envelope.payload);
            }
        }
    }

    let mut contributions = Vec::new();
    for (repo, repo_total_commits) in repo_rows {
        let repo_name = format!("{}/{}", repo.owner, repo.name);
        let commits = commit_history_by_repo
            .remove(&repo_name)
            .unwrap_or_default();

        let weeks = commits_to_weekly_buckets(&commits);
        let total_additions: u64 = commits.iter().map(|commit| commit.additions).sum();
        let total_deletions: u64 = commits.iter().map(|commit| commit.deletions).sum();

        if repo_total_commits == 0 && total_additions == 0 && total_deletions == 0 {
            continue;
        }

        contributions.push(RepoContribution {
            repo_name,
            total_commits: repo_total_commits,
            total_additions,
            total_deletions,
            commits,
            weeks,
            languages: repo.languages,
        });
    }

    emit_cache_warnings(&mut cache_warnings);
    Ok((contributions, contribution_summary))
}

/// Distribute `total` among buckets proportional to `shares` using
/// largest-remainder (Hamilton) apportionment so the parts sum exactly to `total`.
fn apportion(total: u64, shares: &[f64]) -> Vec<u64> {
    if shares.is_empty() {
        return Vec::new();
    }
    let share_sum: f64 = shares.iter().sum();
    if share_sum == 0.0 {
        let mut result = vec![0u64; shares.len()];
        // Give everything to the first bucket to preserve the total
        result[0] = total;
        return result;
    }

    let total_f = total as f64;
    let quotas: Vec<f64> = shares.iter().map(|s| total_f * s / share_sum).collect();
    let mut floors: Vec<u64> = quotas.iter().map(|q| *q as u64).collect();
    let floor_sum: u64 = floors.iter().sum();
    let mut remainder = total.saturating_sub(floor_sum);

    if remainder > 0 {
        // Sort indices by fractional part descending, tie-break by index ascending
        let mut indices: Vec<usize> = (0..quotas.len()).collect();
        indices.sort_by(|&a, &b| {
            let fa = quotas[a] - (quotas[a] as u64) as f64;
            let fb = quotas[b] - (quotas[b] as u64) as f64;
            fb.partial_cmp(&fa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        for &idx in &indices {
            if remainder == 0 {
                break;
            }
            floors[idx] += 1;
            remainder -= 1;
        }
    }

    floors
}

/// Count the grouping dimensions represented by exact GitHub contribution data.
/// GitHub contribution records deliberately have no author dimension.
pub fn contribution_group_cardinality(
    contributions: &[RepoContribution],
    period: &Period,
) -> crate::stats::aggregator::GroupCardinality {
    let mut repos = HashSet::new();
    let mut periods = HashSet::new();
    let mut languages = HashSet::new();

    for contribution in contributions {
        repos.insert(contribution.repo_name.as_str());
        languages.extend(contribution.languages.keys().cloned());
        for commit in &contribution.commits {
            if let Ok(committed_at) = DateTime::parse_from_rfc3339(&commit.committed_date) {
                periods.insert(crate::stats::aggregator::bucket_timestamp(
                    &committed_at.with_timezone(&Utc),
                    period,
                ));
            }
            if contribution.languages.is_empty() && (commit.additions > 0 || commit.deletions > 0) {
                languages.insert("Other".to_string());
            }
        }
    }

    crate::stats::aggregator::GroupCardinality {
        repo: repos.len(),
        author: 0,
        period: periods.len(),
        language: languages.len(),
    }
}

/// Build a hierarchical group tree from exact GitHub commits. `Language` is a
/// final display level and remains in each leaf's `by_language` breakdown,
/// matching local tree semantics.
pub fn contributions_to_group_tree(
    contributions: &[RepoContribution],
    levels: &[GroupBy],
    period: &Period,
) -> Vec<GroupNode> {
    let effective_levels = if levels.len() > 1 && matches!(levels.last(), Some(GroupBy::Language)) {
        &levels[..levels.len() - 1]
    } else {
        levels
    };
    if effective_levels.is_empty() {
        return Vec::new();
    }

    let mut nodes = contributions_to_group_tree_inner(contributions, effective_levels, period);
    prune_empty_group_nodes(&mut nodes);
    nodes
}

fn contributions_to_group_tree_inner(
    contributions: &[RepoContribution],
    levels: &[GroupBy],
    period: &Period,
) -> Vec<GroupNode> {
    let current_group = levels[0];
    let remaining = &levels[1..];

    if remaining.is_empty() {
        let stats = match current_group {
            GroupBy::Repo => contributions_to_repo_stats(contributions),
            GroupBy::Period | GroupBy::Language => {
                contributions_to_period_stats(contributions, period)
            }
            GroupBy::Author => Vec::new(),
        };
        let mut nodes: Vec<GroupNode> = stats
            .into_iter()
            .map(|stats| GroupNode {
                label: stats.period_label.clone(),
                stats,
                children: Vec::new(),
            })
            .collect();
        nodes.sort_by(|left, right| left.label.cmp(&right.label));
        return nodes;
    }

    let mut partitions: HashMap<String, Vec<RepoContribution>> = HashMap::new();
    match current_group {
        GroupBy::Repo => {
            for contribution in contributions {
                partitions
                    .entry(contribution.repo_name.clone())
                    .or_default()
                    .push(contribution.clone());
            }
        }
        GroupBy::Period => {
            for contribution in contributions {
                for commit in &contribution.commits {
                    let Ok(committed_at) = DateTime::parse_from_rfc3339(&commit.committed_date)
                    else {
                        continue;
                    };
                    let label = crate::stats::aggregator::bucket_timestamp(
                        &committed_at.with_timezone(&Utc),
                        period,
                    );
                    let partition = partitions.entry(label).or_default();
                    if let Some(existing) = partition
                        .iter_mut()
                        .find(|existing| existing.repo_name == contribution.repo_name)
                    {
                        existing.commits.push(commit.clone());
                    } else {
                        partition.push(RepoContribution {
                            repo_name: contribution.repo_name.clone(),
                            total_commits: contribution.total_commits,
                            total_additions: contribution.total_additions,
                            total_deletions: contribution.total_deletions,
                            commits: vec![commit.clone()],
                            weeks: Vec::new(),
                            languages: contribution.languages.clone(),
                        });
                    }
                }
            }
        }
        GroupBy::Author | GroupBy::Language => return Vec::new(),
    }

    let mut nodes: Vec<GroupNode> = partitions
        .into_iter()
        .map(|(label, partition)| {
            let children = contributions_to_group_tree_inner(&partition, remaining, period);
            let child_stats: Vec<PeriodStats> =
                children.iter().map(|child| child.stats.clone()).collect();
            GroupNode {
                label,
                stats: crate::stats::aggregator::aggregate_totals(&child_stats),
                children,
            }
        })
        .collect();
    nodes.sort_by(|left, right| left.label.cmp(&right.label));
    nodes
}

fn prune_empty_group_nodes(nodes: &mut Vec<GroupNode>) {
    for node in nodes.iter_mut() {
        prune_empty_group_nodes(&mut node.children);
    }
    nodes.retain(|node| node.stats.total_commits > 0);
}

pub fn contributions_to_period_stats(
    contributions: &[RepoContribution],
    period: &Period,
) -> Vec<crate::stats::models::PeriodStats> {
    let mut buckets: HashMap<String, PeriodStats> = HashMap::new();

    for contrib in contributions {
        let lang_total_bytes: u64 = contrib.languages.values().sum();

        for commit in &contrib.commits {
            let Ok(committed_at) = DateTime::parse_from_rfc3339(&commit.committed_date) else {
                continue;
            };
            let committed_at = committed_at.with_timezone(&Utc);
            let label = crate::stats::aggregator::bucket_timestamp(&committed_at, period);

            let entry = buckets.entry(label.clone()).or_insert_with(|| PeriodStats {
                period_label: label,
                by_language: HashMap::new(),
                by_author: HashMap::new(),
                total_commits: 0,
                total_additions: 0,
                total_deletions: 0,
                total_net_modifications: 0,
                total_net_additions: 0,
            });

            entry.total_commits += 1;
            entry.total_additions += commit.additions;
            entry.total_deletions += commit.deletions;
            entry.total_net_modifications += commit.additions.max(commit.deletions);
            entry.total_net_additions += commit.additions.saturating_sub(commit.deletions);

            if lang_total_bytes > 0 {
                let mut langs: Vec<(&String, &u64)> = contrib.languages.iter().collect();
                langs.sort_by(|a, b| a.0.cmp(b.0));
                let shares: Vec<f64> = langs.iter().map(|&(_, &b)| b as f64).collect();

                let a_parts = apportion(commit.additions, &shares);
                let d_parts = apportion(commit.deletions, &shares);
                let nm_parts = apportion(commit.additions.max(commit.deletions), &shares);
                let na_parts =
                    apportion(commit.additions.saturating_sub(commit.deletions), &shares);

                for (i, (lang, _)) in langs.iter().enumerate() {
                    let lang_entry = entry.by_language.entry((*lang).clone()).or_default();
                    lang_entry.additions += a_parts[i];
                    lang_entry.deletions += d_parts[i];
                    lang_entry.net_modifications += nm_parts[i];
                    lang_entry.net_additions += na_parts[i];
                    lang_entry.files_changed += 1;
                }
            } else if commit.additions > 0 || commit.deletions > 0 {
                let lang_entry = entry.by_language.entry("Other".to_string()).or_default();
                lang_entry.additions += commit.additions;
                lang_entry.deletions += commit.deletions;
                lang_entry.net_modifications += commit.additions.max(commit.deletions);
                lang_entry.net_additions += commit.additions.saturating_sub(commit.deletions);
                lang_entry.files_changed += 1;
            }
        }
    }

    let mut result: Vec<PeriodStats> = buckets.into_values().collect();
    result.sort_by(|a, b| a.period_label.cmp(&b.period_label));
    result
}

#[allow(dead_code)]
pub fn contributions_to_repo_stats(
    contributions: &[RepoContribution],
) -> Vec<crate::stats::models::PeriodStats> {
    let mut result = Vec::with_capacity(contributions.len());

    for contrib in contributions {
        let lang_total_bytes: u64 = contrib.languages.values().sum();
        let mut entry = PeriodStats {
            period_label: contrib.repo_name.clone(),
            by_language: HashMap::new(),
            by_author: HashMap::new(),
            total_commits: 0,
            total_additions: 0,
            total_deletions: 0,
            total_net_modifications: 0,
            total_net_additions: 0,
        };

        for commit in &contrib.commits {
            entry.total_commits += 1;
            entry.total_additions += commit.additions;
            entry.total_deletions += commit.deletions;
            entry.total_net_modifications += commit.additions.max(commit.deletions);
            entry.total_net_additions += commit.additions.saturating_sub(commit.deletions);

            if lang_total_bytes > 0 {
                let mut langs: Vec<(&String, &u64)> = contrib.languages.iter().collect();
                langs.sort_by(|a, b| a.0.cmp(b.0));
                let shares: Vec<f64> = langs.iter().map(|&(_, &b)| b as f64).collect();

                let a_parts = apportion(commit.additions, &shares);
                let d_parts = apportion(commit.deletions, &shares);
                let nm_parts = apportion(commit.additions.max(commit.deletions), &shares);
                let na_parts =
                    apportion(commit.additions.saturating_sub(commit.deletions), &shares);

                for (i, (lang, _)) in langs.iter().enumerate() {
                    let lang_entry = entry.by_language.entry((*lang).clone()).or_default();
                    lang_entry.additions += a_parts[i];
                    lang_entry.deletions += d_parts[i];
                    lang_entry.net_modifications += nm_parts[i];
                    lang_entry.net_additions += na_parts[i];
                    lang_entry.files_changed += 1;
                }
            } else if commit.additions > 0 || commit.deletions > 0 {
                let lang_entry = entry.by_language.entry("Other".to_string()).or_default();
                lang_entry.additions += commit.additions;
                lang_entry.deletions += commit.deletions;
                lang_entry.net_modifications += commit.additions.max(commit.deletions);
                lang_entry.net_additions += commit.additions.saturating_sub(commit.deletions);
                lang_entry.files_changed += 1;
            }
        }

        result.push(entry);
    }

    result.sort_by(|a, b| {
        b.total_additions
            .cmp(&a.total_additions)
            .then(a.period_label.cmp(&b.period_label))
    });
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

#[derive(Debug, Clone, Deserialize)]
pub struct ContributorWeek {
    pub w: i64,
    pub a: u64,
    pub d: u64,
    pub c: u64,
    /// Pre-computed per-commit `max(additions, deletions)` summed across commits in this week.
    #[serde(default)]
    pub net_modifications: u64,
    /// Pre-computed per-commit `additions.saturating_sub(deletions)` summed across commits.
    #[serde(default)]
    pub net_additions: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    fn sample_repo() -> RepoWithLangs {
        RepoWithLangs {
            owner: "octocat".to_string(),
            name: "hello-world".to_string(),
            is_fork: false,
            languages: HashMap::from([("Rust".to_string(), 100)]),
        }
    }

    fn fixed_time(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, day, 0, 0, 0).unwrap()
    }

    fn response_headers(values: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn retry_decision_uses_retry_after_rate_limit_and_wait_bound_contracts() {
        type Case = (
            reqwest::StatusCode,
            &'static [(&'static str, &'static str)],
            Option<u64>,
            Option<&'static [&'static str]>,
        );
        let cases: &[Case] = &[
            (
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                &[
                    ("retry-after", "7"),
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "1735689615"),
                ],
                Some(7),
                None,
            ),
            (
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                &[("retry-after", "Wed, 01 Jan 2025 00:00:09 GMT")],
                Some(9),
                None,
            ),
            (
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                &[("retry-after", "Tue, 31 Dec 2024 23:59:59 GMT")],
                Some(0),
                None,
            ),
            (
                reqwest::StatusCode::FORBIDDEN,
                &[],
                None,
                Some(&["permission"]),
            ),
            (
                reqwest::StatusCode::FORBIDDEN,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "1735689605"),
                ],
                Some(5),
                None,
            ),
            (
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                &[("retry-after", "121")],
                None,
                Some(&["121", "120"]),
            ),
        ];

        for (status, headers, expected_wait, expected_error) in cases {
            let decision = retry_decision(
                *status,
                &response_headers(headers),
                Duration::from_secs(1),
                fixed_time(1),
                Duration::from_secs(120),
            );

            if let Some(seconds) = expected_wait {
                assert_eq!(
                    decision,
                    RetryDecision::RetryAfter(Duration::from_secs(*seconds))
                );
            } else {
                let RetryDecision::Fail(message) = decision else {
                    panic!("expected failure for {status}");
                };
                for expected in expected_error.expect("expected failure fragments") {
                    assert!(message.contains(expected), "message: {message}");
                }
            }
        }
    }

    fn rolling_for_test(observed_at: DateTime<Utc>, lookback: chrono::Duration) -> QueryWindow {
        let requested_from = observed_at.checked_sub_signed(lookback).unwrap();
        QueryWindow {
            scope: CacheWindowScope::Rolling {
                lookback_nanoseconds: lookback.num_nanoseconds().unwrap(),
            },
            requested_from,
            until_exclusive: observed_at,
            observed_at,
            completed: false,
        }
    }

    fn sample_for_test(name: &str, count: u64) -> CachedContributionPayload {
        CachedContributionPayload {
            repos: vec![(
                RepoWithLangs {
                    owner: "octocat".to_string(),
                    name: name.to_string(),
                    is_fork: false,
                    languages: HashMap::from([("Rust".to_string(), 100)]),
                },
                count,
            )],
            summary: ContributionSummary::default(),
        }
    }

    fn contribution_fingerprint(
        repos: &[(RepoWithLangs, u64)],
        summary: &ContributionSummary,
    ) -> serde_json::Value {
        let mut repos: Vec<_> = repos
            .iter()
            .map(|(repo, count)| (repo.owner.clone(), repo.name.clone(), *count))
            .collect();
        repos.sort();
        serde_json::json!({
            "repos": repos,
            "summary": {
                "pull_requests": summary.total_prs,
                "reviews": summary.total_reviews,
                "issues": summary.total_issues,
            }
        })
    }

    fn contribution_response_for_test(name: &str, count: u64) -> String {
        serde_json::json!({
            "data": {
                "user": {
                    "contributionsCollection": {
                        "totalPullRequestContributions": 0,
                        "totalPullRequestReviewContributions": 0,
                        "totalIssueContributions": 0,
                        "commitContributionsByRepository": [{
                            "repository": {
                                "name": name,
                                "owner": { "login": "octocat" },
                                "isFork": false,
                                "languages": { "edges": [] }
                            },
                            "contributions": { "totalCount": count }
                        }]
                    }
                }
            }
        })
        .to_string()
    }

    fn commit(oid: &str, committed_date: &str, additions: u64) -> CommitData {
        commit_with_oid(Some(oid), committed_date, additions)
    }

    fn commit_with_oid(oid: Option<&str>, committed_date: &str, additions: u64) -> CommitData {
        CommitData {
            oid: oid.map(str::to_string),
            additions,
            deletions: 0,
            committed_date: committed_date.to_string(),
        }
    }

    fn history_request(
        owner: &str,
        name: &str,
        since: &str,
        until_exclusive: &str,
    ) -> RepoHistoryRequest {
        RepoHistoryRequest {
            owner: owner.to_string(),
            name: name.to_string(),
            since: Some(since.to_string()),
            until_exclusive: Some(until_exclusive.to_string()),
        }
    }

    #[test]
    fn rolling_history_refresh_starts_at_checked_until_and_trims_left_edge() {
        let window = rolling_for_test(fixed_time(11), chrono::Duration::days(9));
        let cached = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(10),
            observed_at: fixed_time(10),
            completeness: Completeness::Complete,
            payload: vec![
                commit("expired", "2025-01-01T12:00:00Z", 1),
                commit("kept", "2025-01-05T12:00:00Z", 2),
            ],
        };

        let HistoryFetchPlan::Gap { retained, request } = plan_history_refresh(
            Some(cached),
            &window,
            CachePolicy::Refresh,
            "octocat",
            "repo",
        )
        .unwrap() else {
            panic!("expected a right-edge gap");
        };

        assert_eq!(
            retained
                .iter()
                .map(|commit| commit.oid.as_deref())
                .collect::<Vec<_>>(),
            [Some("kept")]
        );
        assert_eq!(request.since.as_deref(), Some("2025-01-10T00:00:00+00:00"));
        assert_eq!(
            request.until_exclusive.as_deref(),
            Some("2025-01-11T00:00:00+00:00")
        );

        let plan = HistoryFetchPlan::Gap {
            retained: vec![commit("kept", "2025-01-05T12:00:00Z", 2)],
            request: history_request(
                "octocat",
                "repo",
                "2025-01-10T00:00:00Z",
                "2025-01-11T00:00:00Z",
            ),
        };

        let envelope = finish_history_fetch(plan, vec![], &window, Completeness::Complete).unwrap();

        assert_eq!(envelope.checked_until, window.until_exclusive);
        assert_eq!(envelope.payload.len(), 1);
        assert_eq!(envelope.payload[0].oid.as_deref(), Some("kept"));
    }

    #[test]
    fn history_dedup_only_collapses_nonempty_oids() {
        let commits = dedup_commits(vec![
            commit("same", "2025-01-05T00:00:00Z", 1),
            commit("same", "2025-01-05T00:00:00Z", 1),
            commit_with_oid(None, "2025-01-06T00:00:00Z", 2),
            commit_with_oid(None, "2025-01-06T00:00:00Z", 2),
            commit_with_oid(Some(""), "2025-01-07T00:00:00Z", 3),
            commit_with_oid(Some(""), "2025-01-07T00:00:00Z", 3),
        ]);

        assert_eq!(commits.len(), 5);
        assert_eq!(
            commits
                .iter()
                .filter(|commit| commit.oid.as_deref().is_none_or(str::is_empty))
                .count(),
            4
        );
    }

    #[test]
    fn malformed_incomplete_or_clock_rollback_history_plans_full_fetch() {
        let window = rolling_for_test(fixed_time(11), chrono::Duration::days(9));
        let temp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(temp.path()).unwrap();

        let malformed = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(10),
            observed_at: fixed_time(10),
            completeness: Completeness::Complete,
            payload: vec![commit("bad", "not-a-timestamp", 1)],
        };
        cache.set("malformed-history", &malformed).unwrap();
        let mut malformed_warnings = CacheWarnings::default();
        let malformed_cached = history_cache_get_or_warn(
            &cache,
            "malformed-history",
            &window,
            &mut malformed_warnings,
        );
        assert!(matches!(
            plan_history_refresh(
                malformed_cached,
                &window,
                CachePolicy::ReadOnly,
                "octocat",
                "repo",
            )
            .unwrap(),
            HistoryFetchPlan::Full { .. }
        ));
        assert_eq!(malformed_warnings.messages.len(), 1);

        let incomplete: CacheEnvelope<Vec<CommitData>> = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(10),
            observed_at: fixed_time(10),
            completeness: Completeness::Incomplete(vec![IncompleteReason::HistoryPageLimit {
                repository: "octocat/repo".to_string(),
                pages: 20,
            }]),
            payload: vec![],
        };
        cache.set("incomplete-history", &incomplete).unwrap();
        let mut incomplete_warnings = CacheWarnings::default();
        let incomplete_cached = history_cache_get_or_warn(
            &cache,
            "incomplete-history",
            &window,
            &mut incomplete_warnings,
        );
        assert!(matches!(
            plan_history_refresh(
                incomplete_cached,
                &window,
                CachePolicy::ReadOnly,
                "octocat",
                "repo",
            )
            .unwrap(),
            HistoryFetchPlan::Full { .. }
        ));
        assert_eq!(incomplete_warnings.messages.len(), 1);

        let clock_rollback: CacheEnvelope<Vec<CommitData>> = CacheEnvelope {
            requested_from: fixed_time(3),
            checked_until: fixed_time(12),
            observed_at: fixed_time(12),
            completeness: Completeness::Complete,
            payload: vec![],
        };
        cache
            .set("clock-rollback-history", &clock_rollback)
            .unwrap();
        let mut rollback_warnings = CacheWarnings::default();
        let rollback_cached = history_cache_get_or_warn(
            &cache,
            "clock-rollback-history",
            &window,
            &mut rollback_warnings,
        );
        assert!(matches!(
            plan_history_refresh(
                rollback_cached,
                &window,
                CachePolicy::ReadOnly,
                "octocat",
                "repo",
            )
            .unwrap(),
            HistoryFetchPlan::Full { .. }
        ));
        assert_eq!(rollback_warnings.messages.len(), 1);
    }

    #[test]
    fn capped_history_envelope_is_incomplete_not_complete() {
        let envelope = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(2),
            observed_at: fixed_time(2),
            completeness: Completeness::Incomplete(vec![IncompleteReason::HistoryPageLimit {
                repository: "octocat/repo".to_string(),
                pages: 20,
            }]),
            payload: vec![commit("partial", "2025-01-01T12:00:00Z", 1)],
        };

        let roundtrip: CacheEnvelope<Vec<CommitData>> =
            serde_json::from_str(&serde_json::to_string(&envelope).unwrap()).unwrap();

        assert!(!roundtrip.completeness.is_complete());
        assert_eq!(
            roundtrip.completeness,
            Completeness::Incomplete(vec![IncompleteReason::HistoryPageLimit {
                repository: "octocat/repo".to_string(),
                pages: 20,
            }])
        );
    }

    fn history_response(commits: Vec<CommitData>) -> String {
        let total_count = commits.len();
        let nodes: Vec<serde_json::Value> = commits
            .into_iter()
            .map(|commit| {
                json!({
                    "oid": commit.oid,
                    "additions": commit.additions,
                    "deletions": commit.deletions,
                    "committedDate": commit.committed_date,
                })
            })
            .collect();
        json!({
            "data": {
                "repo0": {
                    "defaultBranchRef": {
                        "target": {
                            "history": {
                                "nodes": nodes,
                                "totalCount": total_count,
                                "pageInfo": { "hasNextPage": false, "endCursor": null },
                            }
                        }
                    }
                }
            }
        })
        .to_string()
    }

    fn graphql_variables(request: &str) -> serde_json::Value {
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        serde_json::from_str::<serde_json::Value>(body).unwrap()["variables"].clone()
    }

    fn seed_complete_history(
        cache: &DiskCache,
        user_node_id: &str,
        owner: &str,
        name: &str,
        include_private: bool,
        window: &QueryWindow,
        commits: Vec<CommitData>,
    ) {
        let key = history_cache_key(user_node_id, owner, name, include_private, &window.scope);
        cache
            .set(
                &key,
                &CacheEnvelope {
                    requested_from: window.requested_from,
                    checked_until: window.until_exclusive,
                    observed_at: window.observed_at,
                    completeness: Completeness::Complete,
                    payload: commits,
                },
            )
            .unwrap();
    }

    fn fetch_one_history_from_cache(
        client: &GithubClient,
        cache: &DiskCache,
        user_node_id: &str,
        owner: &str,
        name: &str,
        window: &QueryWindow,
        cache_policy: CachePolicy,
    ) -> anyhow::Result<Vec<CommitData>> {
        let key = history_cache_key(user_node_id, owner, name, false, &window.scope);
        let mut warnings = CacheWarnings::default();
        let cached = history_cache_get_or_warn(cache, &key, window, &mut warnings);
        let plan = plan_history_refresh(cached, window, cache_policy, owner, name)?;

        let (HistoryFetchPlan::Full { request } | HistoryFetchPlan::Gap { request, .. }) = &plan
        else {
            unreachable!("a test refresh with an uncovered window must fetch")
        };
        let fetched = client.batch_commit_history(user_node_id, std::slice::from_ref(request))?;
        let repository = repo_key(owner, name);
        let completeness = if fetched.capped_repos.contains(&repository) {
            Completeness::Incomplete(vec![IncompleteReason::HistoryPageLimit {
                repository: format!("{owner}/{name}"),
                pages: 20,
            }])
        } else {
            Completeness::Complete
        };
        let commits = fetched
            .commits
            .get(&repository)
            .cloned()
            .unwrap_or_default();
        let envelope = finish_history_fetch(plan, commits, window, completeness)?;
        if cache_policy.can_write() && envelope.completeness.is_complete() {
            cache.set(&key, &envelope)?;
        }
        Ok(envelope.payload)
    }

    fn refresh_one_history(
        client: &GithubClient,
        cache: &DiskCache,
        user_node_id: &str,
        owner: &str,
        name: &str,
        window: &QueryWindow,
    ) -> anyhow::Result<Vec<CommitData>> {
        fetch_one_history_from_cache(
            client,
            cache,
            user_node_id,
            owner,
            name,
            window,
            CachePolicy::Refresh,
        )
    }

    fn fetch_one_history_without_cache(
        client: &GithubClient,
        user_node_id: &str,
        owner: &str,
        name: &str,
        window: &QueryWindow,
    ) -> anyhow::Result<Vec<CommitData>> {
        let request = RepoHistoryRequest {
            owner: owner.to_string(),
            name: name.to_string(),
            since: Some(window.requested_from.to_rfc3339()),
            until_exclusive: Some(window.until_exclusive.to_rfc3339()),
        };
        let fetched = client.batch_commit_history(user_node_id, std::slice::from_ref(&request))?;
        let repository = repo_key(owner, name);
        let commits = fetched
            .commits
            .get(&repository)
            .cloned()
            .unwrap_or_default();
        Ok(finish_history_fetch(
            HistoryFetchPlan::Full { request },
            commits,
            window,
            Completeness::Complete,
        )?
        .payload)
    }

    #[test]
    fn second_rolling_history_refresh_fetches_only_gap_and_matches_fresh_result() {
        let first_window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let second_window = rolling_for_test(fixed_time(9), chrono::Duration::days(7));
        let temp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(temp.path()).unwrap();
        seed_complete_history(
            &cache,
            "NODE",
            "octocat",
            "repo",
            false,
            &first_window,
            vec![
                commit("old", "2025-01-01T12:00:00Z", 1),
                commit("kept", "2025-01-05T12:00:00Z", 2),
            ],
        );
        let server = start_stub(vec![StubResponse::OwnedJson {
            status: 200,
            body: history_response(vec![commit("new", "2025-01-08T12:00:00Z", 3)]),
            delay: Duration::ZERO,
        }]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));

        let refreshed =
            refresh_one_history(&client, &cache, "NODE", "octocat", "repo", &second_window)
                .unwrap();
        let requests = server.finish();
        let variables = graphql_variables(&requests[0]);
        assert_eq!(variables["since0"], "2025-01-08T00:00:00+00:00");
        assert_eq!(variables["until0"], "2025-01-09T00:00:00+00:00");
        assert_eq!(
            refreshed
                .iter()
                .map(|commit| commit.oid.as_deref())
                .collect::<Vec<_>>(),
            [Some("kept"), Some("new")]
        );

        let fresh_server = start_stub(vec![StubResponse::OwnedJson {
            status: 200,
            body: history_response(vec![
                commit("kept", "2025-01-05T12:00:00Z", 2),
                commit("new", "2025-01-08T12:00:00Z", 3),
            ]),
            delay: Duration::ZERO,
        }]);
        let fresh_client =
            GithubClient::for_test(&fresh_server.base_url, Vec::new(), Duration::from_secs(1));
        let fresh = fetch_one_history_without_cache(
            &fresh_client,
            "NODE",
            "octocat",
            "repo",
            &second_window,
        )
        .unwrap();

        assert_eq!(refreshed, fresh);
        let fresh_requests = fresh_server.finish();
        assert_eq!(fresh_requests.len(), 1);
        let fresh_variables = graphql_variables(&fresh_requests[0]);
        assert_eq!(fresh_variables["since0"], "2025-01-02T00:00:00+00:00");
        assert_eq!(fresh_variables["until0"], "2025-01-09T00:00:00+00:00");
    }

    #[test]
    fn readonly_rolling_history_gap_fetches_current_result_without_cache_write() {
        let first_window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let second_window = rolling_for_test(fixed_time(9), chrono::Duration::days(7));
        let temp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(temp.path()).unwrap();
        seed_complete_history(
            &cache,
            "NODE",
            "octocat",
            "repo",
            false,
            &first_window,
            vec![
                commit("old", "2025-01-01T12:00:00Z", 1),
                commit("kept", "2025-01-05T12:00:00Z", 2),
            ],
        );
        let server = start_stub(vec![StubResponse::OwnedJson {
            status: 200,
            body: history_response(vec![commit("new", "2025-01-08T12:00:00Z", 3)]),
            delay: Duration::ZERO,
        }]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));

        let current = fetch_one_history_from_cache(
            &client,
            &cache,
            "NODE",
            "octocat",
            "repo",
            &second_window,
            CachePolicy::ReadOnly,
        )
        .unwrap();

        assert_eq!(
            current
                .iter()
                .map(|commit| commit.oid.as_deref())
                .collect::<Vec<_>>(),
            [Some("kept"), Some("new")]
        );
        let variables = graphql_variables(&server.finish()[0]);
        assert_eq!(variables["since0"], "2025-01-08T00:00:00+00:00");
        assert_eq!(variables["until0"], "2025-01-09T00:00:00+00:00");

        let key = history_cache_key("NODE", "octocat", "repo", false, &first_window.scope);
        let persisted = cache
            .get::<CacheEnvelope<Vec<CommitData>>>(&key)
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted
                .payload
                .iter()
                .map(|commit| commit.oid.as_deref())
                .collect::<Vec<_>>(),
            [Some("old"), Some("kept")]
        );
    }

    #[test]
    fn v4_rolling_keys_are_stable_across_observation_time_and_v3_files_naturally_miss() {
        let first = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let second = rolling_for_test(fixed_time(9), chrono::Duration::days(7));
        let first_key =
            contribution_cache_key("node-octocat", "OctoCat", false, false, false, &first.scope);
        let second_key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &second.scope,
        );
        let legacy_key = first_key.replacen("v4_", "v3_", 1);
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        cache
            .set(&legacy_key, &serde_json::json!({ "legacy": true }))
            .unwrap();

        assert_eq!(first_key, second_key);
        assert!(first_key.starts_with("v4_contribution_"));
        assert!(
            cache
                .get::<CacheEnvelope<CachedContributionPayload>>(&first_key)
                .unwrap()
                .is_none()
        );
        assert!(tmp.path().join(format!("{legacy_key}.json")).is_file());
    }

    #[test]
    fn envelope_bounds_reject_invalid_coverage_clocks_and_scope() {
        let fixed_window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from: fixed_time(1),
                until_exclusive: fixed_time(2),
            },
            requested_from: fixed_time(1),
            until_exclusive: fixed_time(2),
            observed_at: fixed_time(3),
            completed: true,
        };
        let payload = sample_for_test("repo", 1);
        let invalid_start = CacheEnvelope {
            requested_from: fixed_time(2),
            checked_until: fixed_time(1),
            observed_at: fixed_time(3),
            completeness: Completeness::Complete,
            payload: payload.clone(),
        };
        let invalid_check = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(3),
            observed_at: fixed_time(2),
            completeness: Completeness::Complete,
            payload: payload.clone(),
        };
        assert!(validate_envelope_bounds(&invalid_start, &fixed_window).is_err());
        assert!(validate_envelope_bounds(&invalid_check, &fixed_window).is_err());

        let rolling_window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let clock_rollback = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(9),
            observed_at: fixed_time(9),
            completeness: Completeness::Complete,
            payload: payload.clone(),
        };
        let payload_beyond_coverage = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(7),
            observed_at: fixed_time(8),
            completeness: Completeness::Complete,
            payload,
        };
        assert!(validate_envelope_bounds(&clock_rollback, &rolling_window).is_err());
        assert!(validate_envelope_bounds(&payload_beyond_coverage, &rolling_window).is_err());

        let incompatible_scope = QueryWindow {
            completed: false,
            ..fixed_window
        };
        let valid_fixed = CacheEnvelope {
            requested_from: fixed_time(1),
            checked_until: fixed_time(2),
            observed_at: fixed_time(3),
            completeness: Completeness::Complete,
            payload: sample_for_test("repo", 1),
        };
        assert!(validate_envelope_bounds(&valid_fixed, &incompatible_scope).is_err());
    }

    #[test]
    fn incomplete_contribution_never_claims_complete_and_replays_reason() {
        let window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &window.scope,
        );
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let completeness =
            Completeness::Incomplete(vec![IncompleteReason::ContributionRepositoryLimit {
                limit: 100,
            }]);
        cache
            .set(
                &key,
                &CacheEnvelope {
                    requested_from: window.requested_from,
                    checked_until: window.until_exclusive,
                    observed_at: window.observed_at,
                    completeness: completeness.clone(),
                    payload: sample_for_test("limited", 100),
                },
            )
            .unwrap();
        let client =
            GithubClient::for_test("http://127.0.0.1:1", Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();

        let (repos, _) = get_contribution_repos_cached(
            &client,
            &cache,
            ContributionCacheRequest {
                user_node_id: "node-octocat",
                username: "octocat",
                include_forks: false,
                include_contributed: false,
                include_private: false,
                query_window: &window,
                cache_policy: CachePolicy::ReadOnly,
            },
            &mut warnings,
        )
        .unwrap();

        assert!(!completeness.is_complete());
        assert_eq!(repos[0].0.name, "limited");
        assert_eq!(
            warnings.messages,
            vec![completeness.visible_warning().unwrap()]
        );
    }

    #[test]
    fn rolling_contribution_refresh_requests_the_exact_new_full_window() {
        let old_window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let new_window = rolling_for_test(fixed_time(9), chrono::Duration::days(7));
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &new_window.scope,
        );
        cache
            .set(
                &key,
                &CacheEnvelope {
                    requested_from: old_window.requested_from,
                    checked_until: old_window.until_exclusive,
                    observed_at: old_window.observed_at,
                    completeness: Completeness::Complete,
                    payload: sample_for_test("old-repo", 99),
                },
            )
            .unwrap();
        let new_response = contribution_response_for_test("new-repo", 3);
        let server = start_stub(vec![
            StubResponse::OwnedJson {
                status: 200,
                body: new_response.clone(),
                delay: Duration::ZERO,
            },
            StubResponse::OwnedJson {
                status: 200,
                body: new_response,
                delay: Duration::ZERO,
            },
        ]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();

        let refreshed = get_contribution_repos_cached(
            &client,
            &cache,
            ContributionCacheRequest {
                user_node_id: "node-octocat",
                username: "octocat",
                include_forks: false,
                include_contributed: false,
                include_private: false,
                query_window: &new_window,
                cache_policy: CachePolicy::Refresh,
            },
            &mut warnings,
        )
        .unwrap();
        let refreshed_envelope = cache
            .get::<CacheEnvelope<CachedContributionPayload>>(&key)
            .unwrap()
            .unwrap();
        let fresh = client
            .get_contribution_repos(
                "octocat",
                new_window.requested_from,
                new_window.until_exclusive,
                false,
                false,
            )
            .unwrap();
        let requests = server.finish();
        let variables: Vec<serde_json::Value> = requests
            .iter()
            .map(|request| {
                let (_, body) = request.split_once("\r\n\r\n").unwrap();
                serde_json::from_str::<serde_json::Value>(body).unwrap()["variables"].clone()
            })
            .collect();

        assert_eq!(requests.len(), 2);
        for request in variables {
            assert_eq!(request["from"], "2025-01-02T00:00:00+00:00");
            assert_eq!(request["to"], "2025-01-08T23:59:59.999999999+00:00");
        }
        assert_eq!(refreshed.0.len(), 1);
        assert_eq!(refreshed.0[0].0.name, "new-repo");
        assert_eq!(refreshed.0[0].1, 3);
        assert_eq!(refreshed_envelope.requested_from, new_window.requested_from);
        assert_eq!(refreshed_envelope.checked_until, new_window.until_exclusive);
        assert_eq!(refreshed_envelope.payload.repos[0].0.name, "new-repo");
        assert_eq!(
            contribution_fingerprint(&refreshed.0, &refreshed.1),
            contribution_fingerprint(&fresh.0, &fresh.1)
        );
        assert!(warnings.messages.is_empty());
    }

    #[test]
    fn read_only_contribution_miss_fetches_without_writing_an_envelope() {
        let window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &window.scope,
        );
        let server = start_stub(vec![StubResponse::OwnedJson {
            status: 200,
            body: contribution_response_for_test("fresh-repo", 1),
            delay: Duration::ZERO,
        }]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();

        let (repos, _) = get_contribution_repos_cached(
            &client,
            &cache,
            ContributionCacheRequest {
                user_node_id: "node-octocat",
                username: "octocat",
                include_forks: false,
                include_contributed: false,
                include_private: false,
                query_window: &window,
                cache_policy: CachePolicy::ReadOnly,
            },
            &mut warnings,
        )
        .unwrap();

        assert_eq!(repos[0].0.name, "fresh-repo");
        assert!(
            cache
                .get::<CacheEnvelope<CachedContributionPayload>>(&key)
                .unwrap()
                .is_none()
        );
        assert_eq!(server.finish().len(), 1);
        assert!(warnings.messages.is_empty());
    }

    #[test]
    fn history_cache_key_is_user_scoped_and_component_collision_free() {
        let first_window = rolling_for_test(fixed_time(8), chrono::Duration::days(31));
        let second_window = rolling_for_test(fixed_time(9), chrono::Duration::days(31));
        let alice = history_cache_key("node-alice", "Octo/Org", "Repo", false, &first_window.scope);
        let alice_lower = history_cache_key(
            "node-alice",
            "octo/org",
            "repo",
            false,
            &second_window.scope,
        );
        let bob = history_cache_key("node-bob", "octo/org", "repo", false, &first_window.scope);
        let slash = history_cache_key("node-alice", "octo/org", "repo", false, &first_window.scope);
        let underscore =
            history_cache_key("node-alice", "octo_org", "repo", false, &first_window.scope);
        let different_range = history_cache_key(
            "node-alice",
            "octo/org",
            "repo",
            false,
            &CacheWindowScope::Rolling {
                lookback_nanoseconds: chrono::Duration::days(30).num_nanoseconds().unwrap(),
            },
        );
        let private =
            history_cache_key("node-alice", "octo/org", "repo", true, &first_window.scope);

        assert!(alice.starts_with("v4_"));
        assert_eq!(alice, alice_lower);
        assert_ne!(alice, bob);
        assert_ne!(slash, underscore);
        assert_ne!(alice, different_range);
        assert_ne!(alice, private);

        let from = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
        let scope = CacheWindowScope::Fixed {
            from,
            until_exclusive: to,
        };
        let contribution =
            contribution_cache_key("node-alice", "octo/org", false, false, false, &scope);
        let contribution_user =
            contribution_cache_key("node-bob", "octo/org", false, false, false, &scope);
        let contribution_component =
            contribution_cache_key("node-alice", "octo_org", false, false, false, &scope);
        let contribution_range = contribution_cache_key(
            "node-alice",
            "octo/org",
            false,
            false,
            false,
            &CacheWindowScope::Fixed {
                from,
                until_exclusive: Utc.with_ymd_and_hms(2025, 2, 2, 0, 0, 0).unwrap(),
            },
        );
        let contribution_mode =
            contribution_cache_key("node-alice", "octo/org", true, true, true, &scope);

        assert!(contribution.starts_with("v4_"));
        assert_ne!(contribution, contribution_user);
        assert_ne!(contribution, contribution_component);
        assert_ne!(contribution, contribution_range);
        assert_ne!(contribution, contribution_mode);
    }

    #[test]
    fn disabled_cache_policy_never_invokes_cache_factory() {
        let invocations = Cell::new(0);

        let cache = initialize_cache_for_policy(CachePolicy::Disabled, || {
            invocations.set(invocations.get() + 1);
            DiskCache::new()
        });

        assert!(cache.is_none());
        assert_eq!(invocations.get(), 0);
    }

    #[test]
    fn older_cache_entries_are_misses_for_v4_contribution_and_v4_history_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let contribution_scope = CacheWindowScope::Fixed {
            from,
            until_exclusive: to,
        };
        let contribution_key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &contribution_scope,
        );
        let history_key = history_cache_key(
            "node-octocat",
            "octocat",
            "hello-world",
            false,
            &contribution_scope,
        );
        let v3_contribution_key = contribution_key.replacen("v4_", "v3_", 1);
        let v3_history_key = history_key.replacen("v4_", "v3_", 1);

        cache
            .set(&v3_contribution_key, &serde_json::json!({ "legacy": true }))
            .unwrap();
        cache
            .set(&v3_history_key, &serde_json::json!({ "legacy": true }))
            .unwrap();

        assert!(
            cache
                .get::<CacheEnvelope<CachedContributionPayload>>(&contribution_key)
                .unwrap()
                .is_none(),
            "current contribution key must not load a v3 payload"
        );
        assert!(
            cache
                .get::<CacheEnvelope<Vec<CommitData>>>(&history_key)
                .unwrap()
                .is_none(),
            "current history key must not load a v3 payload"
        );
        assert!(contribution_key.starts_with("v4_"));
        assert!(history_key.starts_with("v4_"));

        cache
            .set(
                &contribution_key,
                &CacheEnvelope {
                    requested_from: from,
                    checked_until: to,
                    observed_at: to,
                    completeness: Completeness::Complete,
                    payload: sample_for_test("hello-world", 7),
                },
            )
            .unwrap();
        cache
            .set(
                &history_key,
                &CacheEnvelope {
                    requested_from: from,
                    checked_until: to,
                    observed_at: to,
                    completeness: Completeness::Complete,
                    payload: vec![CommitData {
                        oid: Some("current".to_string()),
                        additions: 7,
                        deletions: 0,
                        committed_date: "2020-01-01T12:00:00Z".to_string(),
                    }],
                },
            )
            .unwrap();

        let contribution = cache
            .get::<CacheEnvelope<CachedContributionPayload>>(&contribution_key)
            .unwrap()
            .unwrap();
        let history = cache
            .get::<CacheEnvelope<Vec<CommitData>>>(&history_key)
            .unwrap()
            .unwrap();
        assert_eq!(contribution.payload.repos[0].1, 7);
        assert_eq!(history.payload[0].oid.as_deref(), Some("current"));
    }

    #[test]
    fn completed_contribution_cache_hit_restores_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from,
                until_exclusive: to,
            },
            requested_from: from,
            until_exclusive: to,
            observed_at: to,
            completed: true,
        };
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &window.scope,
        );
        let expected_summary = ContributionSummary {
            total_prs: 3,
            total_reviews: 5,
            total_issues: 7,
        };
        cache
            .set(
                &key,
                &CacheEnvelope {
                    requested_from: from,
                    checked_until: to,
                    observed_at: to,
                    completeness: Completeness::Complete,
                    payload: CachedContributionPayload {
                        repos: vec![(sample_repo(), 11)],
                        summary: expected_summary.clone(),
                    },
                },
            )
            .unwrap();

        let client =
            GithubClient::for_test("http://127.0.0.1:1", Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();
        let (repos, summary) = get_contribution_repos_cached(
            &client,
            &cache,
            ContributionCacheRequest {
                user_node_id: "node-octocat",
                username: "octocat",
                include_forks: false,
                include_contributed: false,
                include_private: false,
                query_window: &window,
                cache_policy: CachePolicy::ReadOnly,
            },
            &mut warnings,
        )
        .unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].1, 11);
        assert_eq!(summary.total_prs, expected_summary.total_prs);
        assert_eq!(summary.total_reviews, expected_summary.total_reviews);
        assert_eq!(summary.total_issues, expected_summary.total_issues);
        assert!(warnings.messages.is_empty());
    }

    #[test]
    fn contribution_queries_make_adjacent_half_open_windows_non_overlapping() {
        let from = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let windows = contribution_windows(from, until);
        let contribution_response = r#"{"data":{"user":{"contributionsCollection":{"totalPullRequestContributions":0,"totalPullRequestReviewContributions":0,"totalIssueContributions":0,"commitContributionsByRepository":[]}}}}"#;
        let server = start_stub(vec![
            StubResponse::Json {
                status: 200,
                body: contribution_response.to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 200,
                body: contribution_response.to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 200,
                body: contribution_response.to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
        ]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));

        client
            .get_contribution_repos("octocat", from, until, false, false)
            .unwrap();

        let requests = server.finish();
        assert_eq!(requests.len(), windows.len());
        let variables: Vec<serde_json::Value> = requests
            .iter()
            .map(|request| {
                let (_, body) = request.split_once("\r\n\r\n").unwrap();
                serde_json::from_str::<serde_json::Value>(body).unwrap()["variables"].clone()
            })
            .collect();
        let parse = |value: &serde_json::Value| {
            DateTime::parse_from_rfc3339(value.as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc)
        };
        let first_api_to = parse(&variables[0]["to"]);
        let second_api_from = parse(&variables[1]["from"]);
        let final_api_to = parse(&variables.last().unwrap()["to"]);

        assert!(first_api_to < second_api_from);
        assert_eq!(second_api_from, windows[0].1);
        assert!(final_api_to < until);
    }

    #[test]
    fn contribution_repository_cap_sets_replayable_partial_warning_state() {
        let completeness =
            Completeness::Incomplete(vec![IncompleteReason::ContributionRepositoryLimit {
                limit: 100,
            }]);
        let mut warnings = CacheWarnings::default();

        if let Some(warning) = completeness.visible_warning() {
            warnings.push(warning);
        }

        assert!(!completeness.is_complete());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("100-repository limit"));
    }

    #[test]
    fn saturated_contribution_cache_hit_replays_partial_data_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from,
                until_exclusive: to,
            },
            requested_from: from,
            until_exclusive: to,
            observed_at: to,
            completed: true,
        };
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &window.scope,
        );
        cache
            .set(
                &key,
                &CacheEnvelope {
                    requested_from: from,
                    checked_until: to,
                    observed_at: to,
                    completeness: Completeness::Incomplete(vec![
                        IncompleteReason::ContributionRepositoryLimit { limit: 100 },
                    ]),
                    payload: CachedContributionPayload {
                        repos: vec![(sample_repo(), 100)],
                        summary: ContributionSummary::default(),
                    },
                },
            )
            .unwrap();
        let client =
            GithubClient::for_test("http://127.0.0.1:1", Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();

        let (repos, summary) = get_contribution_repos_cached(
            &client,
            &cache,
            ContributionCacheRequest {
                user_node_id: "node-octocat",
                username: "octocat",
                include_forks: false,
                include_contributed: false,
                include_private: false,
                query_window: &window,
                cache_policy: CachePolicy::ReadOnly,
            },
            &mut warnings,
        )
        .unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].0.name, "hello-world");
        assert_eq!(repos[0].1, 100);
        assert_eq!(summary.total_prs, 0);
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("100-repository limit"));
    }

    #[test]
    fn malformed_or_semantically_invalid_v4_contribution_envelopes_warn_and_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from,
                until_exclusive: to,
            },
            requested_from: from,
            until_exclusive: to,
            observed_at: to,
            completed: true,
        };
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &window.scope,
        );
        std::fs::write(
            tmp.path().join(format!("{key}.json")),
            serde_json::to_string(&sample_for_test("legacy", 1)).unwrap(),
        )
        .unwrap();
        let mut warnings = CacheWarnings::default();

        assert!(contribution_cache_get_or_warn(&cache, &key, &window, &mut warnings).is_none());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("GitHub cache read failed"));

        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let window = rolling_for_test(fixed_time(8), chrono::Duration::days(7));
        let key = contribution_cache_key(
            "node-octocat",
            "octocat",
            false,
            false,
            false,
            &window.scope,
        );
        cache
            .set(
                &key,
                &CacheEnvelope {
                    requested_from: fixed_time(8),
                    checked_until: fixed_time(1),
                    observed_at: fixed_time(8),
                    completeness: Completeness::Complete,
                    payload: sample_for_test("invalid", 1),
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        assert!(contribution_cache_get_or_warn(&cache, &key, &window, &mut warnings).is_none());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("invalid; treating it as a cache miss"));
    }

    #[test]
    fn capped_history_is_not_eligible_for_complete_cache_write() {
        let request = RepoHistoryRequest {
            owner: "Octocat".to_string(),
            name: "Hello-World".to_string(),
            since: Some("2025-01-01T00:00:00Z".to_string()),
            until_exclusive: Some("2025-02-01T00:00:00Z".to_string()),
        };
        let capped_repos = HashSet::from([repo_key(&request.owner, &request.name)]);

        assert!(!history_cache_write_allowed(&request, &capped_repos));
        assert!(history_cache_write_allowed(&request, &HashSet::new()));
    }

    #[test]
    fn invalid_cached_history_commit_date_is_warning_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = "invalid-history-commit-date";
        let from = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
        let window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from,
                until_exclusive: to,
            },
            requested_from: from,
            until_exclusive: to,
            observed_at: to,
            completed: true,
        };
        cache
            .set(
                key,
                &CacheEnvelope {
                    requested_from: from,
                    checked_until: to,
                    observed_at: to,
                    completeness: Completeness::Complete,
                    payload: vec![CommitData {
                        oid: Some("bad-date".to_string()),
                        additions: 1,
                        deletions: 0,
                        committed_date: "not-a-timestamp".to_string(),
                    }],
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        let cached = history_cache_get_or_warn(&cache, key, &window, &mut warnings);

        assert!(cached.is_none());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("history cache"));
        assert!(warnings.messages[0].contains(key));
        assert!(warnings.messages[0].contains("committedDate"));
    }

    #[test]
    fn invalid_cached_history_coverage_boundary_is_warning_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = "invalid-history-check-boundary";
        let from = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
        let window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from,
                until_exclusive: to,
            },
            requested_from: from,
            until_exclusive: to,
            observed_at: to,
            completed: true,
        };
        cache
            .set(
                key,
                &CacheEnvelope::<Vec<CommitData>> {
                    requested_from: to,
                    checked_until: from,
                    observed_at: to,
                    completeness: Completeness::Complete,
                    payload: vec![],
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        let cached = history_cache_get_or_warn(&cache, key, &window, &mut warnings);

        assert!(cached.is_none());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("history cache"));
        assert!(warnings.messages[0].contains(key));
        assert!(warnings.messages[0].contains("checked end"));
    }

    #[test]
    fn valid_cached_history_remains_a_cache_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = "valid-history";
        let from = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
        let window = QueryWindow {
            scope: CacheWindowScope::Fixed {
                from,
                until_exclusive: to,
            },
            requested_from: from,
            until_exclusive: to,
            observed_at: to,
            completed: true,
        };
        cache
            .set(
                key,
                &CacheEnvelope {
                    requested_from: from,
                    checked_until: to,
                    observed_at: to,
                    completeness: Completeness::Complete,
                    payload: vec![CommitData {
                        oid: Some("valid".to_string()),
                        additions: 1,
                        deletions: 0,
                        committed_date: "2025-01-15T00:00:00Z".to_string(),
                    }],
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        let cached = history_cache_get_or_warn(&cache, key, &window, &mut warnings);

        assert_eq!(cached.unwrap().payload.len(), 1);
        assert!(warnings.messages.is_empty());
    }

    #[test]
    fn cache_write_failure_keeps_fresh_result_and_returns_visible_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        std::fs::create_dir(tmp.path().join("blocked.json")).unwrap();
        let fresh = CachedContributionPayload {
            repos: vec![(sample_repo(), 13)],
            summary: ContributionSummary {
                total_prs: 2,
                total_reviews: 3,
                total_issues: 5,
            },
        };
        let mut warnings = CacheWarnings::default();

        cache_set_or_warn(&cache, "blocked", &fresh, &mut warnings);

        assert_eq!(fresh.repos[0].1, 13);
        assert_eq!(fresh.summary.total_prs, 2);
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("blocked"));
        assert!(warnings.messages[0].contains("blocked.json"));
    }

    #[test]
    fn cache_init_and_read_failures_return_visible_warnings() {
        let mut warnings = CacheWarnings::default();
        let cache = cache_init_or_warn(
            Err(anyhow::anyhow!(
                "could not create cache at C:/broken/github"
            )),
            &mut warnings,
        );
        assert!(cache.is_none());

        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("malformed.json"), "not JSON").unwrap();
        let cached: Option<String> = cache_get_or_warn(&cache, "malformed", &mut warnings);

        assert_eq!(cached, None);
        assert_eq!(warnings.messages.len(), 2);
        assert!(warnings.messages[0].contains("C:/broken/github"));
        assert!(warnings.messages[1].contains("malformed"));
        assert!(warnings.messages[1].contains("malformed.json"));
    }

    #[test]
    fn history_cache_same_repo_two_users_never_share_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
        let scope = CacheWindowScope::Fixed {
            from,
            until_exclusive: until,
        };
        let alice_key = history_cache_key("node-alice", "octo", "repo", false, &scope);
        let bob_key = history_cache_key("node-bob", "octo", "repo", false, &scope);
        let history = |additions| CacheEnvelope {
            requested_from: from,
            checked_until: until,
            observed_at: until,
            completeness: Completeness::Complete,
            payload: vec![CommitData {
                oid: None,
                additions,
                deletions: 0,
                committed_date: "2025-01-15T00:00:00Z".to_string(),
            }],
        };

        cache.set(&alice_key, &history(11)).unwrap();
        cache.set(&bob_key, &history(22)).unwrap();

        let alice = cache
            .get::<CacheEnvelope<Vec<CommitData>>>(&alice_key)
            .unwrap()
            .unwrap();
        let bob = cache
            .get::<CacheEnvelope<Vec<CommitData>>>(&bob_key)
            .unwrap()
            .unwrap();

        assert_eq!(alice.payload[0].additions, 11);
        assert_eq!(bob.payload[0].additions, 22);
    }

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
            "created_at": "2011-01-25T18:44:36Z",
            "node_id": "MDQ6VXNlcjU4MzIzMQ=="
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
            "created_at": "2020-01-01T00:00:00Z",
            "node_id": "MDQ6VXNlcjA="
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

    #[test]
    fn parse_graphql_user_query_response() {
        let payload = json!({
            "data": {
                "user": {
                    "id": "MDQ6VXNlcjU4MzIzMQ==",
                    "login": "octocat",
                    "name": "The Octocat",
                    "bio": "GitHub mascot",
                    "publicRepositories": { "totalCount": 8 },
                    "followers": { "totalCount": 10000 },
                    "following": { "totalCount": 5 },
                    "avatarUrl": "https://avatars.githubusercontent.com/u/583231",
                    "url": "https://github.com/octocat",
                    "createdAt": "2011-01-25T18:44:36Z"
                }
            }
        });

        let data = parse_graphql_response_payload(&payload).unwrap();
        let user = parse_graphql_user_data(data, "octocat").unwrap();

        assert_eq!(user.login, "octocat");
        assert_eq!(user.node_id, "MDQ6VXNlcjU4MzIzMQ==");
        assert_eq!(user.name.as_deref(), Some("The Octocat"));
        assert_eq!(user.bio.as_deref(), Some("GitHub mascot"));
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
    fn parse_graphql_repositories_query_response() {
        let payload = json!({
            "data": {
                "user": {
                    "repositories": {
                        "pageInfo": {
                            "hasNextPage": true,
                            "endCursor": "CURSOR_1"
                        },
                        "nodes": [
                            {
                                "name": "repo-a",
                                "owner": { "login": "octocat" },
                                "isFork": false,
                                "languages": {
                                    "edges": [
                                        { "size": 120, "node": { "name": "Rust" } },
                                        { "size": 80, "node": { "name": "TypeScript" } }
                                    ]
                                }
                            },
                            {
                                "name": "repo-fork",
                                "owner": { "login": "octocat" },
                                "isFork": true,
                                "languages": {
                                    "edges": [
                                        { "size": 50, "node": { "name": "Go" } }
                                    ]
                                }
                            }
                        ]
                    }
                }
            }
        });

        let data = parse_graphql_response_payload(&payload).unwrap();
        let (repos, page_info, node_count) =
            parse_repo_connection_data(data, "octocat", RepoConnectionKind::Owned, false).unwrap();

        assert_eq!(node_count, 2);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].owner, "octocat");
        assert_eq!(repos[0].name, "repo-a");
        assert!(!repos[0].is_fork);
        assert_eq!(repos[0].languages.get("Rust"), Some(&120));
        assert_eq!(repos[0].languages.get("TypeScript"), Some(&80));
        assert!(page_info.has_next_page);
        assert_eq!(page_info.end_cursor.as_deref(), Some("CURSOR_1"));
    }

    #[test]
    fn parse_graphql_contributed_repositories_query_response() {
        let payload = json!({
            "data": {
                "user": {
                    "repositoriesContributedTo": {
                        "pageInfo": {
                            "hasNextPage": false,
                            "endCursor": null
                        },
                        "nodes": [
                            {
                                "name": "project-x",
                                "owner": { "login": "other-org" },
                                "isFork": false,
                                "languages": {
                                    "edges": [
                                        { "size": 10, "node": { "name": "Rust" } },
                                        { "size": 20, "node": { "name": "Rust" } }
                                    ]
                                }
                            }
                        ]
                    }
                }
            }
        });

        let data = parse_graphql_response_payload(&payload).unwrap();
        let (repos, page_info, node_count) =
            parse_repo_connection_data(data, "octocat", RepoConnectionKind::Contributed, true)
                .unwrap();

        assert_eq!(node_count, 1);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].owner, "other-org");
        assert_eq!(repos[0].name, "project-x");
        assert_eq!(repos[0].languages.get("Rust"), Some(&30));
        assert!(!page_info.has_next_page);
        assert!(page_info.end_cursor.is_none());
    }

    #[test]
    fn parse_graphql_contributions_collection_response() {
        let data = json!({
            "user": {
                "contributionsCollection": {
                    "totalPullRequestContributions": 9,
                    "totalPullRequestReviewContributions": 14,
                    "totalIssueContributions": 3,
                    "commitContributionsByRepository": [
                        {
                            "repository": {
                                "name": "repo-a",
                                "owner": { "login": "octocat" },
                                "isFork": false,
                                "languages": {
                                    "edges": [
                                        { "size": 70, "node": { "name": "Rust" } },
                                        { "size": 30, "node": { "name": "TypeScript" } }
                                    ]
                                }
                            },
                            "contributions": { "totalCount": 11 }
                        },
                        {
                            "repository": {
                                "name": "repo-b",
                                "owner": { "login": "other" },
                                "isFork": true,
                                "languages": { "edges": [] }
                            },
                            "contributions": { "totalCount": 4 }
                        }
                    ]
                }
            }
        });

        let (repos, summary) = parse_contributions_collection_data(data, "octocat").unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(summary.total_prs, 9);
        assert_eq!(summary.total_reviews, 14);
        assert_eq!(summary.total_issues, 3);

        assert_eq!(repos[0].0.owner, "octocat");
        assert_eq!(repos[0].0.name, "repo-a");
        assert_eq!(repos[0].1, 11);
        assert_eq!(repos[0].0.languages.get("Rust"), Some(&70));
        assert_eq!(repos[0].0.languages.get("TypeScript"), Some(&30));

        assert_eq!(repos[1].0.owner, "other");
        assert_eq!(repos[1].0.name, "repo-b");
        assert_eq!(repos[1].1, 4);
    }

    #[test]
    fn parse_batch_history_with_aliases() {
        let active = vec![
            PageRequest {
                batch_index: 0,
                owner: "octocat".to_string(),
                name: "repo-a".to_string(),
                since: None,
                until_exclusive: None,
                after: None,
            },
            PageRequest {
                batch_index: 1,
                owner: "other".to_string(),
                name: "repo-b".to_string(),
                since: None,
                until_exclusive: None,
                after: None,
            },
        ];
        let data = json!({
            "repo0": {
                "defaultBranchRef": {
                    "target": {
                        "history": {
                            "pageInfo": { "hasNextPage": false, "endCursor": null },
                            "nodes": [
                                {
                                    "oid": "repo-a-1",
                                    "additions": 10,
                                    "deletions": 3,
                                    "committedDate": "2025-01-06T12:00:00Z"
                                },
                                {
                                    "oid": "repo-a-2",
                                    "additions": 5,
                                    "deletions": 1,
                                    "committedDate": "2025-01-07T12:00:00Z"
                                }
                            ],
                            "totalCount": 2
                        }
                    }
                }
            },
            "repo1": {
                "defaultBranchRef": {
                    "target": {
                        "history": {
                            "pageInfo": { "hasNextPage": true, "endCursor": "abc" },
                            "nodes": [
                                {
                                    "oid": "repo-b-1",
                                    "additions": 100,
                                    "deletions": 50,
                                    "committedDate": "2025-01-08T12:00:00Z"
                                }
                            ],
                            "totalCount": 150
                        }
                    }
                }
            }
        });

        let parsed = parse_batch_history_data(data, &active).unwrap();
        let repo0 = parsed.get(&0).unwrap();
        assert_eq!(repo0.total_count, 2);
        assert_eq!(repo0.commits.len(), 2);
        assert_eq!(repo0.commits[0].oid.as_deref(), Some("repo-a-1"));
        assert_eq!(repo0.commits[0].additions, 10);
        assert!(!repo0.has_next_page);

        let repo1 = parsed.get(&1).unwrap();
        assert_eq!(repo1.total_count, 150);
        assert_eq!(repo1.commits.len(), 1);
        assert_eq!(repo1.commits[0].deletions, 50);
        assert!(repo1.has_next_page);
        assert_eq!(repo1.end_cursor.as_deref(), Some("abc"));
    }

    #[test]
    fn commits_bucket_into_monday_weeks() {
        let commits = vec![
            CommitData {
                oid: None,
                additions: 10,
                deletions: 2,
                committed_date: "2025-01-06T10:00:00Z".to_string(),
            }, // Monday
            CommitData {
                oid: None,
                additions: 5,
                deletions: 1,
                committed_date: "2025-01-12T22:00:00Z".to_string(),
            }, // Sunday same week
            CommitData {
                oid: None,
                additions: 7,
                deletions: 3,
                committed_date: "2025-01-13T09:00:00Z".to_string(),
            }, // Next Monday
        ];

        let buckets = commits_to_weekly_buckets(&commits);
        assert_eq!(buckets.len(), 2);

        let first_week = Utc
            .with_ymd_and_hms(2025, 1, 6, 0, 0, 0)
            .unwrap()
            .timestamp();
        let second_week = Utc
            .with_ymd_and_hms(2025, 1, 13, 0, 0, 0)
            .unwrap()
            .timestamp();

        assert_eq!(buckets[0].w, first_week);
        assert_eq!(buckets[0].a, 15);
        assert_eq!(buckets[0].d, 3);
        assert_eq!(buckets[0].c, 2);

        assert_eq!(buckets[1].w, second_week);
        assert_eq!(buckets[1].a, 7);
        assert_eq!(buckets[1].d, 3);
        assert_eq!(buckets[1].c, 1);
    }

    #[test]
    fn graphql_error_response_handling() {
        let payload = json!({
            "errors": [
                { "message": "Bad credentials" },
                { "message": "Field 'foo' doesn't exist" }
            ]
        });

        let err = parse_graphql_response_payload(&payload)
            .unwrap_err()
            .to_string();
        assert!(err.contains("GitHub GraphQL error"));
        assert!(err.contains("Bad credentials"));
        assert!(err.contains("Field 'foo' doesn't exist"));
    }

    #[test]
    fn contributions_to_repo_stats_groups_by_repo_and_sorts_by_additions() {
        let contribs = vec![
            RepoContribution {
                repo_name: "owner/repo-a".to_string(),
                total_commits: 3,
                total_additions: 35,
                total_deletions: 11,
                commits: vec![
                    CommitData {
                        oid: Some("repo-a-1".to_string()),
                        additions: 10,
                        deletions: 3,
                        committed_date: "2025-01-06T10:00:00Z".to_string(),
                    },
                    CommitData {
                        oid: Some("repo-a-2".to_string()),
                        additions: 10,
                        deletions: 3,
                        committed_date: "2025-01-07T10:00:00Z".to_string(),
                    },
                    CommitData {
                        oid: Some("repo-a-3".to_string()),
                        additions: 15,
                        deletions: 5,
                        committed_date: "2025-01-13T10:00:00Z".to_string(),
                    },
                ],
                weeks: vec![
                    ContributorWeek {
                        w: Utc
                            .with_ymd_and_hms(2025, 1, 6, 0, 0, 0)
                            .unwrap()
                            .timestamp(),
                        a: 20,
                        d: 6,
                        c: 2,
                        net_modifications: 20,
                        net_additions: 14,
                    },
                    ContributorWeek {
                        w: Utc
                            .with_ymd_and_hms(2025, 1, 13, 0, 0, 0)
                            .unwrap()
                            .timestamp(),
                        a: 15,
                        d: 5,
                        c: 1,
                        net_modifications: 15,
                        net_additions: 10,
                    },
                ],
                languages: HashMap::from([
                    ("Rust".to_string(), 70),
                    ("TypeScript".to_string(), 30),
                ]),
            },
            RepoContribution {
                repo_name: "owner/repo-b".to_string(),
                total_commits: 2,
                total_additions: 5,
                total_deletions: 3,
                commits: vec![
                    CommitData {
                        oid: Some("repo-b-1".to_string()),
                        additions: 3,
                        deletions: 2,
                        committed_date: "2025-01-20T10:00:00Z".to_string(),
                    },
                    CommitData {
                        oid: Some("repo-b-2".to_string()),
                        additions: 2,
                        deletions: 1,
                        committed_date: "2025-01-21T10:00:00Z".to_string(),
                    },
                ],
                weeks: vec![ContributorWeek {
                    w: Utc
                        .with_ymd_and_hms(2025, 1, 20, 0, 0, 0)
                        .unwrap()
                        .timestamp(),
                    a: 5,
                    d: 3,
                    c: 2,
                    net_modifications: 5,
                    net_additions: 2,
                }],
                languages: HashMap::new(),
            },
        ];

        let stats = contributions_to_repo_stats(&contribs);

        assert_eq!(stats.len(), 2);
        // Sorted by additions descending
        assert_eq!(stats[0].period_label, "owner/repo-a");
        assert_eq!(stats[1].period_label, "owner/repo-b");

        assert_eq!(stats[0].total_commits, 3);
        assert_eq!(stats[0].total_additions, 35);
        assert_eq!(stats[0].total_deletions, 11);
        assert!(stats[0].by_author.is_empty());
        // 70/30 split over total additions/deletions
        assert_eq!(stats[0].by_language["Rust"].additions, 25);
        assert_eq!(stats[0].by_language["Rust"].deletions, 8);
        assert_eq!(stats[0].by_language["TypeScript"].additions, 10);
        assert_eq!(stats[0].by_language["TypeScript"].deletions, 3);
        assert_eq!(stats[0].total_net_modifications, 35);
        assert_eq!(stats[0].total_net_additions, 24);
        assert_eq!(stats[0].by_language["Rust"].net_modifications, 25);
        assert_eq!(stats[0].by_language["Rust"].net_additions, 17);
        assert_eq!(stats[0].by_language["TypeScript"].net_modifications, 10);
        assert_eq!(stats[0].by_language["TypeScript"].net_additions, 7);

        assert_eq!(stats[1].total_commits, 2);
        assert_eq!(stats[1].total_additions, 5);
        assert_eq!(stats[1].total_deletions, 3);
        assert!(stats[1].by_author.is_empty());
        assert_eq!(stats[1].by_language["Other"].additions, 5);
        assert_eq!(stats[1].by_language["Other"].deletions, 3);
        assert_eq!(stats[1].total_net_modifications, 5);
        assert_eq!(stats[1].total_net_additions, 2);
        assert_eq!(stats[1].by_language["Other"].net_modifications, 5);
        assert_eq!(stats[1].by_language["Other"].net_additions, 2);
    }

    #[test]
    fn contributions_to_repo_stats_uses_exact_commits_over_weekly_buckets() {
        let contributions = vec![RepoContribution {
            repo_name: "owner/exact".to_string(),
            total_commits: 99,
            total_additions: 900,
            total_deletions: 300,
            commits: vec![
                CommitData {
                    oid: Some("first".to_string()),
                    additions: 5,
                    deletions: 2,
                    committed_date: "2025-01-01T00:00:00Z".to_string(),
                },
                CommitData {
                    oid: Some("second".to_string()),
                    additions: 1,
                    deletions: 4,
                    committed_date: "2025-01-02T00:00:00Z".to_string(),
                },
            ],
            weeks: vec![ContributorWeek {
                w: 0,
                a: 900,
                d: 300,
                c: 99,
                net_modifications: 900,
                net_additions: 600,
            }],
            languages: HashMap::from([("Rust".to_string(), 3), ("TypeScript".to_string(), 1)]),
        }];

        let stats = contributions_to_repo_stats(&contributions);

        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].period_label, "owner/exact");
        assert_eq!(stats[0].total_commits, 2);
        assert_eq!(stats[0].total_additions, 6);
        assert_eq!(stats[0].total_deletions, 6);
        assert_eq!(stats[0].total_net_modifications, 9);
        assert_eq!(stats[0].total_net_additions, 3);
        assert_eq!(stats[0].by_language["Rust"].additions, 5);
        assert_eq!(stats[0].by_language["Rust"].deletions, 5);
        assert_eq!(stats[0].by_language["Rust"].net_modifications, 7);
        assert_eq!(stats[0].by_language["Rust"].net_additions, 2);
        assert_eq!(stats[0].by_language["TypeScript"].additions, 1);
        assert_eq!(stats[0].by_language["TypeScript"].deletions, 1);
        assert_eq!(stats[0].by_language["TypeScript"].net_modifications, 2);
        assert_eq!(stats[0].by_language["TypeScript"].net_additions, 1);
    }

    #[test]
    fn equivalent_rfc3339_instants_compare_equal_for_range_filtering() {
        let commits = vec![
            CommitData {
                oid: None,
                additions: 1,
                deletions: 0,
                committed_date: "2025-01-01T01:00:00+01:00".to_string(),
            },
            CommitData {
                oid: None,
                additions: 2,
                deletions: 0,
                committed_date: "2025-01-01T00:00:01Z".to_string(),
            },
        ];

        let filtered = filter_commits_to_range(
            &commits,
            Some("2025-01-01T00:00:00Z"),
            Some("2025-01-01T00:00:01Z"),
        )
        .unwrap();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].additions, 1);
    }

    #[test]
    fn identical_metrics_without_oid_are_not_collapsed() {
        let commits = vec![
            CommitData {
                oid: None,
                additions: 5,
                deletions: 3,
                committed_date: "2025-01-01T00:00:00Z".to_string(),
            },
            CommitData {
                oid: None,
                additions: 5,
                deletions: 3,
                committed_date: "2025-01-01T00:00:00Z".to_string(),
            },
        ];

        assert_eq!(dedup_commits(commits).len(), 2);
    }

    #[test]
    fn day_and_month_buckets_use_original_commit_instants_not_monday() {
        let commits = vec![
            CommitData {
                oid: None,
                additions: 1,
                deletions: 0,
                committed_date: "2025-01-31T23:59:59Z".to_string(),
            },
            CommitData {
                oid: None,
                additions: 2,
                deletions: 0,
                committed_date: "2025-02-01T00:00:00Z".to_string(),
            },
        ];
        let contributions = vec![RepoContribution {
            repo_name: "octocat/calendar".to_string(),
            total_commits: 2,
            total_additions: 3,
            total_deletions: 0,
            commits: commits.clone(),
            weeks: commits_to_weekly_buckets(&commits),
            languages: HashMap::from([("Rust".to_string(), 1)]),
        }];

        let day = contributions_to_period_stats(&contributions, &crate::cli::Period::Day);
        let month = contributions_to_period_stats(&contributions, &crate::cli::Period::Month);

        assert_eq!(
            day.iter()
                .map(|stat| stat.period_label.as_str())
                .collect::<Vec<_>>(),
            vec!["2025-01-31", "2025-02-01"]
        );
        assert_eq!(
            month
                .iter()
                .map(|stat| stat.period_label.as_str())
                .collect::<Vec<_>>(),
            vec!["2025-01", "2025-02"]
        );
    }

    #[test]
    fn batch_history_uses_and_filters_each_repository_window() {
        let active = vec![
            PageRequest {
                batch_index: 0,
                owner: "octocat".to_string(),
                name: "old".to_string(),
                since: Some("2025-01-01T00:00:00Z".to_string()),
                until_exclusive: Some("2025-01-04T00:00:00Z".to_string()),
                after: None,
            },
            PageRequest {
                batch_index: 1,
                owner: "octocat".to_string(),
                name: "new".to_string(),
                since: Some("2025-01-02T00:00:00Z".to_string()),
                until_exclusive: Some("2025-01-03T00:00:00Z".to_string()),
                after: None,
            },
        ];
        let query = build_batch_history_query(&active);
        let variables = build_batch_history_variables("USER_NODE_ID", &active);
        let new_repo_commits = vec![
            CommitData {
                oid: Some("old-commit".to_string()),
                additions: 1,
                deletions: 0,
                committed_date: "2025-01-01T00:00:00Z".to_string(),
            },
            CommitData {
                oid: Some("new-commit".to_string()),
                additions: 2,
                deletions: 0,
                committed_date: "2025-01-02T00:00:00Z".to_string(),
            },
        ];
        let filtered = filter_commits_to_range(
            &new_repo_commits,
            active[1].since.as_deref(),
            active[1].until_exclusive.as_deref(),
        )
        .unwrap();

        assert!(query.contains("$since0: GitTimestamp"));
        assert!(query.contains("$since1: GitTimestamp"));
        assert!(query.contains("nodes { oid additions deletions committedDate }"));
        assert_eq!(variables["since0"], json!("2025-01-01T00:00:00Z"));
        assert_eq!(variables["until0"], json!("2025-01-04T00:00:00Z"));
        assert_eq!(variables["since1"], json!("2025-01-02T00:00:00Z"));
        assert_eq!(variables["until1"], json!("2025-01-03T00:00:00Z"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].additions, 2);
    }

    enum StubResponse {
        DropConnection,
        Json {
            status: u16,
            body: String,
            headers: Vec<(String, String)>,
            delay: Duration,
        },
        OwnedJson {
            status: u16,
            body: String,
            delay: Duration,
        },
    }

    struct StubServer {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
        worker: thread::JoinHandle<()>,
    }

    impl StubServer {
        fn finish(self) -> Vec<String> {
            self.worker.join().unwrap();
            self.requests.lock().unwrap().clone()
        }
    }

    fn start_stub(responses: Vec<StubResponse>) -> StubServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded_requests = Arc::clone(&requests);
        let worker = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                match response {
                    StubResponse::DropConnection => {
                        recorded_requests.lock().unwrap().push(String::new());
                    }
                    StubResponse::Json {
                        status,
                        body,
                        headers,
                        delay,
                    } => {
                        recorded_requests
                            .lock()
                            .unwrap()
                            .push(read_stub_request(&mut stream));
                        thread::sleep(delay);
                        let headers = headers
                            .iter()
                            .map(|(name, value)| format!("{name}: {value}\r\n"))
                            .collect::<String>();
                        let response = format!(
                            "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    StubResponse::OwnedJson {
                        status,
                        body,
                        delay,
                    } => {
                        recorded_requests
                            .lock()
                            .unwrap()
                            .push(read_stub_request(&mut stream));
                        thread::sleep(delay);
                        let response = format!(
                            "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
            }
        });

        StubServer {
            base_url,
            requests,
            worker,
        }
    }

    fn read_stub_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    bytes.extend_from_slice(&buffer[..read]);
                    if let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let header = std::str::from_utf8(&bytes[..header_end]).unwrap();
                        let content_length = header
                            .lines()
                            .find_map(|line| line.strip_prefix("Content-Length: "))
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        if bytes.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("failed to read stub request: {error}"),
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn decode_query_component(component: &str) -> String {
        let mut decoded = Vec::with_capacity(component.len());
        let bytes = component.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'+' => {
                    decoded.push(b' ');
                    index += 1;
                }
                b'%' if index + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap();
                    decoded.push(u8::from_str_radix(hex, 16).unwrap());
                    index += 3;
                }
                byte => {
                    decoded.push(byte);
                    index += 1;
                }
            }
        }
        String::from_utf8(decoded).unwrap()
    }

    fn graphql_success_response() -> StubResponse {
        StubResponse::Json {
            status: 200,
            body: r#"{"data":{"ok":true}}"#.to_string(),
            headers: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    fn json_response(status: u16, body: impl Into<String>) -> StubResponse {
        StubResponse::Json {
            status,
            body: body.into(),
            headers: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    fn json_response_with_headers(
        status: u16,
        body: impl Into<String>,
        headers: &[(&str, &str)],
    ) -> StubResponse {
        StubResponse::Json {
            status,
            body: body.into(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            delay: Duration::ZERO,
        }
    }

    #[test]
    fn permission_403_fails_once_while_exhausted_403_uses_reset() {
        let server = start_stub(vec![json_response(403, "{}")]);
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO],
            Duration::from_secs(1),
        );

        let error = client
            .resolve_single_email_result("octocat", "repo", "octocat@example.com")
            .unwrap_err()
            .to_string();

        assert!(error.contains("permission"));
        assert_eq!(server.finish().len(), 1);

        let now = fixed_time(1);
        assert_eq!(
            retry_decision(
                reqwest::StatusCode::FORBIDDEN,
                &response_headers(&[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &(now.timestamp() + 5).to_string()),
                ]),
                Duration::ZERO,
                now,
                Duration::from_secs(120),
            ),
            RetryDecision::RetryAfter(Duration::from_secs(5))
        );
    }

    #[test]
    fn identity_report_truncates_and_canonicalizes_known_emails() {
        let commits = (0..20)
            .map(|index| {
                let email = match index {
                    0 => " Alice@Example.COM ",
                    1 => "alice@example.com",
                    2 => " BOB@Example.com",
                    _ => "bob@example.com ",
                };
                json!({ "commit": { "author": { "email": email } } })
            })
            .collect::<Vec<_>>();
        let server = start_stub(vec![
            json_response(
                200,
                r#"{"data":{"user":{"repositories":{"pageInfo":{"hasNextPage":true,"endCursor":"cursor"},"nodes":[{"name":"repo-one","owner":{"login":"octocat"},"isFork":false,"languages":{"edges":[]}}]}}}}"#,
            ),
            json_response_with_headers(
                200,
                serde_json::to_string(&commits).unwrap(),
                &[("Link", "<https://example.test/next>; rel=\"next\"")],
            ),
        ]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));

        let report = client.resolve_user_identity("octocat");

        assert_eq!(
            report.emails,
            BTreeSet::from([
                "alice@example.com".to_string(),
                "bob@example.com".to_string(),
            ])
        );
        assert_eq!(report.repositories_examined, 1);
        assert_eq!(report.logical_requests, 2);
        assert!(report.truncated_repositories);
        assert!(report.truncated_commits);
        assert!(report.failures.is_empty());
        assert!(report.is_partial());
        let warning = report.warning().unwrap();
        assert!(warning.contains("known emails"));
        assert!(warning.contains("may miss others"));
        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn identity_report_permission_403_records_failure_without_retry() {
        let server = start_stub(vec![
            json_response(
                200,
                r#"{"data":{"user":{"repositories":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"name":"repo-one","owner":{"login":"octocat"},"isFork":false,"languages":{"edges":[]}}]}}}}"#,
            ),
            json_response(403, "{}"),
        ]);
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO],
            Duration::from_secs(1),
        );

        let report = client.resolve_user_identity("octocat");

        assert!(report.is_partial());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(
            report.failures[0].repository.as_deref(),
            Some("octocat/repo-one")
        );
        assert_eq!(report.logical_requests, 2);
        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn graphql_retries_408_429_and_5xx_then_succeeds_with_a_bound() {
        let server = start_stub(vec![
            StubResponse::Json {
                status: 408,
                body: "{}".to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 429,
                body: "{}".to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 503,
                body: "{}".to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            graphql_success_response(),
        ]);
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO; 3],
            Duration::from_secs(1),
        );

        let data = client.graphql_query("query Test", &json!({})).unwrap();

        assert_eq!(data["ok"], true);
        assert_eq!(server.finish().len(), 4);
    }

    #[test]
    fn retry_exhaustion_returns_last_http_status() {
        let server = start_stub(
            (0..7)
                .map(|_| StubResponse::Json {
                    status: 503,
                    body: "{}".to_string(),
                    headers: Vec::new(),
                    delay: Duration::ZERO,
                })
                .collect(),
        );
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO; 6],
            Duration::from_secs(1),
        );

        let error = client
            .graphql_query("query Test", &json!({}))
            .unwrap_err()
            .to_string();

        assert!(error.contains("503"));
        assert_eq!(server.finish().len(), 7);
    }

    #[test]
    fn graphql_transient_transport_error_retries_then_succeeds() {
        let server = start_stub(vec![
            StubResponse::DropConnection,
            graphql_success_response(),
        ]);
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO],
            Duration::from_secs(1),
        );

        let data = client.graphql_query("query Test", &json!({})).unwrap();

        assert_eq!(data["ok"], true);
        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn graphql_timeout_retries_and_exhausts_with_a_wall_clock_bound() {
        let server = start_stub(
            (0..3)
                .map(|_| StubResponse::Json {
                    status: 200,
                    body: r#"{"data":{"ok":true}}"#.to_string(),
                    headers: Vec::new(),
                    delay: Duration::from_millis(100),
                })
                .collect(),
        );
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO; 2],
            Duration::from_millis(10),
        );
        let started = Instant::now();

        let error = client
            .graphql_query("query Test", &json!({}))
            .unwrap_err()
            .to_string();
        let requests = server.finish();

        assert!(error.contains("timed out"));
        assert_eq!(requests.len(), 3);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn rest_identity_retries_transient_status_then_succeeds() {
        let server = start_stub(vec![
            StubResponse::Json {
                status: 503,
                body: "{}".to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 200,
                body: r#"[{"author":{"login":"alice"}}]"#.to_string(),
                headers: Vec::new(),
                delay: Duration::ZERO,
            },
        ]);
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO],
            Duration::from_secs(1),
        );

        let login = client
            .resolve_single_email_result("owner", "repo", "alice@example.com")
            .unwrap();

        assert_eq!(login.as_deref(), Some("alice"));
        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn rest_identity_retry_exhaustion_is_bounded_and_visible() {
        let server = start_stub(
            (0..3)
                .map(|_| StubResponse::Json {
                    status: 503,
                    body: "{}".to_string(),
                    headers: Vec::new(),
                    delay: Duration::ZERO,
                })
                .collect(),
        );
        let client = GithubClient::for_test(
            &server.base_url,
            vec![Duration::ZERO; 2],
            Duration::from_secs(1),
        );

        let error = client
            .resolve_single_email_result("owner", "repo", "alice@example.com")
            .unwrap_err()
            .to_string();

        assert!(error.contains("503"));
        assert_eq!(server.finish().len(), 3);
    }

    #[test]
    fn rest_author_query_is_url_encoded() {
        let server = start_stub(vec![json_response(200, "[]")]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));
        let email = "a+b @example.com";

        assert_eq!(
            client
                .resolve_single_email_result("owner", "repo", email)
                .unwrap(),
            None
        );
        let request = server.finish().pop().unwrap();
        let target = request.split_whitespace().nth(1).unwrap();
        let author = target
            .split_once('?')
            .unwrap()
            .1
            .split('&')
            .find_map(|component| component.strip_prefix("author="))
            .unwrap();

        assert!(author.contains("%2B"));
        assert!(author.contains("%40"));
        assert!(author.contains("+") || author.contains("%20"));
        assert_eq!(decode_query_component(author), email);
    }

    #[test]
    fn has_next_page_without_cursor_is_an_incomplete_response_error() {
        let error = pagination_decision(true, None, 1, 20, "history for octocat/repo")
            .unwrap_err()
            .to_string();

        assert!(error.contains("incomplete"));
        assert!(error.contains("history for octocat/repo"));
    }

    #[test]
    fn pagination_caps_warn_for_history_owned_contributed_and_private_scopes() {
        for (scope, limit) in [
            ("history for octocat/repo", 20),
            ("owned repositories", 300),
            ("contributed repositories", 300),
            ("private repositories", 10),
        ] {
            let PaginationDecision::Capped(warning) =
                pagination_decision(true, Some("next"), limit, limit, scope).unwrap()
            else {
                panic!("expected a cap warning for {scope}");
            };
            assert!(warning.contains(scope));
            assert!(warning.contains(&limit.to_string()));
        }
    }

    #[test]
    fn github_group_node_tree_follows_plan_level_order_and_preserves_totals() {
        let contributions = vec![
            RepoContribution {
                repo_name: "repo-a".to_string(),
                total_commits: 1,
                total_additions: 7,
                total_deletions: 2,
                commits: vec![CommitData {
                    oid: Some("repo-a-1".to_string()),
                    additions: 7,
                    deletions: 2,
                    committed_date: "2025-01-15T12:00:00Z".to_string(),
                }],
                weeks: vec![],
                languages: HashMap::from([("Rust".to_string(), 1)]),
            },
            RepoContribution {
                repo_name: "repo-b".to_string(),
                total_commits: 1,
                total_additions: 5,
                total_deletions: 1,
                commits: vec![CommitData {
                    oid: Some("repo-b-1".to_string()),
                    additions: 5,
                    deletions: 1,
                    committed_date: "2025-02-15T12:00:00Z".to_string(),
                }],
                weeks: vec![],
                languages: HashMap::from([("TypeScript".to_string(), 1)]),
            },
        ];

        let nodes = contributions_to_group_tree(
            &contributions,
            &[
                crate::cli::GroupBy::Repo,
                crate::cli::GroupBy::Period,
                crate::cli::GroupBy::Language,
            ],
            &crate::cli::Period::Month,
        );

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
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.stats.total_deletions)
                .sum::<u64>(),
            3
        );
        assert_eq!(nodes[0].children[0].stats.by_language["Rust"].additions, 7);
        assert_eq!(
            nodes[1].children[0].stats.by_language["TypeScript"].additions,
            5
        );
    }

    #[test]
    fn github_group_cardinality_uses_exact_repos_periods_and_languages() {
        let contributions = vec![RepoContribution {
            repo_name: "repo-a".to_string(),
            total_commits: 1,
            total_additions: 1,
            total_deletions: 0,
            commits: vec![CommitData {
                oid: Some("repo-a-1".to_string()),
                additions: 1,
                deletions: 0,
                committed_date: "2025-01-15T12:00:00Z".to_string(),
            }],
            weeks: vec![],
            languages: HashMap::new(),
        }];

        let counts = contribution_group_cardinality(&contributions, &crate::cli::Period::Month);

        assert_eq!(counts.repo, 1);
        assert_eq!(counts.author, 0);
        assert_eq!(counts.period, 1);
        assert_eq!(counts.language, 1);
    }
}
