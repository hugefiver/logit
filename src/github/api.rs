use anyhow::Context;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::cache::DiskCache;
use crate::cli::{GroupBy, Period};
use crate::stats::models::{GroupNode, PeriodStats};

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
                    if status.as_u16() == 403 {
                        if let Some(wait) = Self::parse_rate_limit_wait(&response) {
                            if attempt + 1 < max_attempts {
                                let secs = wait.min(120);
                                eprintln!(
                                    "\nRate limited by GitHub API. Waiting {secs}s before retry (attempt {}/{})...",
                                    attempt + 1,
                                    max_attempts
                                );
                                std::thread::sleep(std::time::Duration::from_secs(secs));
                                continue;
                            }
                            anyhow::bail!(
                                "GitHub {scope} request failed after {n} retries. Last status: 403.",
                                n = max_attempts - 1
                            );
                        }
                        anyhow::bail!("GitHub {scope} request failed with status {status}.");
                    }

                    let retryable = matches!(status.as_u16(), 408 | 429 | 500..=599);
                    if status.is_client_error() && !retryable {
                        anyhow::bail!("GitHub {scope} request failed with status {status}.");
                    }
                    if !retryable {
                        return Ok(response);
                    }
                    if attempt + 1 == max_attempts {
                        anyhow::bail!(
                            "GitHub {scope} request failed after {n} retries. Last status: {status}.",
                            n = max_attempts - 1
                        );
                    }

                    let delay = self.retry_delays[attempt];
                    eprintln!(
                        "\nGitHub {scope} request returned {status}. Retrying in {}s (attempt {}/{})...",
                        delay.as_secs(),
                        attempt + 1,
                        max_attempts,
                    );
                    std::thread::sleep(delay);
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

    fn parse_rate_limit_wait(resp: &reqwest::blocking::Response) -> Option<u64> {
        if let Some(retry_after) = resp.headers().get("retry-after")
            && let Ok(secs) = retry_after.to_str().unwrap_or("").parse::<u64>()
        {
            return Some(secs);
        }
        if let Some(reset) = resp.headers().get("x-ratelimit-reset")
            && let Ok(ts) = reset.to_str().unwrap_or("").parse::<i64>()
        {
            let now = Utc::now().timestamp();
            let wait = (ts - now).max(1) as u64;
            return Some(wait);
        }
        None
    }

    pub fn get_user(&self, username: &str) -> anyhow::Result<GithubUser> {
        let variables = serde_json::json!({ "login": username });
        let data = self.graphql_query(USER_QUERY, &variables)?;
        parse_graphql_user_data(data, username)
    }

    pub fn get_contribution_repos(
        &self,
        username: &str,
        since: Option<i64>,
        until: Option<i64>,
        include_forks: bool,
        include_contributed: bool,
    ) -> anyhow::Result<(Vec<(RepoWithLangs, u64)>, ContributionSummary)> {
        let now = effective_window_end(until);
        let windows = contribution_windows(since, now);
        let mut merged: HashMap<String, (RepoWithLangs, u64)> = HashMap::new();
        let mut total_summary = ContributionSummary::default();
        let mut has_saturated_window = false;

        for (from, to) in windows {
            let variables = contribution_query_variables(username, &from, &to)?;
            let data = self.graphql_query(CONTRIBUTIONS_QUERY, &variables)?;
            let (repos, summary) = parse_contributions_collection_data(data, username)?;
            has_saturated_window |= contribution_window_is_saturated(&repos);
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

        if has_saturated_window {
            eprintln!("Warning: {}", contribution_partial_data_warning());
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

        Ok((repos, total_summary))
    }

    fn batch_commit_history(
        &self,
        user_node_id: &str,
        repos: &[RepoHistoryRequest],
    ) -> anyhow::Result<BatchCommitHistory> {
        const MAX_PAGES_PER_REPO: usize = 20; // safety cap: 20 * 100 = 2000 commits per repo per window
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
                        MAX_PAGES_PER_REPO,
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

    fn resolve_single_email_result(
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

    pub fn resolve_user_emails(&self, username: &str) -> anyhow::Result<Vec<String>> {
        let repos = self.list_user_repos_graphql(username, false)?;
        let mut emails: std::collections::HashSet<String> = std::collections::HashSet::new();

        for repo in repos.iter().take(8) {
            let mut url = reqwest::Url::parse(&format!(
                "{}/repos/{}/{}/commits",
                self.rest_base_url.trim_end_matches('/'),
                repo.owner,
                repo.name
            ))
            .with_context(|| {
                format!(
                    "failed to build GitHub REST commit email URL for {}/{}",
                    repo.owner, repo.name
                )
            })?;
            url.query_pairs_mut()
                .append_pair("author", username)
                .append_pair("per_page", "20");
            match self.send_with_retry(|| self.client.get(url.clone()), "REST commit email lookup")
            {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<Vec<serde_json::Value>>() {
                        Ok(commits) => {
                            for commit in commits {
                                if let Some(email) = commit
                                    .pointer("/commit/author/email")
                                    .and_then(|v| v.as_str())
                                    .filter(|e| !e.is_empty())
                                {
                                    emails.insert(email.to_string());
                                }
                            }
                        }
                        Err(error) => eprintln!(
                            "Warning: failed to parse commit emails for {}/{}: {error}",
                            repo.owner, repo.name
                        ),
                    }
                }
                Ok(resp) => eprintln!(
                    "Warning: GitHub commit email lookup for {}/{} failed with status {}.",
                    repo.owner,
                    repo.name,
                    resp.status()
                ),
                Err(error) => eprintln!(
                    "Warning: GitHub commit email lookup for {}/{} failed: {error}",
                    repo.owner, repo.name
                ),
            }
        }

        if emails.is_empty() {
            anyhow::bail!(
                "No commit emails found for GitHub user '{username}'. The user may have no public repos or commits."
            );
        }

        let mut sorted: Vec<String> = emails.into_iter().collect();
        sorted.sort();
        Ok(sorted)
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
            match self.resolve_single_email_result(owner, repo, email) {
                Ok(Some(login)) => {
                    map.insert(email.clone(), login);
                }
                Ok(None) => {}
                Err(error) => eprintln!(
                    "Warning: failed to resolve GitHub identity for '{email}' in {owner}/{repo}: {error}"
                ),
            }
        }
        map
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    since: Option<i64>,
    now: DateTime<Utc>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let one_year_ago = now - Duration::days(365);
    let start = since
        .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
        .unwrap_or(one_year_ago);

    if start >= now {
        return vec![(now - Duration::minutes(1), now)];
    }

    let mut windows = Vec::new();
    let mut window_start = start;

    while window_start < now {
        let candidate_end = window_start + Duration::days(365);
        let window_end = if candidate_end < now {
            candidate_end
        } else {
            now
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

const CACHE_SCHEMA_VERSION: &str = "v3";

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

fn cache_optional_string_component(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("some_{}", cache_string_component(value)),
        None => "none".to_string(),
    }
}

fn contribution_cache_key(
    user_node_id: &str,
    username: &str,
    from: &DateTime<Utc>,
    to: &DateTime<Utc>,
    include_forks: bool,
    include_contributed: bool,
) -> String {
    format!(
        "{CACHE_SCHEMA_VERSION}_contribution_{}_{}_{}_{}_forks_{}_contributed_{}",
        cache_string_component(user_node_id),
        cache_string_component(username),
        cache_string_component(&from.to_rfc3339()),
        cache_string_component(&to.to_rfc3339()),
        include_forks as u8,
        include_contributed as u8,
    )
}

fn history_cache_key(
    user_node_id: &str,
    owner: &str,
    name: &str,
    since: Option<&str>,
    until: Option<&str>,
    include_private: bool,
) -> String {
    format!(
        "{CACHE_SCHEMA_VERSION}_history_{}_{}_{}_{}_{}_private_{}",
        cache_string_component(user_node_id),
        cache_string_component(owner),
        cache_string_component(name),
        cache_optional_string_component(since),
        cache_optional_string_component(until),
        include_private as u8,
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

#[derive(Serialize, Deserialize)]
struct CachedContributionWindow {
    repos: Vec<(RepoWithLangs, u64)>,
    summary: ContributionSummary,
    #[serde(default)]
    saturated: bool,
}

#[derive(Serialize, Deserialize)]
struct CachedCommitHistory {
    since: String,
    /// Latest committedDate among cached commits (data-derived, used as gap fetch start).
    until: String,
    /// Query's `until_iso` from the last cache write (used to detect if a new gap exists).
    /// Empty string on old cache entries; falls back to `until` for backward compat.
    #[serde(default)]
    checked_until: String,
    commits: Vec<CommitData>,
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

fn latest_commit_date(commits: &[CommitData]) -> anyhow::Result<Option<String>> {
    let mut latest: Option<(DateTime<Utc>, String)> = None;

    for commit in commits {
        let committed_at =
            parse_rfc3339_instant(&commit.committed_date, "cached commit committedDate")?;
        if latest
            .as_ref()
            .is_none_or(|(latest_at, _)| committed_at > *latest_at)
        {
            latest = Some((committed_at, commit.committed_date.clone()));
        }
    }

    Ok(latest.map(|(_, committed_date)| committed_date))
}

fn parse_rfc3339_instant(value: &str, context: &str) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("{context} '{value}' is not a valid RFC3339 timestamp"))
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn contribution_window_is_saturated(repos: &[(RepoWithLangs, u64)]) -> bool {
    repos.len() >= 100
}

fn inactive_gap_repos(
    to_fetch: &[RepoHistoryRequest],
    gap_repo_keys: &HashSet<String>,
    active: &[(RepoWithLangs, u64)],
) -> Option<Vec<(String, String)>> {
    if contribution_window_is_saturated(active) {
        return None;
    }

    let active_keys: HashSet<String> = active
        .iter()
        .map(|(repo, _)| repo_key(&repo.owner, &repo.name))
        .collect();
    Some(
        to_fetch
            .iter()
            .filter(|request| {
                let key = repo_key(&request.owner, &request.name);
                gap_repo_keys.contains(&key) && !active_keys.contains(&key)
            })
            .map(|request| (request.owner.clone(), request.name.clone()))
            .collect(),
    )
}

fn contribution_partial_data_warning() -> &'static str {
    "GitHub contribution repository data may be partial: commitContributionsByRepository reached its 100-repository limit."
}

fn validate_cached_commit_history(history: &CachedCommitHistory) -> anyhow::Result<()> {
    let since = parse_rfc3339_instant(&history.since, "cached history start")?;
    let until = parse_rfc3339_instant(&history.until, "cached history end")?;
    if since > until {
        anyhow::bail!("cached history start is after its data end");
    }

    if !history.checked_until.is_empty() {
        let checked_until =
            parse_rfc3339_instant(&history.checked_until, "cached history check end")?;
        if checked_until < until {
            anyhow::bail!("cached history check end is before its data end");
        }
    }

    for commit in &history.commits {
        parse_rfc3339_instant(&commit.committed_date, "cached commit committedDate")?;
    }

    Ok(())
}

fn history_cache_get_or_warn(
    cache: &DiskCache,
    key: &str,
    warnings: &mut CacheWarnings,
) -> Option<CachedCommitHistory> {
    let cached = cache_get_or_warn(cache, key, warnings)?;
    if let Err(error) = validate_cached_commit_history(&cached) {
        warnings.push(format!(
            "GitHub history cache entry for key '{key}' is invalid; treating it as a cache miss: {error}"
        ));
        return None;
    }
    Some(cached)
}

fn history_cache_write_allowed(
    request: &RepoHistoryRequest,
    capped_repos: &HashSet<String>,
) -> bool {
    !capped_repos.contains(&repo_key(&request.owner, &request.name))
}

#[allow(clippy::too_many_arguments)]
fn get_contribution_repos_cached(
    client: &GithubClient,
    cache: &DiskCache,
    user_node_id: &str,
    username: &str,
    since: Option<i64>,
    until: Option<i64>,
    include_forks: bool,
    include_contributed: bool,
    read_cache: bool,
    write_cache: bool,
    warnings: &mut CacheWarnings,
) -> anyhow::Result<(Vec<(RepoWithLangs, u64)>, ContributionSummary)> {
    let now = effective_window_end(until);
    let today = Utc::now().date_naive();
    let windows = contribution_windows(since, now);
    let mut merged: HashMap<String, (RepoWithLangs, u64)> = HashMap::new();
    let mut accumulated_summary = ContributionSummary::default();

    for (from, to) in windows {
        let key = contribution_cache_key(
            user_node_id,
            username,
            &from,
            &to,
            include_forks,
            include_contributed,
        );

        let window_completed = to.date_naive() < today;

        let cached: Option<CachedContributionWindow> =
            if read_cache && (window_completed || !write_cache) {
                cache_get_or_warn(cache, &key, warnings)
            } else {
                None
            };

        let (repos_chunk, summary_chunk, saturated): (
            Vec<(RepoWithLangs, u64)>,
            ContributionSummary,
            bool,
        ) = if let Some(cached) = cached {
            let saturated = cached.saturated || contribution_window_is_saturated(&cached.repos);
            (cached.repos, cached.summary, saturated)
        } else {
            let variables = contribution_query_variables(username, &from, &to)?;
            let data = client.graphql_query(CONTRIBUTIONS_QUERY, &variables)?;
            let (repos, summary) = parse_contributions_collection_data(data, username)?;
            let saturated = contribution_window_is_saturated(&repos);
            if write_cache {
                cache_set_or_warn(
                    cache,
                    &key,
                    &CachedContributionWindow {
                        repos: repos.clone(),
                        summary: summary.clone(),
                        saturated,
                    },
                    warnings,
                );
            }
            (repos, summary, saturated)
        };

        if saturated {
            warnings.push(contribution_partial_data_warning());
        }

        accumulated_summary.total_prs += summary_chunk.total_prs;
        accumulated_summary.total_reviews += summary_chunk.total_reviews;
        accumulated_summary.total_issues += summary_chunk.total_issues;

        for (repo, commit_count) in repos_chunk {
            let repo_id = repo_key(&repo.owner, &repo.name);
            if let Some((existing, total)) = merged.get_mut(&repo_id) {
                *total += commit_count;
                if existing.languages.is_empty() && !repo.languages.is_empty() {
                    existing.languages = repo.languages;
                }
            } else {
                merged.insert(repo_id, (repo, commit_count));
            }
        }
    }

    let mut repo_rows: Vec<(RepoWithLangs, u64)> = merged.into_values().collect();
    if !include_forks {
        repo_rows.retain(|(repo, _)| !repo.is_fork);
    }
    if !include_contributed {
        repo_rows.retain(|(repo, _)| repo.owner.eq_ignore_ascii_case(username));
    }

    repo_rows.sort_by(|(a, _), (b, _)| {
        a.owner
            .to_lowercase()
            .cmp(&b.owner.to_lowercase())
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok((repo_rows, accumulated_summary))
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
    since: Option<i64>,
    until: Option<i64>,
    read_cache: bool,
    write_cache: bool,
) -> anyhow::Result<(Vec<RepoContribution>, ContributionSummary)> {
    let mut cache_warnings = CacheWarnings::default();
    let cache = if read_cache || write_cache {
        cache_init_or_warn(DiskCache::new(), &mut cache_warnings)
    } else {
        None
    };
    let now = effective_window_end(until);

    let (mut repo_rows, contribution_summary) = if let Some(cache) = cache.as_ref() {
        get_contribution_repos_cached(
            client,
            cache,
            user_node_id,
            username,
            since,
            until,
            include_forks,
            include_contributed,
            read_cache,
            write_cache,
            &mut cache_warnings,
        )?
    } else {
        client.get_contribution_repos(username, since, until, include_forks, include_contributed)?
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

    let until_iso = Some(now.to_rfc3339());
    let default_since_ts = (now - Duration::days(365)).timestamp();
    let effective_since_ts = since.unwrap_or(default_since_ts);
    let since_iso = Utc
        .timestamp_opt(effective_since_ts, 0)
        .single()
        .map(|dt| dt.to_rfc3339());

    let mut commit_history_by_repo: HashMap<String, Vec<CommitData>> = HashMap::new();
    let mut to_fetch: Vec<RepoHistoryRequest> = Vec::new();
    let mut gap_repo_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (repo, _) in &repo_rows {
        let history_key = history_cache_key(
            user_node_id,
            &repo.owner,
            &repo.name,
            since_iso.as_deref(),
            until_iso.as_deref(),
            include_private,
        );
        let repo_name = format!("{}/{}", repo.owner, repo.name);

        let cached: Option<CachedCommitHistory> = if read_cache {
            cache
                .as_ref()
                .and_then(|c| history_cache_get_or_warn(c, &history_key, &mut cache_warnings))
        } else {
            None
        };

        match &cached {
            Some(ch) if write_cache => {
                let overlap = filter_commits_to_range(
                    &ch.commits,
                    since_iso.as_deref(),
                    until_iso.as_deref(),
                )?;
                let checked = if ch.checked_until.is_empty() {
                    &ch.until
                } else {
                    &ch.checked_until
                };
                let fetch_since = until_iso
                    .as_deref()
                    .map(|until| {
                        let checked_at =
                            parse_rfc3339_instant(checked, "cached history check end")?;
                        let until_at = parse_rfc3339_instant(until, "requested history range end")?;
                        Ok::<_, anyhow::Error>((checked_at < until_at).then(|| ch.until.clone()))
                    })
                    .transpose()?
                    .flatten();
                commit_history_by_repo.insert(repo_name, overlap);
                if let Some(fs) = fetch_since {
                    gap_repo_keys.insert(repo_key(&repo.owner, &repo.name));
                    to_fetch.push(RepoHistoryRequest {
                        owner: repo.owner.clone(),
                        name: repo.name.clone(),
                        since: Some(fs),
                        until_exclusive: until_iso.clone(),
                    });
                }
            }
            Some(ch) => {
                let in_range = filter_commits_to_range(
                    &ch.commits,
                    since_iso.as_deref(),
                    until_iso.as_deref(),
                )?;
                commit_history_by_repo.insert(repo_name, in_range);
            }
            None if write_cache => {
                to_fetch.push(RepoHistoryRequest {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    since: since_iso.clone(),
                    until_exclusive: until_iso.clone(),
                });
            }
            None => {
                to_fetch.push(RepoHistoryRequest {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    since: since_iso.clone(),
                    until_exclusive: until_iso.clone(),
                });
            }
        }
    }

    if !gap_repo_keys.is_empty() {
        let min_gap_since = to_fetch
            .iter()
            .filter(|request| gap_repo_keys.contains(&repo_key(&request.owner, &request.name)))
            .filter_map(|request| request.since.as_deref())
            .min()
            .unwrap_or("");
        let gap_until = until_iso.as_deref().unwrap_or("");

        let gap_from = parse_rfc3339_instant(min_gap_since, "contribution gap query start")?;
        let gap_to = parse_rfc3339_instant(gap_until, "contribution gap query end")?;
        let variables = contribution_query_variables(username, &gap_from, &gap_to)?;
        if let Ok(data) = client.graphql_query(CONTRIBUTIONS_QUERY, &variables)
            && let Ok((active, _)) = parse_contributions_collection_data(data, username)
        {
            if let Some(inactive) = inactive_gap_repos(&to_fetch, &gap_repo_keys, &active) {
                let inactive_keys: HashSet<String> = inactive
                    .iter()
                    .map(|(owner, name)| repo_key(owner, name))
                    .collect();
                to_fetch.retain(|request| {
                    !inactive_keys.contains(&repo_key(&request.owner, &request.name))
                });

                if write_cache && let Some(c) = &cache {
                    for (owner, name) in &inactive {
                        let history_key = history_cache_key(
                            user_node_id,
                            owner,
                            name,
                            since_iso.as_deref(),
                            until_iso.as_deref(),
                            include_private,
                        );
                        if let Some(mut ch) =
                            history_cache_get_or_warn(c, &history_key, &mut cache_warnings)
                        {
                            ch.checked_until = gap_until.to_string();
                            cache_set_or_warn(c, &history_key, &ch, &mut cache_warnings);
                        }
                    }
                }

                if !inactive.is_empty() {
                    eprintln!(
                        "Skipped {} repos with no new activity in gap period",
                        inactive.len()
                    );
                }
            } else {
                cache_warnings.push(contribution_partial_data_warning());
            }
        }
    }

    if !to_fetch.is_empty() {
        for batch in to_fetch.chunks(5) {
            let fetched = client.batch_commit_history(user_node_id, batch)?;

            for request in batch {
                let repo_name = format!("{}/{}", request.owner, request.name);
                let new_commits = filter_commits_to_range(
                    &fetched.commits.get(&repo_name).cloned().unwrap_or_default(),
                    request.since.as_deref(),
                    request.until_exclusive.as_deref(),
                )
                .with_context(|| {
                    format!(
                        "failed to filter fetched commit history for {}/{} before merge",
                        request.owner, request.name
                    )
                })?;

                let merged = if let Some(mut existing) = commit_history_by_repo.remove(&repo_name) {
                    existing.extend(new_commits);
                    dedup_commits(existing)
                } else {
                    new_commits
                };

                if write_cache
                    && history_cache_write_allowed(request, &fetched.capped_repos)
                    && let Some(c) = &cache
                {
                    let history_key = history_cache_key(
                        user_node_id,
                        &request.owner,
                        &request.name,
                        since_iso.as_deref(),
                        until_iso.as_deref(),
                        include_private,
                    );
                    let default_since = since_iso.clone().unwrap_or_default();
                    let default_until = until_iso.clone().unwrap_or_default();
                    let data_until = latest_commit_date(&merged)?.unwrap_or(default_until.clone());
                    let cached_entry = CachedCommitHistory {
                        since: default_since,
                        until: data_until,
                        checked_until: default_until.clone(),
                        commits: merged.clone(),
                    };
                    cache_set_or_warn(c, &history_key, &cached_entry, &mut cache_warnings);
                }
                commit_history_by_repo.insert(repo_name, merged);
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

fn effective_window_end(until: Option<i64>) -> DateTime<Utc> {
    let now = Utc::now();
    until
        .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
        .map(|dt| dt.min(now))
        .unwrap_or(now)
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

    #[test]
    fn history_cache_key_is_user_scoped_and_component_collision_free() {
        let since = Some("2025-01-01T00:00:00Z");
        let until = Some("2025-02-01T00:00:00Z");
        let alice = history_cache_key("node-alice", "octo/org", "repo", since, until, false);
        let bob = history_cache_key("node-bob", "octo/org", "repo", since, until, false);
        let slash = history_cache_key("node-alice", "octo/org", "repo", since, until, false);
        let underscore = history_cache_key("node-alice", "octo_org", "repo", since, until, false);
        let different_range = history_cache_key(
            "node-alice",
            "octo/org",
            "repo",
            Some("2025-01-02T00:00:00Z"),
            until,
            false,
        );
        let private = history_cache_key("node-alice", "octo/org", "repo", since, until, true);

        assert!(alice.starts_with("v3_"));
        assert_ne!(alice, bob);
        assert_ne!(slash, underscore);
        assert_ne!(alice, different_range);
        assert_ne!(alice, private);

        let from = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap();
        let contribution =
            contribution_cache_key("node-alice", "octo/org", &from, &to, false, false);
        let contribution_user =
            contribution_cache_key("node-bob", "octo/org", &from, &to, false, false);
        let contribution_component =
            contribution_cache_key("node-alice", "octo_org", &from, &to, false, false);
        let contribution_range = contribution_cache_key(
            "node-alice",
            "octo/org",
            &from,
            &Utc.with_ymd_and_hms(2025, 2, 2, 0, 0, 0).unwrap(),
            false,
            false,
        );
        let contribution_mode =
            contribution_cache_key("node-alice", "octo/org", &from, &to, true, true);

        assert!(contribution.starts_with("v3_"));
        assert_ne!(contribution, contribution_user);
        assert_ne!(contribution, contribution_component);
        assert_ne!(contribution, contribution_range);
        assert_ne!(contribution, contribution_mode);
    }

    #[test]
    fn v2_cache_entries_are_misses_for_v3_contribution_and_history_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let contribution_key =
            contribution_cache_key("node-octocat", "octocat", &from, &to, false, false);
        let history_key = history_cache_key(
            "node-octocat",
            "octocat",
            "hello-world",
            Some("2020-01-01T00:00:00Z"),
            Some("2020-01-02T00:00:00Z"),
            false,
        );
        let v2_contribution_key = contribution_key.replacen("v3_", "v2_", 1);
        let v2_history_key = history_key.replacen("v3_", "v2_", 1);

        cache
            .set(
                &v2_contribution_key,
                &CachedContributionWindow {
                    repos: vec![(sample_repo(), 1)],
                    summary: ContributionSummary::default(),
                    saturated: false,
                },
            )
            .unwrap();
        cache
            .set(
                &v2_history_key,
                &CachedCommitHistory {
                    since: "2020-01-01T00:00:00Z".to_string(),
                    until: "2020-01-02T00:00:00Z".to_string(),
                    checked_until: "2020-01-02T00:00:00Z".to_string(),
                    commits: vec![],
                },
            )
            .unwrap();

        assert!(
            cache
                .get::<CachedContributionWindow>(&contribution_key)
                .unwrap()
                .is_none(),
            "current contribution key must not load a v2 payload"
        );
        assert!(
            cache
                .get::<CachedCommitHistory>(&history_key)
                .unwrap()
                .is_none(),
            "current history key must not load a v2 payload"
        );
        assert!(contribution_key.starts_with("v3_"));
        assert!(history_key.starts_with("v3_"));

        cache
            .set(
                &contribution_key,
                &CachedContributionWindow {
                    repos: vec![(sample_repo(), 7)],
                    summary: ContributionSummary::default(),
                    saturated: false,
                },
            )
            .unwrap();
        cache
            .set(
                &history_key,
                &CachedCommitHistory {
                    since: "2020-01-01T00:00:00Z".to_string(),
                    until: "2020-01-02T00:00:00Z".to_string(),
                    checked_until: "2020-01-02T00:00:00Z".to_string(),
                    commits: vec![CommitData {
                        oid: Some("current".to_string()),
                        additions: 7,
                        deletions: 0,
                        committed_date: "2020-01-01T12:00:00Z".to_string(),
                    }],
                },
            )
            .unwrap();

        let contribution = cache
            .get::<CachedContributionWindow>(&contribution_key)
            .unwrap()
            .unwrap();
        let history = cache
            .get::<CachedCommitHistory>(&history_key)
            .unwrap()
            .unwrap();
        assert_eq!(contribution.repos[0].1, 7);
        assert_eq!(history.commits[0].oid.as_deref(), Some("current"));
    }

    #[test]
    fn completed_contribution_cache_hit_restores_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let key = contribution_cache_key("node-octocat", "octocat", &from, &to, false, false);
        let expected_summary = ContributionSummary {
            total_prs: 3,
            total_reviews: 5,
            total_issues: 7,
        };
        cache
            .set(
                &key,
                &CachedContributionWindow {
                    repos: vec![(sample_repo(), 11)],
                    summary: expected_summary.clone(),
                    saturated: false,
                },
            )
            .unwrap();

        let client =
            GithubClient::for_test("http://127.0.0.1:1", Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();
        let (repos, summary) = get_contribution_repos_cached(
            &client,
            &cache,
            "node-octocat",
            "octocat",
            Some(from.timestamp()),
            Some(to.timestamp()),
            false,
            false,
            true,
            false,
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
        let windows = contribution_windows(Some(from.timestamp()), until);
        let contribution_response = r#"{"data":{"user":{"contributionsCollection":{"totalPullRequestContributions":0,"totalPullRequestReviewContributions":0,"totalIssueContributions":0,"commitContributionsByRepository":[]}}}}"#;
        let server = start_stub(vec![
            StubResponse::Json {
                status: 200,
                body: contribution_response,
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 200,
                body: contribution_response,
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 200,
                body: contribution_response,
                delay: Duration::ZERO,
            },
        ]);
        let client = GithubClient::for_test(&server.base_url, Vec::new(), Duration::from_secs(1));

        client
            .get_contribution_repos(
                "octocat",
                Some(from.timestamp()),
                Some(until.timestamp()),
                false,
                false,
            )
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
        let repos = vec![(sample_repo(), 1); 100];
        let mut warnings = CacheWarnings::default();

        if contribution_window_is_saturated(&repos) {
            warnings.push(contribution_partial_data_warning());
        }

        assert!(contribution_window_is_saturated(&repos));
        assert_eq!(warnings.messages, vec![contribution_partial_data_warning()]);
    }

    #[test]
    fn saturated_contribution_cache_hit_replays_partial_data_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let key = contribution_cache_key("node-octocat", "octocat", &from, &to, false, false);
        cache
            .set(
                &key,
                &CachedContributionWindow {
                    repos: vec![(sample_repo(), 100)],
                    summary: ContributionSummary::default(),
                    saturated: true,
                },
            )
            .unwrap();
        let client =
            GithubClient::for_test("http://127.0.0.1:1", Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();

        let (repos, summary) = get_contribution_repos_cached(
            &client,
            &cache,
            "node-octocat",
            "octocat",
            Some(from.timestamp()),
            Some(to.timestamp()),
            false,
            false,
            true,
            false,
            &mut warnings,
        )
        .unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].0.name, "hello-world");
        assert_eq!(repos[0].1, 100);
        assert_eq!(summary.total_prs, 0);
        assert_eq!(warnings.messages, vec![contribution_partial_data_warning()]);
    }

    #[test]
    fn current_schema_saturated_contribution_cache_hit_replays_partial_data_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let from = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2020, 1, 2, 0, 0, 0).unwrap();
        let key = contribution_cache_key("node-octocat", "octocat", &from, &to, false, false);
        let mut legacy_payload = serde_json::to_value(&CachedContributionWindow {
            repos: vec![(sample_repo(), 1); 100],
            summary: ContributionSummary::default(),
            saturated: false,
        })
        .unwrap();
        legacy_payload.as_object_mut().unwrap().remove("saturated");
        std::fs::write(
            tmp.path().join(format!("{key}.json")),
            serde_json::to_string(&legacy_payload).unwrap(),
        )
        .unwrap();

        let cached = cache
            .get::<CachedContributionWindow>(&key)
            .unwrap()
            .unwrap();
        assert!(!cached.saturated);

        let client =
            GithubClient::for_test("http://127.0.0.1:1", Vec::new(), Duration::from_secs(1));
        let mut warnings = CacheWarnings::default();
        let (repos, _) = get_contribution_repos_cached(
            &client,
            &cache,
            "node-octocat",
            "octocat",
            Some(from.timestamp()),
            Some(to.timestamp()),
            false,
            false,
            true,
            false,
            &mut warnings,
        )
        .unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(warnings.messages, vec![contribution_partial_data_warning()]);
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
    fn saturated_gap_activity_cannot_prove_repositories_inactive() {
        let to_fetch = vec![
            RepoHistoryRequest {
                owner: "octocat".to_string(),
                name: "hello-world".to_string(),
                since: None,
                until_exclusive: None,
            },
            RepoHistoryRequest {
                owner: "octocat".to_string(),
                name: "inactive".to_string(),
                since: None,
                until_exclusive: None,
            },
        ];
        let gap_repo_keys = HashSet::from([
            repo_key("octocat", "hello-world"),
            repo_key("octocat", "inactive"),
        ]);

        assert_eq!(
            inactive_gap_repos(&to_fetch, &gap_repo_keys, &vec![(sample_repo(), 1); 100]),
            None
        );
    }

    #[test]
    fn complete_gap_activity_skips_repositories_without_activity() {
        let to_fetch = vec![
            RepoHistoryRequest {
                owner: "octocat".to_string(),
                name: "hello-world".to_string(),
                since: None,
                until_exclusive: None,
            },
            RepoHistoryRequest {
                owner: "octocat".to_string(),
                name: "inactive".to_string(),
                since: None,
                until_exclusive: None,
            },
        ];
        let gap_repo_keys = HashSet::from([
            repo_key("octocat", "hello-world"),
            repo_key("octocat", "inactive"),
        ]);

        assert_eq!(
            inactive_gap_repos(&to_fetch, &gap_repo_keys, &[(sample_repo(), 1)]),
            Some(vec![("octocat".to_string(), "inactive".to_string())])
        );
    }

    #[test]
    fn invalid_cached_history_commit_date_is_warning_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = "invalid-history-commit-date";
        cache
            .set(
                key,
                &CachedCommitHistory {
                    since: "2025-01-01T00:00:00Z".to_string(),
                    until: "2025-02-01T00:00:00Z".to_string(),
                    checked_until: "2025-02-01T00:00:00Z".to_string(),
                    commits: vec![CommitData {
                        oid: Some("bad-date".to_string()),
                        additions: 1,
                        deletions: 0,
                        committed_date: "not-a-timestamp".to_string(),
                    }],
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        let cached = history_cache_get_or_warn(&cache, key, &mut warnings);

        assert!(cached.is_none());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("history cache"));
        assert!(warnings.messages[0].contains(key));
        assert!(warnings.messages[0].contains("committedDate"));
    }

    #[test]
    fn invalid_cached_history_checked_boundary_is_warning_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = "invalid-history-check-boundary";
        cache
            .set(
                key,
                &CachedCommitHistory {
                    since: "2025-01-01T00:00:00Z".to_string(),
                    until: "2025-02-01T00:00:00Z".to_string(),
                    checked_until: "not-a-timestamp".to_string(),
                    commits: vec![],
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        let cached = history_cache_get_or_warn(&cache, key, &mut warnings);

        assert!(cached.is_none());
        assert_eq!(warnings.messages.len(), 1);
        assert!(warnings.messages[0].contains("history cache"));
        assert!(warnings.messages[0].contains(key));
        assert!(warnings.messages[0].contains("check end"));
    }

    #[test]
    fn valid_cached_history_remains_a_cache_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        let key = "valid-history";
        cache
            .set(
                key,
                &CachedCommitHistory {
                    since: "2025-01-01T00:00:00Z".to_string(),
                    until: "2025-02-01T00:00:00Z".to_string(),
                    checked_until: "2025-02-01T00:00:00Z".to_string(),
                    commits: vec![CommitData {
                        oid: Some("valid".to_string()),
                        additions: 1,
                        deletions: 0,
                        committed_date: "2025-01-15T00:00:00Z".to_string(),
                    }],
                },
            )
            .unwrap();
        let mut warnings = CacheWarnings::default();

        let cached = history_cache_get_or_warn(&cache, key, &mut warnings);

        assert_eq!(cached.unwrap().commits.len(), 1);
        assert!(warnings.messages.is_empty());
    }

    #[test]
    fn cache_write_failure_keeps_fresh_result_and_returns_visible_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::with_dir(tmp.path()).unwrap();
        std::fs::create_dir(tmp.path().join("blocked.json")).unwrap();
        let fresh = CachedContributionWindow {
            repos: vec![(sample_repo(), 13)],
            summary: ContributionSummary {
                total_prs: 2,
                total_reviews: 3,
                total_issues: 5,
            },
            saturated: false,
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
        let since = "2025-01-01T00:00:00Z";
        let until = "2025-02-01T00:00:00Z";
        let alice_key = history_cache_key(
            "node-alice",
            "octo",
            "repo",
            Some(since),
            Some(until),
            false,
        );
        let bob_key =
            history_cache_key("node-bob", "octo", "repo", Some(since), Some(until), false);
        let history = |additions| CachedCommitHistory {
            since: since.to_string(),
            until: until.to_string(),
            checked_until: until.to_string(),
            commits: vec![CommitData {
                oid: None,
                additions,
                deletions: 0,
                committed_date: "2025-01-15T00:00:00Z".to_string(),
            }],
        };

        cache.set(&alice_key, &history(11)).unwrap();
        cache.set(&bob_key, &history(22)).unwrap();

        let alice = cache
            .get::<CachedCommitHistory>(&alice_key)
            .unwrap()
            .unwrap();
        let bob = cache.get::<CachedCommitHistory>(&bob_key).unwrap().unwrap();

        assert_eq!(alice.commits[0].additions, 11);
        assert_eq!(bob.commits[0].additions, 22);
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
            body: &'static str,
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
            body: r#"{"data":{"ok":true}}"#,
            delay: Duration::ZERO,
        }
    }

    #[test]
    fn graphql_retries_408_429_and_5xx_then_succeeds_with_a_bound() {
        let server = start_stub(vec![
            StubResponse::Json {
                status: 408,
                body: "{}",
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 429,
                body: "{}",
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 503,
                body: "{}",
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
                    body: "{}",
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
                    body: r#"{"data":{"ok":true}}"#,
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
                body: "{}",
                delay: Duration::ZERO,
            },
            StubResponse::Json {
                status: 200,
                body: r#"[{"author":{"login":"alice"}}]"#,
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
                    body: "{}",
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
        let server = start_stub(vec![StubResponse::Json {
            status: 200,
            body: "[]",
            delay: Duration::ZERO,
        }]);
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
