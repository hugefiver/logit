//! logit — lines of git
//!
//! CLI tool for analyzing git repository history with per-language,
//! per-author, and per-time-period statistics.

mod analyze;
mod cli;
mod error;
mod filter;
mod git;
mod lang;
mod output;
mod scanner;
mod stats;

#[cfg(feature = "github")]
mod github;

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::Parser;

use cli::{Cli, Commands, GroupBy, OutputFormat, Period, ScanArgs, ScanFormat, StatsArgs};

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
    let repos = scanner::scan_for_repos(&args.path)?;
    let content = match args.format {
        ScanFormat::Table => output::table::render_scan_table(&repos),
        ScanFormat::Json => output::json::render_scan_json(&repos)?,
    };
    write_output(content, args.output.as_deref())
}

fn cmd_stats(args: StatsArgs) -> anyhow::Result<()> {
    let since = if let Some(days) = args.days {
        let duration = chrono::Duration::seconds((days * 86400.0) as i64);
        Some(Utc::now() - duration)
    } else {
        args.since.as_deref().map(parse_date).transpose()?
    };
    let until = args
        .until
        .as_deref()
        .map(parse_date)
        .transpose()?;

    let mut repos: Vec<PathBuf> = Vec::new();
    for path in &args.paths {
        if path.join(".git").exists() {
            repos.push(path.clone());
        } else {
            repos.extend(scanner::scan_for_repos(path)?);
        }
    }

    let (commits, errors) = analyze::analyze_repos(&repos, since, until);

    for e in &errors {
        eprintln!("Warning: failed to analyze {}: {}", e.path.display(), e.error);
    }

    if commits.is_empty() {
        eprintln!("No commits found in the given period.");
        return Ok(());
    }

    let active_repos: std::collections::HashSet<&str> =
        commits.iter().map(|c| c.repo.as_str()).collect();
    let skipped = repos.len() - active_repos.len();
    if skipped > 0 {
        eprintln!("Skipped {skipped} repo(s) with no activity in the period.");
    }

    let identity_map = build_identity_map(&args.dedup, &repos, &commits);

    let me_expr = args
        .me
        .as_deref()
        .map(filter::parse_me_expr)
        .transpose()?;

    let commits = if let Some(ref expr) = me_expr {
        commits
            .into_iter()
            .filter(|c| expr.matches_commit(c, &identity_map))
            .collect()
    } else {
        commits
    };

    let period = args.period.unwrap_or(Period::Month);
    let author_filter = args.author.as_deref();
    let lang_filter = args.lang.as_deref();
    let num_fmt = if args.short { cli::NumberFormat::Short } else { args.number_format };
    let compact = !args.no_compact;
    let columns = cli::resolve_columns(&args.columns, &args.exclude_columns);

    if let Err(e) = stats::aggregator::validate_groups(&args.group) {
        anyhow::bail!(e);
    }
    if let Err(e) = stats::aggregator::validate_groups(&args.groups) {
        anyhow::bail!(e);
    }

    let primary_candidates =
        stats::aggregator::effective_groups(&commits, &args.group, &period);
    let primary = primary_candidates
        .first()
        .copied()
        .unwrap_or(GroupBy::Language);

    let mut effective: Vec<GroupBy> = Vec::with_capacity(1 + args.groups.len());
    effective.push(primary);
    for g in &args.groups {
        if !effective.contains(g) {
            effective.push(*g);
        }
    }
    if let Err(e) = stats::aggregator::validate_groups(&effective) {
        anyhow::bail!(e);
    }

    let use_tree = !args.groups.is_empty();

    if use_tree {
        let mut nodes = stats::aggregator::build_group_tree(
            &commits, &effective, &period, author_filter, lang_filter,
        );

        if !args.exclude_lang.is_empty() {
            stats::aggregator::filter_excluded_languages_tree(&mut nodes, &args.exclude_lang);
        }

        match args.format {
            OutputFormat::Table => {
                let leaf_group = effective.last().copied().unwrap_or(GroupBy::Language);
                let content = output::table::render_group_tree(
                    &nodes,
                    &leaf_group,
                    args.sort.as_ref(),
                    num_fmt,
                    &columns,
                    compact,
                    args.inline_tree,
                );
                write_output(content, args.output.as_deref())?;
            }
            OutputFormat::Json => {
                let content = serde_json::to_string_pretty(&nodes)?;
                write_output(content, args.output.as_deref())?;
            }
            #[cfg(feature = "tui")]
            OutputFormat::Tui => {
                eprintln!("TUI mode does not support multi-group trees yet");
            }
        }
    } else {
        let group = primary;

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
                let content = output::table::render_stats_table(
                    &period_stats,
                    &totals,
                    &group,
                    &args.show_email,
                    &args.dedup,
                    &identity_map,
                    args.sort.as_ref(),
                    num_fmt,
                    &columns,
                    compact,
                    args.inline_tree,
                );
                write_output(content, args.output.as_deref())?;
            }
            OutputFormat::Json => {
                let content = output::json::render_stats_json(&period_stats, &totals)?;
                write_output(content, args.output.as_deref())?;
            }
            #[cfg(feature = "tui")]
            OutputFormat::Tui => output::tui::run_tui(&period_stats, &totals)?,
        }
    }

    Ok(())
}

fn parse_date(s: &str) -> anyhow::Result<DateTime<Utc>> {
    let naive = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}. Expected format: YYYY-MM-DD", s, e))?;
    let midnight = naive
        .and_hms_opt(0, 0, 0)
        .expect("midnight (00:00:00) is always a valid time");
    Ok(midnight.and_utc())
}

fn build_identity_map(
    _dedup: &cli::DedupMode,
    _repos: &[PathBuf],
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
    repos: &[PathBuf],
    commits: &[stats::models::CommitStats],
) -> HashMap<String, String> {
    let mut github_info = None;
    for repo_path in repos {
        if let Some(url) = git::repo::get_remote_origin(repo_path)
            && let Some(info) = git::repo::parse_remote_url(&url)
            && matches!(info.platform, git::repo::Platform::GitHub)
        {
            github_info = Some(info);
            break;
        }
    }

    let Some(info) = github_info else {
        return HashMap::new();
    };

    let mut all_emails: Vec<String> = Vec::new();
    for commit in commits {
        let email = &commit.author.email;
        if !all_emails.contains(email) {
            all_emails.push(email.clone());
        }
        for co in &commit.co_authors {
            if !all_emails.contains(&co.email) {
                all_emails.push(co.email.clone());
            }
        }
    }

    let client = match github::GithubClient::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: failed to create GitHub client for remote dedup: {e}");
            return HashMap::new();
        }
    };

    eprintln!("Resolving {} email(s) via GitHub API ({}/{})...", all_emails.len(), info.owner, info.repo);
    client.resolve_emails(&info.owner, &info.repo, &all_emails)
}

#[cfg(feature = "github")]
fn cmd_github_fetch(args: cli::GithubFetchArgs) -> anyhow::Result<()> {
    use cli::FetchFormat;

    let (user, contributions, contribution_summary, days_value) = fetch_github_data(
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

    let period = args.period.unwrap_or(Period::Month);
    let num_fmt = if args.short { cli::NumberFormat::Short } else { args.number_format };
    let compact = !args.no_compact;
    let columns = cli::resolve_columns(&args.columns, &args.exclude_columns);

    if let Err(e) = stats::aggregator::validate_groups(&args.group) {
        anyhow::bail!(e);
    }

    let group = args.group.first().copied().unwrap_or(GroupBy::Language);
    let mut period_stats = if matches!(group, GroupBy::Repo) {
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
            let json = serde_json::json!({
                "metadata": {
                    "username": args.data.username,
                    "days": days_value,
                    "active_repos": contributions.len(),
                    "generated_at": chrono::Utc::now().to_rfc3339(),
                },
                "user": user,
                "periods": period_stats,
                "totals": {
                    "total_commits": totals.total_commits,
                    "total_additions": totals.total_additions,
                    "total_deletions": totals.total_deletions,
                    "total_net_modifications": totals.total_net_modifications,
                    "total_net_additions": totals.total_net_additions,
                    "by_language": totals.by_language,
                },
                "summary": contribution_summary,
            });
            let content = serde_json::to_string_pretty(&json)?;
            write_output(content, args.output.as_deref())?;
        }
        FetchFormat::Table => {
            let dedup = cli::DedupMode::None;
            let email = cli::EmailDisplay::None;
            let identity_map = HashMap::new();
            let content = output::table::render_stats_table(
                &period_stats,
                &totals,
                &group,
                &email,
                &dedup,
                &identity_map,
                args.sort.as_ref(),
                num_fmt,
                &columns,
                compact,
                args.inline_tree,
            );
            write_output(content, args.output.as_deref())?;
        }
        #[cfg(feature = "tui")]
        FetchFormat::Tui => output::tui::run_tui(&period_stats, &totals)?,
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

    let (user, mut totals, summary, active_repos, days_value, username) =
        if let Some(ref input_path) = args.input {
            load_card_data_from_json(input_path)?
        } else {
            let username = args
                .username
                .as_deref()
                .expect("username is required when --input is not provided");

            let (user, contributions, contribution_summary, days_value) = fetch_github_data(
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

            let period_stats = github::api::contributions_to_period_stats(&contributions, &Period::Month);
            let totals = stats::aggregator::aggregate_totals(&period_stats);

            (
                user,
                totals,
                contribution_summary,
                contributions.len(),
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
    let client = github::GithubClient::new()?;
    if !client.has_token() {
        anyhow::bail!("GITHUB_TOKEN environment variable is required for the github subcommand.");
    }

    let user = client.get_user(username)?;

    let since_ts = if let Some(days) = days {
        let duration = chrono::Duration::seconds((days * 86400.0) as i64);
        Some((Utc::now() - duration).timestamp())
    } else if let Some(since_str) = since {
        Some(parse_date(since_str)?.timestamp())
    } else {
        None
    };

    let until_ts = until
        .map(parse_date)
        .transpose()?
        .map(|date| date.timestamp());

    let read_cache = !no_cache;
    let write_cache = refresh_cache;

    let (contributions, contribution_summary) = github::api::fetch_user_stats(
        &client,
        &user.node_id,
        username,
        include_forks,
        include_contributed,
        include_private,
        since_ts,
        until_ts,
        read_cache,
        write_cache,
    )?;

    let days_value = if let Some(days) = days {
        days.ceil() as u64
    } else if let Some(since_str) = since {
        let since_dt = parse_date(since_str)?;
        let diff = Utc::now() - since_dt;
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
    num_str.parse::<f64>().map_err(|_| {
        anyhow::anyhow!(
            "Invalid period '{s}'. Expected: week, month, quarter, year, or Nd (e.g. 7d, 30d)"
        )
    })
}

#[cfg(feature = "github")]
fn cmd_github_multi(args: cli::GithubMultiArgs) -> anyhow::Result<()> {
    let client = github::GithubClient::new()?;
    if !client.has_token() {
        anyhow::bail!("GITHUB_TOKEN environment variable is required for the github subcommand.");
    }

    let user = client.get_user(&args.username)?;
    let read_cache = !args.no_cache;
    let write_cache = args.refresh_cache;

    let mut columns = Vec::new();
    for period_str in &args.periods {
        let days = parse_period(period_str)?;
        let duration = chrono::Duration::seconds((days * 86400.0) as i64);
        let since_ts = Some((Utc::now() - duration).timestamp());

        let (contributions, _summary) = github::api::fetch_user_stats(
            &client,
            &user.node_id,
            &args.username,
            args.include_forks,
            args.include_contributed,
            args.include_private,
            since_ts,
            None,
            read_cache,
            write_cache,
        )?;

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
