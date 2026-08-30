//! logit — lines of git
//!
//! CLI tool for analyzing git repository history with per-language,
//! per-author, and per-time-period statistics.

mod analyze;
mod cli;
mod error;
mod exclude;
mod filter;
mod git;
mod lang;
mod output;
mod scanner;
mod stats;

#[cfg(feature = "github")]
mod github;

use std::collections::HashMap;
#[cfg(feature = "github")]
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::Parser;

use cli::{Cli, Commands, GroupBy, OutputFormat, Period, ScanArgs, ScanFormat, StatsArgs};
use stats::models::CommitStats;

#[derive(Debug, Clone, Copy)]
struct TimeRange {
    since: Option<DateTime<Utc>>,
    until_exclusive: Option<DateTime<Utc>>,
}

#[cfg(feature = "github")]
fn collect_requested_github_logins(
    me: Option<&filter::MeExpr>,
    rules: &[exclude::ExcludeRule],
) -> BTreeSet<String> {
    let mut logins = me.map(filter::MeExpr::github_logins).unwrap_or_default();
    logins.extend(
        exclude::collect_github_users(rules)
            .into_iter()
            .map(|login| login.trim().to_ascii_lowercase())
            .filter(|login| !login.is_empty()),
    );
    logins
}

#[cfg(feature = "github")]
const MAX_COMMAND_GITHUB_LOGINS: usize = 8;

#[cfg(feature = "github")]
#[derive(Debug, Default)]
struct CommandIdentityResolution {
    reports: BTreeMap<String, github::api::IdentityResolutionReport>,
    skipped_logins: usize,
}

#[cfg(feature = "github")]
impl CommandIdentityResolution {
    fn email_to_login_map(&self) -> HashMap<String, String> {
        let mut mappings = HashMap::new();
        for (login, report) in &self.reports {
            let login = login.to_ascii_lowercase();
            for email in &report.emails {
                let email = git::author::canonical_email_key(email);
                if !email.is_empty() {
                    mappings.entry(email).or_insert_with(|| login.clone());
                }
            }
        }
        mappings
    }

    fn warning(&self) -> Option<String> {
        let partial = self
            .reports
            .values()
            .filter(|report| report.is_partial())
            .collect::<Vec<_>>();
        if partial.is_empty() && self.skipped_logins == 0 {
            return None;
        }

        let mut parts = Vec::new();
        if let Some(message) = partial
            .iter()
            .flat_map(|report| report.failures.iter())
            .map(|failure| failure.message.as_str())
            .find(|message| message.contains("GITHUB_TOKEN"))
        {
            parts.push(format!(
                "GitHub identity resolution could not run: {message}; local noreply matching remains enabled"
            ));
        } else if !partial.is_empty() {
            let logins = partial
                .iter()
                .map(|report| report.login.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!(
                "GitHub identity resolution is partial for {logins}; known emails were applied, but results may miss others"
            ));
        }
        if self.skipped_logins > 0 {
            parts.push(format!(
                "skipped {} login(s) after the command limit of {MAX_COMMAND_GITHUB_LOGINS}",
                self.skipped_logins
            ));
        }
        Some(parts.join("; "))
    }
}

#[cfg(feature = "github")]
fn resolve_identity_reports<F>(
    requested_logins: &BTreeSet<String>,
    mut resolve: F,
) -> CommandIdentityResolution
where
    F: FnMut(&str) -> github::api::IdentityResolutionReport,
{
    let logins = requested_logins
        .iter()
        .map(|login| login.trim().to_ascii_lowercase())
        .filter(|login| !login.is_empty())
        .collect::<BTreeSet<_>>();
    let mut resolution = CommandIdentityResolution {
        skipped_logins: logins.len().saturating_sub(MAX_COMMAND_GITHUB_LOGINS),
        ..Default::default()
    };

    for login in logins.into_iter().take(MAX_COMMAND_GITHUB_LOGINS) {
        let mut report = resolve(&login);
        report.login = login.clone();
        resolution.reports.insert(login, report);
    }
    resolution
}

#[cfg(feature = "github")]
fn unavailable_identity_report(
    login: &str,
    message: String,
) -> github::api::IdentityResolutionReport {
    github::api::IdentityResolutionReport {
        login: login.to_string(),
        emails: BTreeSet::new(),
        repositories_examined: 0,
        logical_requests: 0,
        truncated_repositories: false,
        truncated_commits: false,
        failures: vec![github::api::IdentityLookupFailure {
            repository: None,
            message,
        }],
    }
}

#[cfg(feature = "github")]
fn resolve_command_identity_reports(
    requested_logins: &BTreeSet<String>,
) -> CommandIdentityResolution {
    if requested_logins.is_empty() {
        return CommandIdentityResolution::default();
    }

    match github::GithubClient::new() {
        Ok(client) if client.has_token() => resolve_identity_reports(requested_logins, |login| {
            client.resolve_user_identity(login)
        }),
        Ok(_) => resolve_identity_reports(requested_logins, |login| {
            unavailable_identity_report(
                login,
                "GITHUB_TOKEN is not configured for GitHub identity resolution".to_string(),
            )
        }),
        Err(error) => resolve_identity_reports(requested_logins, |login| {
            unavailable_identity_report(
                login,
                format!("failed to initialize the GitHub identity client: {error}"),
            )
        }),
    }
}

#[cfg(feature = "github")]
#[derive(Debug, Default)]
struct RemoteIdentityReport {
    mappings: HashMap<String, String>,
    warnings: Vec<String>,
}

fn write_output(content: String, path: Option<&std::path::Path>) -> anyhow::Result<()> {
    if let Some(path) = path {
        std::fs::write(path, &content)?;
        eprintln!("Output written to: {}", path.display());
    } else {
        println!("{content}");
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Scan(args) => cmd_scan(args),
        Commands::Stats(args) => cmd_stats(args),
        #[cfg(feature = "github")]
        Commands::Github(sub) => match sub {
            cli::GithubSubcommand::Fetch(args) => cmd_github_fetch(args),
            cli::GithubSubcommand::Card(args) => cmd_github_card(args),
            cli::GithubSubcommand::Multi(args) => cmd_github_multi(args),
        },
    }
}

fn cmd_scan(args: ScanArgs) -> anyhow::Result<()> {
    let report = scanner::scan_for_repos(&args.path)?;
    for warning in &report.warnings {
        eprintln!("Warning: {warning}");
    }
    let content = match args.format {
        ScanFormat::Table => output::table::render_scan_table(&report.repos),
        ScanFormat::Json => output::json::render_scan_json(&report.repos)?,
    };
    write_output(content, args.output.as_deref())
}

fn cmd_stats(args: StatsArgs) -> anyhow::Result<()> {
    let time_range = resolve_time_range(
        args.days,
        args.since.as_deref(),
        args.until.as_deref(),
        Utc::now(),
    )?;
    let since = time_range.since;
    let until = time_range.until_exclusive;
    let mut exclude_rules: Vec<exclude::ExcludeRule> = args
        .exclude
        .iter()
        .map(|v| exclude::ExcludeRule::parse_many(v))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let me_expr = args.me.as_deref().map(filter::parse_me_expr).transpose()?;
    #[cfg(feature = "github")]
    let requested_github_logins = collect_requested_github_logins(me_expr.as_ref(), &exclude_rules);

    let mut repos: Vec<PathBuf> = Vec::new();
    let mut scan_warnings = Vec::new();
    for path in &args.paths {
        if path.join(".git").exists() {
            repos.push(path.clone());
        } else {
            let report = scanner::scan_for_repos(path)?;
            repos.extend(report.repos);
            scan_warnings.extend(report.warnings);
        }
    }

    for warning in scan_warnings {
        eprintln!("Warning: {warning}");
    }

    let repos = analyze::normalize_repo_inputs(repos, args.repo.as_deref())?;
    let (commits, errors) = analyze::analyze_repos(&repos, since, until);

    for e in &errors {
        eprintln!(
            "Warning: analysis failed for {}: {}",
            e.path.display(),
            e.error
        );
    }

    if !repos.is_empty() && errors.len() == repos.len() {
        anyhow::bail!("failed to analyze all {} repositories", repos.len());
    }

    if commits.is_empty() {
        eprintln!("No commits found in the given period.");
        return Ok(());
    }

    let commits =
        filter_commits_for_stats(commits, args.committer.as_deref(), args.lang.as_deref());
    if commits.is_empty() {
        eprintln!("commits exist in the given period, but none matched the requested filters.");
        return Ok(());
    }

    let active_repos: std::collections::HashSet<&str> =
        commits.iter().map(|c| c.repo_id.as_str()).collect();
    let skipped = repos.len() - active_repos.len();
    if skipped > 0 {
        if args.committer.is_some() || args.lang.is_some() {
            eprintln!("Skipped {skipped} repo(s) with no commits matching the requested filters.");
        } else {
            eprintln!("Skipped {skipped} repo(s) with no activity in the period.");
        }
    }

    #[cfg(feature = "github")]
    let command_identity_resolution = resolve_command_identity_reports(&requested_github_logins);
    #[cfg(feature = "github")]
    if let Some(warning) = command_identity_resolution.warning() {
        eprintln!("Warning: {warning}");
    }
    #[cfg(feature = "github")]
    for (login, report) in &command_identity_resolution.reports {
        let emails = report.emails.iter().cloned().collect::<Vec<_>>();
        if !emails.is_empty() {
            for rule in &mut exclude_rules {
                rule.resolve_github_user(login, &emails);
            }
        }
    }

    let mut identity_map = build_identity_map(&args.dedup, &repos, &commits);
    #[cfg(feature = "github")]
    identity_map.extend(command_identity_resolution.email_to_login_map());

    let commits = if let Some(ref expr) = me_expr {
        commits
            .into_iter()
            .filter(|c| expr.matches_commit(c, &identity_map))
            .collect()
    } else {
        commits
    };

    let commits = exclude::filter_commits(commits, &exclude_rules);

    let period = args.period.unwrap_or(Period::Month);
    let author_filter = args.author.as_deref();
    let lang_filter = args.lang.as_deref();
    let num_fmt = if args.short {
        cli::NumberFormat::Short
    } else {
        args.number_format
    };
    let compact = !args.no_compact;
    let columns = cli::resolve_columns(&args.columns, &args.exclude_columns);

    let counts =
        stats::aggregator::local_group_cardinality(&commits, &period, author_filter, lang_filter);
    let plan = stats::aggregator::resolve_group_plan(
        &args.group,
        &args.groups,
        &counts,
        stats::aggregator::GroupSource::Local,
    )
    .map_err(anyhow::Error::msg)?;

    if plan.hierarchical {
        let mut period_stats =
            stats::aggregator::aggregate_commits(&commits, &period, author_filter, lang_filter);
        let mut totals = stats::aggregator::aggregate_totals(&period_stats);
        let mut nodes = stats::aggregator::build_group_tree(
            &commits,
            &plan.levels,
            &period,
            author_filter,
            lang_filter,
        );

        if !args.exclude_lang.is_empty() {
            stats::aggregator::filter_excluded_languages_tree(&mut nodes, &args.exclude_lang);
            stats::aggregator::filter_excluded_languages(
                &mut period_stats,
                &mut totals,
                &args.exclude_lang,
            );
        }

        match args.format {
            OutputFormat::Table => {
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Tree {
                        nodes: &nodes,
                        levels: &plan.levels,
                        totals: &totals,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &args.show_email,
                        dedup: &args.dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                let content = output::table::render_presentation_table(&model, num_fmt, compact);
                write_output(content, args.output.as_deref())?;
            }
            OutputFormat::Json => {
                let content = output::json::render_group_tree_json(&nodes)?;
                write_output(content, args.output.as_deref())?;
            }
            #[cfg(feature = "tui")]
            OutputFormat::Tui => {
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Tree {
                        nodes: &nodes,
                        levels: &plan.levels,
                        totals: &totals,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &args.show_email,
                        dedup: &args.dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                output::tui::run_tui(&model, num_fmt, compact)?;
            }
        }
    } else {
        let group = plan.primary;

        let mut period_stats = if matches!(group, GroupBy::Repo) {
            stats::aggregator::aggregate_by_repo(&commits, author_filter, lang_filter)
        } else {
            stats::aggregator::aggregate_commits(&commits, &period, author_filter, lang_filter)
        };
        let mut totals = stats::aggregator::aggregate_totals(&period_stats);

        if !args.exclude_lang.is_empty() {
            stats::aggregator::filter_excluded_languages(
                &mut period_stats,
                &mut totals,
                &args.exclude_lang,
            );
        }

        match args.format {
            OutputFormat::Table => {
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Flat {
                        stats: &period_stats,
                        totals: &totals,
                        primary: group,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &args.show_email,
                        dedup: &args.dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                let content = output::table::render_presentation_table(&model, num_fmt, compact);
                write_output(content, args.output.as_deref())?;
            }
            OutputFormat::Json => {
                let content = output::json::render_stats_json(&period_stats, &totals)?;
                write_output(content, args.output.as_deref())?;
            }
            #[cfg(feature = "tui")]
            OutputFormat::Tui => {
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Flat {
                        stats: &period_stats,
                        totals: &totals,
                        primary: group,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &args.show_email,
                        dedup: &args.dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                output::tui::run_tui(&model, num_fmt, compact)?;
            }
        }
    }

    Ok(())
}

fn filter_commits_for_stats(
    commits: Vec<CommitStats>,
    committer: Option<&str>,
    language: Option<&str>,
) -> Vec<CommitStats> {
    commits
        .into_iter()
        .filter(|commit| {
            committer.is_none_or(|pattern| commit.committer.matches(pattern))
                && language.is_none_or(|target| {
                    commit.file_changes.iter().any(|file_change| {
                        file_change
                            .language
                            .as_deref()
                            .is_some_and(|lang| lang.eq_ignore_ascii_case(target))
                    })
                })
        })
        .collect()
}

fn parse_date(s: &str, flag: &str) -> anyhow::Result<DateTime<Utc>> {
    let naive = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| {
        anyhow::anyhow!("Invalid {flag} date '{s}': {e}. Expected format: YYYY-MM-DD")
    })?;
    let midnight = naive
        .and_hms_opt(0, 0, 0)
        .expect("midnight (00:00:00) is always a valid time");
    Ok(midnight.and_utc())
}

fn duration_for_days(days: f64, flag: &str) -> anyhow::Result<chrono::Duration> {
    if !days.is_finite() || days <= 0.0 {
        anyhow::bail!("{flag} must be a finite number greater than zero");
    }

    let seconds = (days * 86_400.0).ceil();
    if !seconds.is_finite() || seconds >= i64::MAX as f64 {
        anyhow::bail!("{flag} duration is too large");
    }

    let duration = chrono::Duration::try_seconds(seconds as i64)
        .ok_or_else(|| anyhow::anyhow!("{flag} duration is too large"))?;
    if duration.num_nanoseconds().is_none() {
        anyhow::bail!("{flag} duration is too large");
    }
    Ok(duration)
}

fn resolve_time_range(
    days: Option<f64>,
    since: Option<&str>,
    until: Option<&str>,
    now: DateTime<Utc>,
) -> anyhow::Result<TimeRange> {
    let since = if let Some(days) = days {
        let duration = duration_for_days(days, "--days")?;
        Some(
            now.checked_sub_signed(duration)
                .ok_or_else(|| anyhow::anyhow!("--days duration is too large"))?,
        )
    } else {
        since
            .map(|value| parse_date(value, "--since"))
            .transpose()?
    };
    let until_exclusive = until
        .map(|value| -> anyhow::Result<DateTime<Utc>> {
            let date = parse_date(value, "--until")?.date_naive();
            let next_date = date
                .succ_opt()
                .ok_or_else(|| anyhow::anyhow!("--until date '{value}' is out of range"))?;
            let midnight = next_date
                .and_hms_opt(0, 0, 0)
                .expect("midnight (00:00:00) is always a valid time");
            Ok(midnight.and_utc())
        })
        .transpose()?;

    if since
        .zip(until_exclusive)
        .is_some_and(|(since, until)| since >= until)
    {
        anyhow::bail!("--since must not be after --until");
    }

    Ok(TimeRange {
        since,
        until_exclusive,
    })
}

#[cfg(feature = "github")]
fn resolve_github_query_window(
    days: Option<f64>,
    since: Option<&str>,
    until: Option<&str>,
    observed_at: DateTime<Utc>,
) -> anyhow::Result<github::api::QueryWindow> {
    use github::api::CacheWindowScope;

    let explicit_since = if days.is_some() {
        None
    } else {
        since
            .map(|value| parse_date(value, "--since"))
            .transpose()?
    };
    if explicit_since.is_some_and(|from| from > observed_at) {
        anyhow::bail!("--since must not be in the future");
    }

    let explicit_until = until
        .map(|value| -> anyhow::Result<DateTime<Utc>> {
            let date = parse_date(value, "--until")?.date_naive();
            let next_date = date
                .succ_opt()
                .ok_or_else(|| anyhow::anyhow!("--until date '{value}' is out of range"))?;
            let midnight = next_date
                .and_hms_opt(0, 0, 0)
                .expect("midnight (00:00:00) is always a valid time");
            Ok(midnight.and_utc())
        })
        .transpose()?;
    let elapsed_until = explicit_until.filter(|end| *end <= observed_at);
    let duration = if explicit_since.is_none() {
        Some(duration_for_days(days.unwrap_or(365.0), "--days")?)
    } else {
        None
    };
    let duration_start = elapsed_until.unwrap_or(observed_at);
    let requested_from = match (explicit_since, duration) {
        (Some(from), _) => from,
        (None, Some(duration)) => duration_start
            .checked_sub_signed(duration)
            .ok_or_else(|| anyhow::anyhow!("--days duration is too large"))?,
        (None, None) => unreachable!("a GitHub query must have a start boundary"),
    };
    let until_exclusive = elapsed_until.unwrap_or(observed_at);

    if requested_from >= until_exclusive {
        anyhow::bail!("--since must not be after --until");
    }

    let scope = match (elapsed_until, explicit_since, duration) {
        (Some(until_exclusive), _, _) => CacheWindowScope::Fixed {
            from: requested_from,
            until_exclusive,
        },
        (None, Some(from), _) => CacheWindowScope::Anchored { from },
        (None, None, Some(duration)) => CacheWindowScope::Rolling {
            lookback_nanoseconds: duration
                .num_nanoseconds()
                .expect("duration_for_days rejects cache scope overflow"),
        },
        (None, None, None) => unreachable!("a rolling GitHub query needs a duration"),
    };

    Ok(github::api::QueryWindow {
        scope,
        requested_from,
        until_exclusive,
        observed_at,
        completed: elapsed_until.is_some(),
    })
}

fn build_identity_map(
    _dedup: &cli::DedupMode,
    _repos: &[analyze::RepoInput],
    _commits: &[stats::models::CommitStats],
) -> HashMap<String, String> {
    #[cfg(feature = "github")]
    {
        if matches!(_dedup, cli::DedupMode::Remote) {
            return build_remote_identity_map(_repos, _commits);
        }
    }
    HashMap::new()
}

#[cfg(feature = "github")]
fn build_remote_identity_map(
    repos: &[analyze::RepoInput],
    commits: &[stats::models::CommitStats],
) -> HashMap<String, String> {
    let has_github_origin = repos.iter().any(|repo| {
        git::repo::get_remote_origin(&repo.path)
            .and_then(|url| git::repo::parse_remote_url(&url))
            .is_some_and(|info| matches!(info.platform, git::repo::Platform::GitHub))
    });
    if !has_github_origin {
        return HashMap::new();
    }

    let client = match github::GithubClient::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: failed to create GitHub client for remote dedup: {e}");
            return HashMap::new();
        }
    };

    let report = build_remote_identity_map_with(
        repos,
        commits,
        |repo| {
            let url = git::repo::get_remote_origin(&repo.path)?;
            let info = git::repo::parse_remote_url(&url)?;
            matches!(info.platform, git::repo::Platform::GitHub).then_some((info.owner, info.repo))
        },
        |owner, repo, email| {
            client
                .resolve_single_email_result(owner, repo, email)
                .map_err(|error| error.to_string())
        },
    );
    for warning in &report.warnings {
        eprintln!("Warning: {warning}");
    }
    report.mappings
}

#[cfg(feature = "github")]
fn build_remote_identity_map_with<O, R>(
    repos: &[analyze::RepoInput],
    commits: &[stats::models::CommitStats],
    mut remote_for_repo: O,
    mut resolve_email: R,
) -> RemoteIdentityReport
where
    O: FnMut(&analyze::RepoInput) -> Option<(String, String)>,
    R: FnMut(&str, &str, &str) -> Result<Option<String>, String>,
{
    let mut report = RemoteIdentityReport::default();
    let mut first_success_origin = HashMap::new();
    let mut ordered_repos = repos.iter().collect::<Vec<_>>();
    ordered_repos.sort_by(|left, right| {
        analyze::platform_repo_key(&left.id, cfg!(windows))
            .cmp(&analyze::platform_repo_key(&right.id, cfg!(windows)))
            .then_with(|| left.id.cmp(&right.id))
    });

    for selected_repo in ordered_repos {
        let Some((owner, repo)) = remote_for_repo(selected_repo) else {
            continue;
        };
        let remote_name = format!("{owner}/{repo}");
        let mut emails = BTreeSet::new();
        for commit in commits
            .iter()
            .filter(|commit| commit.repo_id == selected_repo.id)
        {
            emails.insert(git::author::canonical_email_key(&commit.author.email));
            emails.extend(
                commit
                    .co_authors
                    .iter()
                    .map(|author| git::author::canonical_email_key(&author.email)),
            );
        }

        for email in emails.into_iter().filter(|email| !email.is_empty()) {
            match resolve_email(&owner, &repo, &email) {
                Ok(Some(login)) if !login.trim().is_empty() => {
                    let login = login.trim().to_ascii_lowercase();
                    if let Some(existing) = report.mappings.get(&email) {
                        if existing != &login {
                            let first_origin = first_success_origin
                                .get(&email)
                                .expect("mapped identity records its first successful origin");
                            report.warnings.push(format!(
                                "conflicting GitHub identity for '{email}': keeping '{existing}' from {first_origin}, ignoring '{login}' from {remote_name}"
                            ));
                        }
                    } else {
                        report.mappings.insert(email.clone(), login);
                        first_success_origin.insert(email, remote_name.clone());
                    }
                }
                Ok(None) => {}
                Err(error) => report.warnings.push(format!(
                    "failed to resolve GitHub identity for '{email}' in {remote_name}: {error}"
                )),
                Ok(Some(_)) => {}
            }
        }
    }

    report
}

#[cfg(feature = "github")]
fn cmd_github_fetch(args: cli::GithubFetchArgs) -> anyhow::Result<()> {
    use cli::FetchFormat;

    stats::aggregator::validate_group_source(
        &args.group,
        &args.groups,
        stats::aggregator::GroupSource::Github,
    )
    .map_err(anyhow::Error::msg)?;

    let exclude_rules: Vec<exclude::ExcludeRule> = args
        .exclude
        .iter()
        .map(|v| exclude::ExcludeRule::parse_many(v))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let (user, mut contributions, contribution_summary, days_value) = fetch_github_data(
        &args.data.username,
        args.data.days,
        args.data.since.as_deref(),
        args.data.until.as_deref(),
        args.data.include_forks,
        args.data.include_contributed,
        args.data.include_private,
        args.data.no_cache,
        args.data.refresh_cache,
    )?;

    if !exclude_rules.is_empty() {
        if exclude::any_path_rules(&exclude_rules) {
            eprintln!(
                "Warning: --exclude path rules are ignored in GitHub mode (no per-file data from API)"
            );
        }
        let before = contributions.len();
        contributions.retain(|c| {
            !exclude::is_repo_excluded_with_mode(
                &c.repo_name,
                &exclude_rules,
                exclude::RepoMatchMode::Github,
            )
        });
        if contributions.len() < before {
            eprintln!(
                "Excluded {} repo(s) via --exclude",
                before - contributions.len()
            );
        }
        for c in &mut contributions {
            let langs = exclude::excluded_langs_for_repo_with_mode(
                &c.repo_name,
                &exclude_rules,
                exclude::RepoMatchMode::Github,
            );
            for lang in langs {
                c.languages.retain(|l, _| !l.eq_ignore_ascii_case(&lang));
            }
        }
    }

    let period = args.period.unwrap_or(Period::Month);
    let num_fmt = if args.short {
        cli::NumberFormat::Short
    } else {
        args.number_format
    };
    let compact = !args.no_compact;
    let columns = cli::resolve_columns(&args.columns, &args.exclude_columns);

    let counts = github::api::contribution_group_cardinality(&contributions, &period);
    let plan = stats::aggregator::resolve_group_plan(
        &args.group,
        &args.groups,
        &counts,
        stats::aggregator::GroupSource::Github,
    )
    .map_err(anyhow::Error::msg)?;
    let metadata = serde_json::json!({
        "username": args.data.username,
        "days": days_value,
        "active_repos": contributions.len(),
        "generated_at": chrono::Utc::now().to_rfc3339(),
    });

    if plan.hierarchical {
        let mut period_stats = github::api::contributions_to_period_stats(&contributions, &period);
        let mut totals = stats::aggregator::aggregate_totals(&period_stats);
        let mut nodes =
            github::api::contributions_to_group_tree(&contributions, &plan.levels, &period);
        if !args.exclude_lang.is_empty() {
            stats::aggregator::filter_excluded_languages_tree(&mut nodes, &args.exclude_lang);
            stats::aggregator::filter_excluded_languages(
                &mut period_stats,
                &mut totals,
                &args.exclude_lang,
            );
        }

        match args.format {
            FetchFormat::Json => {
                let content = output::json::render_github_group_tree_json(
                    metadata,
                    &user,
                    &contribution_summary,
                    &nodes,
                )?;
                write_output(content, args.output.as_deref())?;
            }
            FetchFormat::Table => {
                let email = cli::EmailDisplay::None;
                let dedup = cli::DedupMode::None;
                let identity_map = HashMap::new();
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Tree {
                        nodes: &nodes,
                        levels: &plan.levels,
                        totals: &totals,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &email,
                        dedup: &dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                let content = output::table::render_presentation_table(&model, num_fmt, compact);
                write_output(content, args.output.as_deref())?;
            }
            #[cfg(feature = "tui")]
            FetchFormat::Tui => {
                let email = cli::EmailDisplay::None;
                let dedup = cli::DedupMode::None;
                let identity_map = HashMap::new();
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Tree {
                        nodes: &nodes,
                        levels: &plan.levels,
                        totals: &totals,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &email,
                        dedup: &dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                output::tui::run_tui(&model, num_fmt, compact)?;
            }
        }
    } else {
        let group = plan.primary;
        let mut period_stats = if matches!(group, cli::GroupBy::Repo) {
            github::api::contributions_to_repo_stats(&contributions)
        } else {
            github::api::contributions_to_period_stats(&contributions, &period)
        };
        let mut totals = stats::aggregator::aggregate_totals(&period_stats);

        if !args.exclude_lang.is_empty() {
            stats::aggregator::filter_excluded_languages(
                &mut period_stats,
                &mut totals,
                &args.exclude_lang,
            );
        }

        match args.format {
            FetchFormat::Json => {
                let content = output::json::render_github_stats_json(
                    metadata,
                    &user,
                    &period_stats,
                    &totals,
                    &contribution_summary,
                )?;
                write_output(content, args.output.as_deref())?;
            }
            FetchFormat::Table => {
                let dedup = cli::DedupMode::None;
                let email = cli::EmailDisplay::None;
                let identity_map = HashMap::new();
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Flat {
                        stats: &period_stats,
                        totals: &totals,
                        primary: group,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &email,
                        dedup: &dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                let content = output::table::render_presentation_table(&model, num_fmt, compact);
                write_output(content, args.output.as_deref())?;
            }
            #[cfg(feature = "tui")]
            FetchFormat::Tui => {
                let email = cli::EmailDisplay::None;
                let dedup = cli::DedupMode::None;
                let identity_map = HashMap::new();
                let model = output::presentation::build_presentation(
                    output::presentation::PresentationData::Flat {
                        stats: &period_stats,
                        totals: &totals,
                        primary: group,
                    },
                    output::presentation::PresentationOptions {
                        columns: &columns,
                        sort: args.sort.as_ref(),
                        email_display: &email,
                        dedup: &dedup,
                        identity_map: &identity_map,
                        inline_tree: args.inline_tree,
                    },
                );
                output::tui::run_tui(&model, num_fmt, compact)?;
            }
        }
    }

    Ok(())
}

#[cfg(feature = "github")]
fn cmd_github_card(args: cli::GithubCardArgs) -> anyhow::Result<()> {
    if args.username.is_none() && args.input.is_none() {
        anyhow::bail!("Either provide a username or use --input to load from JSON file");
    }
    if args.username.is_some() && args.input.is_some() {
        anyhow::bail!("Use either a username or --input, not both");
    }

    let (user, mut totals, summary, active_repos, days_value, username) = if let Some(
        ref input_path,
    ) = args.input
    {
        load_card_data_from_json(input_path)?
    } else {
        let username = args
            .username
            .as_deref()
            .expect("username is required when --input is not provided");
        let exclude_rules: Vec<exclude::ExcludeRule> = args
            .exclude
            .iter()
            .map(|v| exclude::ExcludeRule::parse_many(v))
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();

        let (user, mut contributions, contribution_summary, days_value) = fetch_github_data(
            username,
            args.days,
            args.since.as_deref(),
            args.until.as_deref(),
            args.include_forks,
            args.include_contributed,
            args.include_private,
            args.no_cache,
            args.refresh_cache,
        )?;

        if !exclude_rules.is_empty() {
            if exclude::any_path_rules(&exclude_rules) {
                eprintln!(
                    "Warning: --exclude path rules are ignored in GitHub mode (no per-file data from API)"
                );
            }
            contributions.retain(|c| {
                !exclude::is_repo_excluded_with_mode(
                    &c.repo_name,
                    &exclude_rules,
                    exclude::RepoMatchMode::Github,
                )
            });
            for c in &mut contributions {
                let langs = exclude::excluded_langs_for_repo_with_mode(
                    &c.repo_name,
                    &exclude_rules,
                    exclude::RepoMatchMode::Github,
                );
                for lang in langs {
                    c.languages.retain(|l, _| !l.eq_ignore_ascii_case(&lang));
                }
            }
        }

        let active_repos = contributions.len();
        let period_stats =
            github::api::contributions_to_period_stats(&contributions, &Period::Month);
        let totals = stats::aggregator::aggregate_totals(&period_stats);

        (
            user,
            totals,
            contribution_summary,
            active_repos,
            days_value,
            username.to_string(),
        )
    };

    if !args.exclude_lang.is_empty() {
        stats::aggregator::remove_excluded_from_period(&mut totals, &args.exclude_lang);
    }

    let svg = github::render_profile_card(
        &username,
        &user,
        Some(&totals),
        active_repos,
        &summary,
        days_value,
        args.short,
        args.number_format,
        args.number_format_lines,
        args.lang_rows,
        args.title.as_deref(),
    )?;
    write_output(svg, args.output.as_deref())
}

#[cfg(feature = "github")]
fn load_card_data_from_json(
    path: &std::path::Path,
) -> anyhow::Result<(
    github::api::GithubUser,
    stats::models::PeriodStats,
    github::ContributionSummary,
    usize,
    u64,
    String,
)> {
    let content = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    let user: github::api::GithubUser = serde_json::from_value(
        json.get("user")
            .ok_or_else(|| anyhow::anyhow!("Missing 'user' in JSON"))?
            .clone(),
    )?;

    let metadata = json.get("metadata");
    let days = metadata
        .and_then(|m| m.get("days"))
        .and_then(|d| d.as_u64())
        .unwrap_or(365);
    let active_repos = metadata
        .and_then(|m| m.get("active_repos"))
        .and_then(|a| a.as_u64())
        .unwrap_or(0) as usize;
    let username = metadata
        .and_then(|m| m.get("username"))
        .and_then(|u| u.as_str())
        .unwrap_or(&user.login)
        .to_string();

    let summary: github::ContributionSummary = json
        .get("summary")
        .map(|summary| serde_json::from_value(summary.clone()))
        .transpose()?
        .unwrap_or_default();

    let totals_json = json
        .get("totals")
        .ok_or_else(|| anyhow::anyhow!("Missing 'totals' in JSON"))?;
    let totals = stats::models::PeriodStats {
        period_label: "Total".to_string(),
        total_commits: totals_json
            .get("total_commits")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        total_additions: totals_json
            .get("total_additions")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        total_deletions: totals_json
            .get("total_deletions")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        total_net_modifications: totals_json
            .get("total_net_modifications")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        total_net_additions: totals_json
            .get("total_net_additions")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        by_language: totals_json
            .get("by_language")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()?
            .unwrap_or_default(),
        by_author: HashMap::new(),
    };

    Ok((user, totals, summary, active_repos, days, username))
}

#[cfg(feature = "github")]
#[allow(clippy::too_many_arguments)]
fn fetch_github_data(
    username: &str,
    days: Option<f64>,
    since: Option<&str>,
    until: Option<&str>,
    include_forks: bool,
    include_contributed: bool,
    include_private: bool,
    no_cache: bool,
    refresh_cache: bool,
) -> anyhow::Result<(
    github::api::GithubUser,
    Vec<github::api::RepoContribution>,
    github::ContributionSummary,
    u64,
)> {
    let observed_at = Utc::now();
    let query_window = resolve_github_query_window(days, since, until, observed_at)?;
    let (cache_policy, warn_no_cache_override) =
        github::api::CachePolicy::from_flags(no_cache, refresh_cache);
    if warn_no_cache_override {
        eprintln!("Warning: --no-cache overrides --refresh-cache; cache is disabled");
    }
    let client = github::GithubClient::new()?;
    if !client.has_token() {
        anyhow::bail!("GITHUB_TOKEN environment variable is required for the github subcommand.");
    }

    let user = client.get_user(username)?;

    let (contributions, contribution_summary) = github::api::fetch_user_stats(
        &client,
        &user.node_id,
        username,
        include_forks,
        include_contributed,
        include_private,
        &query_window,
        cache_policy,
    )?;

    let days_value = if let Some(days) = days {
        days.ceil() as u64
    } else if since.is_some() {
        let diff = observed_at - query_window.requested_from;
        diff.num_days().max(1) as u64
    } else {
        365
    };

    Ok((user, contributions, contribution_summary, days_value))
}

#[cfg(feature = "github")]
fn parse_period(s: &str) -> anyhow::Result<f64> {
    let s = s.trim();
    match s.to_ascii_lowercase().as_str() {
        "week" | "w" => return Ok(7.0),
        "month" | "m" => return Ok(30.0),
        "quarter" | "q" => return Ok(90.0),
        "half" | "h" => return Ok(180.0),
        "year" | "y" => return Ok(365.0),
        _ => {}
    }
    let num_str = s.trim_end_matches(['d', 'D']);
    if num_str.is_empty() {
        anyhow::bail!(
            "Invalid period '{s}'. Expected: week, month, quarter, year, or Nd (e.g. 7d, 30d)"
        );
    }
    let days = num_str.parse::<f64>().map_err(|_| {
        anyhow::anyhow!(
            "Invalid period '{s}'. Expected: week, month, quarter, year, or Nd (e.g. 7d, 30d)"
        )
    })?;
    if !days.is_finite() || days <= 0.0 {
        anyhow::bail!("Invalid period '{s}': numeric periods must be finite and greater than zero");
    }
    duration_for_days(days, "period")
        .map_err(|e| anyhow::anyhow!("Invalid period '{s}': range is too large: {e}"))?;
    Ok(days)
}

#[cfg(feature = "github")]
fn cmd_github_multi(args: cli::GithubMultiArgs) -> anyhow::Result<()> {
    let observed_at = Utc::now();
    let periods: Vec<(f64, github::api::QueryWindow)> = args
        .periods
        .iter()
        .map(|period| {
            let days = parse_period(period)?;
            let query_window = resolve_github_query_window(Some(days), None, None, observed_at)
                .map_err(|error| anyhow::anyhow!("Invalid period '{period}': {error}"))?;
            Ok((days, query_window))
        })
        .collect::<anyhow::Result<_>>()?;
    let exclude_rules: Vec<exclude::ExcludeRule> = args
        .exclude
        .iter()
        .map(|v| exclude::ExcludeRule::parse_many(v))
        .collect::<anyhow::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let (cache_policy, warn_no_cache_override) =
        github::api::CachePolicy::from_flags(args.no_cache, args.refresh_cache);
    if warn_no_cache_override {
        eprintln!("Warning: --no-cache overrides --refresh-cache; cache is disabled");
    }
    let client = github::GithubClient::new()?;
    if !client.has_token() {
        anyhow::bail!("GITHUB_TOKEN environment variable is required for the github subcommand.");
    }

    let user = client.get_user(&args.username)?;

    if exclude::any_path_rules(&exclude_rules) {
        eprintln!(
            "Warning: --exclude path rules are ignored in GitHub mode (no per-file data from API)"
        );
    }

    let mut columns = Vec::new();
    for (days, query_window) in periods {
        let (mut contributions, _summary) = github::api::fetch_user_stats(
            &client,
            &user.node_id,
            &args.username,
            args.include_forks,
            args.include_contributed,
            args.include_private,
            &query_window,
            cache_policy,
        )?;

        if !exclude_rules.is_empty() {
            contributions.retain(|c| {
                !exclude::is_repo_excluded_with_mode(
                    &c.repo_name,
                    &exclude_rules,
                    exclude::RepoMatchMode::Github,
                )
            });
            for c in &mut contributions {
                let langs = exclude::excluded_langs_for_repo_with_mode(
                    &c.repo_name,
                    &exclude_rules,
                    exclude::RepoMatchMode::Github,
                );
                for lang in langs {
                    c.languages.retain(|l, _| !l.eq_ignore_ascii_case(&lang));
                }
            }
        }

        if contributions.is_empty() {
            continue;
        }

        let period_stats =
            github::api::contributions_to_period_stats(&contributions, &Period::Month);
        let mut totals = stats::aggregator::aggregate_totals(&period_stats);

        if !args.exclude_lang.is_empty() {
            stats::aggregator::remove_excluded_from_period(&mut totals, &args.exclude_lang);
        }

        if contributions.is_empty() {
            continue;
        }

        columns.push(github::MultiColumnData {
            days: days.ceil() as u64,
            stats: totals,
            active_repos: contributions.len(),
        });
    }

    let svg = github::render_multi_card(&columns, args.number_format, args.number_format_lines)?;
    write_output(svg, args.output.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[cfg(feature = "github")]
    use std::cell::RefCell;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 2, 10, 12, 0, 0)
            .single()
            .unwrap()
    }

    #[cfg(feature = "github")]
    fn identity_report(
        login: &str,
        emails: &[&str],
        partial: bool,
    ) -> github::api::IdentityResolutionReport {
        github::api::IdentityResolutionReport {
            login: login.to_string(),
            emails: emails.iter().map(|email| (*email).to_string()).collect(),
            repositories_examined: 1,
            logical_requests: 1,
            truncated_repositories: partial,
            truncated_commits: false,
            failures: Vec::new(),
        }
    }

    #[cfg(feature = "github")]
    fn remote_commit(repo_id: &str, email: &str) -> CommitStats {
        CommitStats {
            repo_id: repo_id.to_string(),
            repo: repo_id.to_string(),
            oid: format!("{repo_id}-oid"),
            author: stats::models::Author {
                name: "Author".to_string(),
                email: email.to_string(),
            },
            committer: stats::models::Author {
                name: "Committer".to_string(),
                email: "committer@example.com".to_string(),
            },
            co_authors: Vec::new(),
            timestamp: fixed_now(),
            message_subject: "test".to_string(),
            file_changes: Vec::new(),
        }
    }

    #[cfg(feature = "github")]
    #[test]
    fn identity_resolution_deduplicates_shared_logins_and_bounds_sorted_requests() {
        let me = filter::parse_me_expr("github:OctoCat").unwrap();
        let rules = exclude::ExcludeRule::parse_many(":author:github:octocat").unwrap();
        let requested = collect_requested_github_logins(Some(&me), &rules);
        let calls = RefCell::new(Vec::new());

        let resolution = resolve_identity_reports(&requested, |login| {
            calls.borrow_mut().push(login.to_string());
            identity_report(login, &["octocat@example.com"], false)
        });

        assert_eq!(requested, BTreeSet::from(["octocat".to_string()]));
        assert_eq!(*calls.borrow(), vec!["octocat".to_string()]);
        assert_eq!(
            resolution.email_to_login_map(),
            HashMap::from([("octocat@example.com".to_string(), "octocat".to_string())])
        );

        let requested = (0..10)
            .rev()
            .map(|index| format!("user-{index:02}"))
            .collect::<BTreeSet<_>>();
        let calls = RefCell::new(Vec::new());

        let resolution = resolve_identity_reports(&requested, |login| {
            calls.borrow_mut().push(login.to_string());
            identity_report(login, &[], matches!(login, "user-00" | "user-01"))
        });

        assert_eq!(
            *calls.borrow(),
            (0..8)
                .map(|index| format!("user-{index:02}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(resolution.skipped_logins, 2);
        let warning = resolution.warning().expect("one combined warning");
        assert_eq!(warning.matches("partial").count(), 1, "{warning}");
        assert!(warning.contains("2 login(s)"), "{warning}");
    }

    #[cfg(feature = "github")]
    #[test]
    fn remote_identity_map_continues_after_misses_and_keeps_first_conflict() {
        let repos = vec![
            analyze::RepoInput {
                path: PathBuf::from("one"),
                id: "one".to_string(),
                label: "one".to_string(),
            },
            analyze::RepoInput {
                path: PathBuf::from("two"),
                id: "two".to_string(),
                label: "two".to_string(),
            },
        ];
        let commits = vec![
            remote_commit("one", "Alice@Example.com"),
            remote_commit("two", "alice@example.com"),
        ];
        let calls = RefCell::new(Vec::new());

        let report = build_remote_identity_map_with(
            &repos,
            &commits,
            |repo| match repo.id.as_str() {
                "one" => Some(("one".to_string(), "first".to_string())),
                "two" => Some(("two".to_string(), "second".to_string())),
                _ => None,
            },
            |owner, repo, email| {
                calls.borrow_mut().push(format!("{owner}/{repo}:{email}"));
                Ok(if owner == "two" {
                    Some("OctoCat".to_string())
                } else {
                    None
                })
            },
        );

        assert_eq!(
            *calls.borrow(),
            vec![
                "one/first:alice@example.com".to_string(),
                "two/second:alice@example.com".to_string(),
            ]
        );
        assert_eq!(
            report.mappings,
            HashMap::from([("alice@example.com".to_string(), "octocat".to_string())])
        );
        assert!(report.warnings.is_empty());

        let repos = vec![
            analyze::RepoInput {
                path: PathBuf::from("one"),
                id: "one".to_string(),
                label: "one".to_string(),
            },
            analyze::RepoInput {
                path: PathBuf::from("two"),
                id: "two".to_string(),
                label: "two".to_string(),
            },
        ];
        let commits = vec![
            remote_commit("one", "alice@example.com"),
            remote_commit("two", "alice@example.com"),
        ];

        let report = build_remote_identity_map_with(
            &repos,
            &commits,
            |repo| match repo.id.as_str() {
                "one" => Some(("one".to_string(), "first".to_string())),
                "two" => Some(("two".to_string(), "second".to_string())),
                _ => None,
            },
            |owner, _, _| {
                Ok(Some(
                    if owner == "one" { "Alice" } else { "Bob" }.to_string(),
                ))
            },
        );

        assert_eq!(
            report.mappings,
            HashMap::from([("alice@example.com".to_string(), "alice".to_string())])
        );
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("one/first"));
        assert!(report.warnings[0].contains("two/second"));
    }

    #[test]
    fn time_validation_and_duration_rounding_cover_edge_cases() {
        let error = resolve_time_range(None, Some("2025-02-02"), Some("2025-02-01"), fixed_now())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("--since must not be after --until")
        );

        let range = resolve_time_range(None, None, Some("2025-02-01"), fixed_now()).unwrap();
        assert_eq!(
            range.until_exclusive,
            Some(Utc.with_ymd_and_hms(2025, 2, 2, 0, 0, 0).single().unwrap())
        );

        for days in [-1.0, 0.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(
                resolve_time_range(Some(days), None, None, fixed_now()).is_err(),
                "expected {days:?} to be rejected"
            );
        }
        assert_eq!(
            duration_for_days(0.000_001, "--days")
                .unwrap()
                .num_seconds(),
            1
        );
        assert_eq!(
            duration_for_days(0.5, "--days").unwrap().num_seconds(),
            12 * 60 * 60
        );
    }

    #[cfg(feature = "github")]
    #[test]
    fn github_query_windows_use_one_clock_and_semantic_scopes() {
        use github::api::{CachePolicy, CacheWindowScope};

        let observed_at = fixed_now();
        let rolling = resolve_github_query_window(Some(0.5), None, None, observed_at).unwrap();
        assert_eq!(
            rolling.scope,
            CacheWindowScope::Rolling {
                lookback_nanoseconds: 43_200_000_000_000,
            }
        );
        assert_eq!(
            rolling.requested_from,
            observed_at - chrono::Duration::hours(12)
        );
        assert_eq!(rolling.until_exclusive, observed_at);
        assert!(!rolling.completed);

        let rolling_with_future_until =
            resolve_github_query_window(Some(0.5), None, Some("2025-02-20"), observed_at).unwrap();
        assert_eq!(
            rolling_with_future_until.scope,
            CacheWindowScope::Rolling {
                lookback_nanoseconds: 43_200_000_000_000,
            }
        );
        assert_eq!(
            rolling_with_future_until.requested_from,
            observed_at - chrono::Duration::hours(12)
        );
        assert_eq!(rolling_with_future_until.until_exclusive, observed_at);
        assert!(!rolling_with_future_until.completed);

        let anchored =
            resolve_github_query_window(None, Some("2025-02-01"), None, observed_at).unwrap();
        assert_eq!(
            anchored.scope,
            CacheWindowScope::Anchored {
                from: Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).single().unwrap(),
            }
        );
        assert_eq!(anchored.until_exclusive, observed_at);
        assert!(!anchored.completed);

        let anchored_with_future_until =
            resolve_github_query_window(None, Some("2025-02-01"), Some("2025-02-20"), observed_at)
                .unwrap();
        assert_eq!(
            anchored_with_future_until.scope,
            CacheWindowScope::Anchored {
                from: Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).single().unwrap(),
            }
        );
        assert_eq!(anchored_with_future_until.until_exclusive, observed_at);
        assert!(!anchored_with_future_until.completed);

        let fixed =
            resolve_github_query_window(None, Some("2025-02-01"), Some("2025-02-02"), observed_at)
                .unwrap();
        assert_eq!(
            fixed.scope,
            CacheWindowScope::Fixed {
                from: Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).single().unwrap(),
                until_exclusive: Utc.with_ymd_and_hms(2025, 2, 3, 0, 0, 0).single().unwrap(),
            }
        );
        assert!(fixed.completed);

        let (policy, warn) = CachePolicy::from_flags(true, true);
        assert_eq!(policy, CachePolicy::Disabled);
        assert!(warn);
        assert!(!policy.can_read());
        assert!(!policy.can_write());
    }

    #[cfg(feature = "github")]
    #[test]
    fn invalid_numeric_periods_are_rejected() {
        for period in ["0", "-1", "NaN", "inf", "1e300"] {
            assert!(
                parse_period(period).is_err(),
                "expected '{period}' to be rejected"
            );
        }
        assert_eq!(parse_period("7d").unwrap(), 7.0);
    }
}
