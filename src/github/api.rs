use std::collections::HashMap;

use anyhow::Context;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};

use super::cache::DiskCache;

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

impl GithubClient {
    pub fn new() -> anyhow::Result<Self> {
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

        let client = Client::builder().default_headers(headers).build()?;

        Ok(Self { client, has_token })
    }

    pub fn has_token(&self) -> bool {
        self.has_token
    }

    fn graphql_query(&self, query: &str, variables: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        if !self.has_token {
            anyhow::bail!("GITHUB_TOKEN is required for this operation");
        }

        let body = serde_json::json!({ "query": query, "variables": variables });

        for attempt in 0..3u32 {
            let resp = self
                .client
                .post("https://api.github.com/graphql")
                .json(&body)
                .send()?;

            match resp.status().as_u16() {
                401 => anyhow::bail!("GitHub GraphQL authentication failed. Check GITHUB_TOKEN."),
                403 => {
                    let wait = Self::parse_rate_limit_wait(&resp);
                    if attempt < 2 {
                        let secs = wait.unwrap_or(60).min(120);
                        eprintln!(
                            "\nRate limited by GitHub API. Waiting {secs}s before retry (attempt {}/{})...",
                            attempt + 1, 3
                        );
                        std::thread::sleep(std::time::Duration::from_secs(secs));
                        continue;
                    }
                    anyhow::bail!(
                        "GitHub API rate limit exceeded after retries. Try again later or check GITHUB_TOKEN scopes."
                    );
                }
                200..=299 => {
                    let payload: serde_json::Value = resp.json()?;
                    let data = parse_graphql_response_payload(&payload)?;

                    // Check remaining budget and warn/pre-emptively wait
                    if let Some(rate_limit) = payload.get("data").and_then(|d| d.get("rateLimit")) {
                        let remaining = rate_limit.get("remaining").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
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
                            eprintln!("\nWarning: GraphQL budget low (remaining={remaining}, last cost={cost}).");
                        }
                    }

                    return Ok(data);
                }
                _ => {
                    resp.error_for_status_ref()?;
                }
            }
        }

        anyhow::bail!("GitHub GraphQL query failed after retries")
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

        for (from, to) in windows {
            let variables = serde_json::json!({
                "login": username,
                "from": from.to_rfc3339(),
                "to": to.to_rfc3339(),
            });
            let data = self.graphql_query(CONTRIBUTIONS_QUERY, &variables)?;
            let (repos, summary) = parse_contributions_collection_data(data, username)?;
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

        Ok((repos, total_summary))
    }

    pub fn batch_commit_history(
        &self,
        user_node_id: &str,
        repos: &[(String, String)],
        since: Option<&str>,
        until: Option<&str>,
    ) -> anyhow::Result<HashMap<String, Vec<CommitData>>> {
        let mut all_commits = HashMap::new();

        for repo_batch in repos.chunks(5) {
            let query = build_batch_history_query(repo_batch);
            let variables = serde_json::json!({
                "userId": user_node_id,
                "since": since,
                "until": until,
            });

            let data = self.graphql_query(&query, &variables)?;
            let parsed = parse_batch_history_data(data, repo_batch)?;

            for (repo_name, parsed_repo) in parsed {
                if parsed_repo.total_count > 100 {
                    eprintln!(
                        "Warning: commit history truncated for {repo_name}: {} commits (showing first 100).",
                        parsed_repo.total_count
                    );
                }
                all_commits.insert(repo_name, parsed_repo.commits);
            }
        }

        Ok(all_commits)
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

            if repos.is_empty() && page_node_count == 0 {
                break;
            }

            all_repos.extend(repos);

            fetched_count += page_node_count;
            if fetched_count >= 300 || !page_info.has_next_page {
                break;
            }

            after = page_info.end_cursor;
            if after.is_none() {
                break;
            }
        }

        Ok(all_repos)
    }

    #[allow(dead_code)]
    pub fn list_contributed_repos_graphql(&self, username: &str) -> anyhow::Result<Vec<RepoWithLangs>> {
        let mut all_repos = Vec::new();
        let mut fetched_count = 0usize;
        let mut after: Option<String> = None;

        loop {
            let variables = serde_json::json!({ "login": username, "after": after });
            let data = self.graphql_query(CONTRIBUTED_REPOS_QUERY, &variables)?;
            let (repos, page_info, page_node_count) = parse_repo_connection_data(
                data,
                username,
                RepoConnectionKind::Contributed,
                true,
            )?;

            if repos.is_empty() && page_node_count == 0 {
                break;
            }

            all_repos.extend(repos);

            fetched_count += page_node_count;
            if fetched_count >= 300 || !page_info.has_next_page {
                break;
            }

            after = page_info.end_cursor;
            if after.is_none() {
                break;
            }
        }

        Ok(all_repos)
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

fn parse_reset_at_wait(reset_at: &str) -> Option<u64> {
    let reset_time = chrono::DateTime::parse_from_rfc3339(reset_at).ok()?;
    let now = Utc::now();
    let wait = (reset_time.timestamp() - now.timestamp()).max(1) as u64;
    Some(wait)
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
    pub additions: u64,
    pub deletions: u64,
    pub committed_date: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlHistoryNode {
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
}

#[derive(Debug, Deserialize)]
struct ParsedBatchHistoryRepo {
    commits: Vec<CommitData>,
    total_count: u64,
}

#[allow(dead_code)]
enum RepoConnectionKind {
    Owned,
    Contributed,
}

fn parse_graphql_response_payload(payload: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
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
    repos: &[(String, String)],
) -> anyhow::Result<HashMap<String, ParsedBatchHistoryRepo>> {
    let obj = data
        .as_object()
        .context("batch history data should be a JSON object")?;

    let mut result = HashMap::new();
    for (idx, (owner, name)) in repos.iter().enumerate() {
        let alias = format!("repo{idx}");
        let mut commits = Vec::new();
        let mut total_count = 0;

        if let Some(repo_value) = obj.get(&alias)
            && !repo_value.is_null()
        {
            let history_value = repo_value
                .pointer("/defaultBranchRef/target/history")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            if !history_value.is_null() {
                let history: GraphqlHistoryConnection = serde_json::from_value(history_value)
                    .with_context(|| format!("failed to parse commit history for {owner}/{name}"))?;
                total_count = history.total_count;
                if let Some(nodes) = history.nodes {
                    commits = nodes
                        .into_iter()
                        .map(|node| CommitData {
                            additions: node.additions,
                            deletions: node.deletions,
                            committed_date: node.committed_date,
                        })
                        .collect();
                }
            }
        }

        result.insert(
            format!("{owner}/{name}"),
            ParsedBatchHistoryRepo {
                commits,
                total_count,
            },
        );
    }

    Ok(result)
}

fn build_batch_history_query(repos: &[(String, String)]) -> String {
    let mut query = String::from(
        "query($userId: ID!, $since: GitTimestamp, $until: GitTimestamp) {\n  rateLimit { cost remaining resetAt }\n",
    );

    for (idx, (owner, name)) in repos.iter().enumerate() {
        let owner_literal = serde_json::to_string(owner).unwrap_or_else(|_| format!("\"{owner}\""));
        let name_literal = serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""));
        query.push_str(&format!(
            "  repo{idx}: repository(owner: {owner_literal}, name: {name_literal}) {{\n    defaultBranchRef {{\n      target {{\n        ... on Commit {{\n          history(author: {{id: $userId}}, since: $since, until: $until, first: 100) {{\n            nodes {{ additions deletions committedDate }}\n            totalCount\n          }}\n        }}\n      }}\n    }}\n  }}\n"
        ));
    }

    query.push('}');
    query
}

fn contribution_windows(since: Option<i64>, now: DateTime<Utc>) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
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
        let window_end = if candidate_end < now { candidate_end } else { now };
        windows.push((window_start, window_end));
        window_start = window_end;
    }

    windows
}

fn repo_key(owner: &str, name: &str) -> String {
    format!("{}/{}", owner.to_lowercase(), name.to_lowercase())
}

fn sanitize_cache_key(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
    commits.dedup_by(|a, b| {
        a.committed_date == b.committed_date
            && a.additions == b.additions
            && a.deletions == b.deletions
    });
    commits
}

#[derive(Serialize, Deserialize)]
struct CachedCommitHistory {
    since: String,
    until: String,
    commits: Vec<CommitData>,
}

fn filter_commits_to_range(commits: &[CommitData], since: Option<&str>, until: Option<&str>) -> Vec<CommitData> {
    commits
        .iter()
        .filter(|c| {
            since.is_none_or(|s| c.committed_date.as_str() >= s)
                && until.is_none_or(|u| c.committed_date.as_str() <= u)
        })
        .cloned()
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn get_contribution_repos_cached(
    client: &GithubClient,
    cache: &DiskCache,
    username: &str,
    since: Option<i64>,
    until: Option<i64>,
    include_forks: bool,
    include_contributed: bool,
    read_cache: bool,
    write_cache: bool,
) -> anyhow::Result<(Vec<(RepoWithLangs, u64)>, ContributionSummary)> {
    let now = effective_window_end(until);
    let today = Utc::now().date_naive();
    let windows = contribution_windows(since, now);
    let mut merged: HashMap<String, (RepoWithLangs, u64)> = HashMap::new();
    let mut accumulated_summary = ContributionSummary::default();

    for (from, to) in windows {
        let key = format!(
            "contrib_{}_{}_{}",
            sanitize_cache_key(username),
            from.format("%Y%m%d"),
            to.format("%Y%m%d")
        );

        let window_completed = to.date_naive() < today;

        let cached: Option<Vec<(RepoWithLangs, u64)>> = if read_cache
            && (window_completed || !write_cache)
        {
            cache.get(&key)
        } else {
            None
        };

        let (repos_chunk, summary_chunk): (Vec<(RepoWithLangs, u64)>, ContributionSummary) = if let Some(cached) = cached {
            (cached, ContributionSummary::default())
        } else {
            let variables = serde_json::json!({
                "login": username,
                "from": from.to_rfc3339(),
                "to": to.to_rfc3339(),
            });
            let data = client.graphql_query(CONTRIBUTIONS_QUERY, &variables)?;
            let parsed = parse_contributions_collection_data(data, username)?;
            if write_cache {
                let _ = cache.set(&key, &parsed.0);
            }
            parsed
        };

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
    since: Option<i64>,
    until: Option<i64>,
    read_cache: bool,
    write_cache: bool,
) -> anyhow::Result<(Vec<RepoContribution>, ContributionSummary)> {
    let cache = if read_cache || write_cache {
        DiskCache::new().ok()
    } else {
        None
    };
    let now = effective_window_end(until);

    let (repo_rows, contribution_summary) = if let Some(cache) = cache.as_ref() {
        get_contribution_repos_cached(
            client,
            cache,
            username,
            since,
            until,
            include_forks,
            include_contributed,
            read_cache,
            write_cache,
        )?
    } else {
        client.get_contribution_repos(
            username,
            since,
            until,
            include_forks,
            include_contributed,
        )?
    };

    eprintln!("Found {} repos with contributions", repo_rows.len());

    let until_iso = Some(now.to_rfc3339());
    let default_since_ts = (now - Duration::days(365)).timestamp();
    let effective_since_ts = since.unwrap_or(default_since_ts);
    let since_iso = Utc
        .timestamp_opt(effective_since_ts, 0)
        .single()
        .map(|dt| dt.to_rfc3339());

    let mut commit_history_by_repo: HashMap<String, Vec<CommitData>> = HashMap::new();
    let mut to_fetch: Vec<(String, String, Option<String>)> = Vec::new();
    let mut gap_repo_keys: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (repo, _) in &repo_rows {
        let history_key = format!(
            "history_{}_{}",
            sanitize_cache_key(&repo.owner),
            sanitize_cache_key(&repo.name),
        );
        let repo_name = format!("{}/{}", repo.owner, repo.name);

        let cached: Option<CachedCommitHistory> =
            if read_cache { cache.as_ref().and_then(|c| c.get(&history_key)) } else { None };

        match &cached {
            Some(ch) if write_cache => {
                let overlap = filter_commits_to_range(
                    &ch.commits,
                    since_iso.as_deref(),
                    until_iso.as_deref(),
                );
                let fetch_since = if ch.until.as_str() < until_iso.as_deref().unwrap_or("") {
                    Some(ch.until.clone())
                } else {
                    None
                };
                commit_history_by_repo.insert(repo_name, overlap);
                if let Some(fs) = fetch_since {
                    gap_repo_keys.insert(repo_key(&repo.owner, &repo.name));
                    to_fetch.push((repo.owner.clone(), repo.name.clone(), Some(fs)));
                }
            }
            Some(ch) => {
                let in_range = filter_commits_to_range(
                    &ch.commits,
                    since_iso.as_deref(),
                    until_iso.as_deref(),
                );
                commit_history_by_repo.insert(repo_name, in_range);
            }
            None if write_cache => {
                to_fetch.push((repo.owner.clone(), repo.name.clone(), since_iso.clone()));
            }
            None => {
                to_fetch.push((repo.owner.clone(), repo.name.clone(), since_iso.clone()));
            }
        }
    }

    if !gap_repo_keys.is_empty() {
        let min_gap_since = to_fetch
            .iter()
            .filter(|(o, n, _)| gap_repo_keys.contains(&repo_key(o, n)))
            .filter_map(|(_, _, s)| s.as_deref())
            .min()
            .unwrap_or("");
        let gap_until = until_iso.as_deref().unwrap_or("");

        let variables = serde_json::json!({
            "login": username,
            "from": min_gap_since,
            "to": gap_until,
        });
        if let Ok(data) = client.graphql_query(CONTRIBUTIONS_QUERY, &variables)
            && let Ok((active, _)) = parse_contributions_collection_data(data, username)
        {
                let active_keys: std::collections::HashSet<String> = active
                    .iter()
                    .map(|(r, _)| repo_key(&r.owner, &r.name))
                    .collect();

                let inactive_gap_repos: Vec<(String, String)> = to_fetch
                    .iter()
                    .filter(|(o, n, _)| {
                        let key = repo_key(o, n);
                        gap_repo_keys.contains(&key) && !active_keys.contains(&key)
                    })
                    .map(|(o, n, _)| (o.clone(), n.clone()))
                    .collect();

                to_fetch.retain(|(o, n, _)| {
                    let key = repo_key(o, n);
                    !gap_repo_keys.contains(&key) || active_keys.contains(&key)
                });

                if write_cache
                    && let Some(c) = &cache
                {
                    for (owner, name) in &inactive_gap_repos {
                        let history_key = format!(
                            "history_{}_{}",
                            sanitize_cache_key(owner),
                            sanitize_cache_key(name),
                        );
                        if let Some(mut ch) = c.get::<CachedCommitHistory>(&history_key) {
                            ch.until = gap_until.to_string();
                            let _ = c.set(&history_key, &ch);
                        }
                    }
                }

                if !inactive_gap_repos.is_empty() {
                    eprintln!(
                        "Skipped {} repos with no new activity in gap period",
                        inactive_gap_repos.len()
                    );
                }
        }
    }

    if !to_fetch.is_empty() {
        let fetch_pairs: Vec<(String, String)> = to_fetch
            .iter()
            .map(|(o, n, _)| (o.clone(), n.clone()))
            .collect();

        let per_repo_since: HashMap<String, Option<&str>> = to_fetch
            .iter()
            .map(|(o, n, s)| (format!("{o}/{n}"), s.as_deref()))
            .collect();

        for batch in fetch_pairs.chunks(5) {
            let batch_since = batch
                .iter()
                .filter_map(|(o, n)| {
                    per_repo_since.get(&format!("{o}/{n}")).copied().flatten()
                })
                .min_by(|a, b| a.cmp(b));

            let fetched = client.batch_commit_history(
                user_node_id,
                batch,
                batch_since,
                until_iso.as_deref(),
            )?;

            for (owner, name) in batch {
                let repo_name = format!("{owner}/{name}");
                let new_commits = fetched.get(&repo_name).cloned().unwrap_or_default();

                let merged = if let Some(mut existing) = commit_history_by_repo.remove(&repo_name) {
                    existing.extend(new_commits);
                    dedup_commits(existing)
                } else {
                    new_commits
                };

                if write_cache
                    && let Some(c) = &cache
                {
                    let history_key = format!(
                        "history_{}_{}",
                        sanitize_cache_key(owner),
                        sanitize_cache_key(name),
                    );
                    let cached_entry = CachedCommitHistory {
                        since: since_iso.clone().unwrap_or_default(),
                        until: until_iso.clone().unwrap_or_default(),
                        commits: merged.clone(),
                    };
                    let _ = c.set(&history_key, &cached_entry);
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
        let total_additions: u64 = weeks.iter().map(|w| w.a).sum();
        let total_deletions: u64 = weeks.iter().map(|w| w.d).sum();

        if repo_total_commits == 0 && total_additions == 0 && total_deletions == 0 {
            continue;
        }

        contributions.push(RepoContribution {
            repo_name,
            total_commits: repo_total_commits,
            total_additions,
            total_deletions,
            weeks,
            languages: repo.languages,
        });
    }

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
            fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
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
                total_net_modifications: 0,
                total_net_additions: 0,
            });

            entry.total_commits += week.c;
            entry.total_additions += week.a;
            entry.total_deletions += week.d;
            entry.total_net_modifications += week.net_modifications;
            entry.total_net_additions += week.net_additions;

            if lang_total_bytes > 0 {
                let mut langs: Vec<(&String, &u64)> = contrib.languages.iter().collect();
                langs.sort_by(|a, b| a.0.cmp(b.0));
                let shares: Vec<f64> = langs.iter().map(|&(_, &b)| b as f64).collect();

                let a_parts = apportion(week.a, &shares);
                let d_parts = apportion(week.d, &shares);
                let nm_parts = apportion(week.net_modifications, &shares);
                let na_parts = apportion(week.net_additions, &shares);

                for (i, (lang, _)) in langs.iter().enumerate() {
                    let lang_entry = entry.by_language.entry((*lang).clone()).or_default();
                    lang_entry.additions += a_parts[i];
                    lang_entry.deletions += d_parts[i];
                    lang_entry.net_modifications += nm_parts[i];
                    lang_entry.net_additions += na_parts[i];
                    lang_entry.files_changed += 1;
                }
            } else if week.a > 0 || week.d > 0 {
                let lang_entry = entry.by_language.entry("Other".to_string()).or_default();
                lang_entry.additions += week.a;
                lang_entry.deletions += week.d;
                lang_entry.net_modifications += week.net_modifications;
                lang_entry.net_additions += week.net_additions;
                lang_entry.files_changed += 1;
            }
        }
    }

    let mut result: Vec<PeriodStats> = buckets.into_values().collect();
    result.sort_by(|a, b| a.period_label.cmp(&b.period_label));
    result
}

#[allow(dead_code)]
pub fn contributions_to_repo_stats(contributions: &[RepoContribution]) -> Vec<crate::stats::models::PeriodStats> {
    use crate::stats::models::PeriodStats;

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

        for week in &contrib.weeks {
            if week.c == 0 && week.a == 0 && week.d == 0 {
                continue;
            }

            entry.total_commits += week.c;
            entry.total_additions += week.a;
            entry.total_deletions += week.d;
            entry.total_net_modifications += week.net_modifications;
            entry.total_net_additions += week.net_additions;

            if lang_total_bytes > 0 {
                let mut langs: Vec<(&String, &u64)> = contrib.languages.iter().collect();
                langs.sort_by(|a, b| a.0.cmp(b.0));
                let shares: Vec<f64> = langs.iter().map(|&(_, &b)| b as f64).collect();

                let a_parts = apportion(week.a, &shares);
                let d_parts = apportion(week.d, &shares);
                let nm_parts = apportion(week.net_modifications, &shares);
                let na_parts = apportion(week.net_additions, &shares);

                for (i, (lang, _)) in langs.iter().enumerate() {
                    let lang_entry = entry.by_language.entry((*lang).clone()).or_default();
                    lang_entry.additions += a_parts[i];
                    lang_entry.deletions += d_parts[i];
                    lang_entry.net_modifications += nm_parts[i];
                    lang_entry.net_additions += na_parts[i];
                    lang_entry.files_changed += 1;
                }
            } else if week.a > 0 || week.d > 0 {
                let lang_entry = entry.by_language.entry("Other".to_string()).or_default();
                lang_entry.additions += week.a;
                lang_entry.deletions += week.d;
                lang_entry.net_modifications += week.net_modifications;
                lang_entry.net_additions += week.net_additions;
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
            parse_repo_connection_data(data, "octocat", RepoConnectionKind::Owned, false)
                .unwrap();

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
        let (repos, page_info, node_count) = parse_repo_connection_data(
            data,
            "octocat",
            RepoConnectionKind::Contributed,
            true,
        )
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
        let repos = vec![
            ("octocat".to_string(), "repo-a".to_string()),
            ("other".to_string(), "repo-b".to_string()),
        ];
        let data = json!({
            "repo0": {
                "defaultBranchRef": {
                    "target": {
                        "history": {
                            "nodes": [
                                {
                                    "additions": 10,
                                    "deletions": 3,
                                    "committedDate": "2025-01-06T12:00:00Z"
                                },
                                {
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
                            "nodes": [
                                {
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

        let parsed = parse_batch_history_data(data, &repos).unwrap();
        let repo0 = parsed.get("octocat/repo-a").unwrap();
        assert_eq!(repo0.total_count, 2);
        assert_eq!(repo0.commits.len(), 2);
        assert_eq!(repo0.commits[0].additions, 10);

        let repo1 = parsed.get("other/repo-b").unwrap();
        assert_eq!(repo1.total_count, 150);
        assert_eq!(repo1.commits.len(), 1);
        assert_eq!(repo1.commits[0].deletions, 50);
    }

    #[test]
    fn commits_bucket_into_monday_weeks() {
        let commits = vec![
            CommitData {
                additions: 10,
                deletions: 2,
                committed_date: "2025-01-06T10:00:00Z".to_string(),
            }, // Monday
            CommitData {
                additions: 5,
                deletions: 1,
                committed_date: "2025-01-12T22:00:00Z".to_string(),
            }, // Sunday same week
            CommitData {
                additions: 7,
                deletions: 3,
                committed_date: "2025-01-13T09:00:00Z".to_string(),
            }, // Next Monday
        ];

        let buckets = commits_to_weekly_buckets(&commits);
        assert_eq!(buckets.len(), 2);

        let first_week = Utc.with_ymd_and_hms(2025, 1, 6, 0, 0, 0).unwrap().timestamp();
        let second_week = Utc.with_ymd_and_hms(2025, 1, 13, 0, 0, 0).unwrap().timestamp();

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

        let err = parse_graphql_response_payload(&payload).unwrap_err().to_string();
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
                weeks: vec![
                    ContributorWeek {
                        w: Utc.with_ymd_and_hms(2025, 1, 6, 0, 0, 0).unwrap().timestamp(),
                        a: 20,
                        d: 6,
                        c: 2,
                        net_modifications: 20,
                        net_additions: 14,
                    },
                    ContributorWeek {
                        w: Utc.with_ymd_and_hms(2025, 1, 13, 0, 0, 0).unwrap().timestamp(),
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
                weeks: vec![ContributorWeek {
                    w: Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap().timestamp(),
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
}
