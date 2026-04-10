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
        Commands::Github(args) => cmd_github(args),
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

    let repos: Vec<PathBuf> = if args.path.join(".git").exists() {
        vec![args.path.clone()]
    } else {
        scanner::scan_for_repos(&args.path)?
    };

    let (commits, errors) = analyze::analyze_repos(&repos, since, until);

    for e in &errors {
        eprintln!("Warning: failed to analyze {}: {}", e.path.display(), e.error);
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

    let mut period_stats = if matches!(args.group, GroupBy::Repo) {
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
            let content = output::table::render_stats_table(&period_stats, &totals, &args.group, &args.show_email, &args.dedup, &identity_map, args.sort.as_ref(), args.short, args.compact, args.inline_tree);
            write_output(content, args.output.as_deref())?;
        }
        OutputFormat::Json => {
            let content = output::json::render_stats_json(&period_stats, &totals)?;
            write_output(content, args.output.as_deref())?;
        }
        #[cfg(feature = "tui")]
        OutputFormat::Tui => output::tui::run_tui(&period_stats, &totals)?,
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
fn cmd_github(args: cli::GithubArgs) -> anyhow::Result<()> {
    use cli::ExportFormat;

    let client = github::GithubClient::new()?;
    if !client.has_token() {
        anyhow::bail!("GITHUB_TOKEN environment variable is required for the github subcommand.");
    }
    let user = client.get_user(&args.username)?;

    let since_ts = if let Some(days) = args.days {
        let duration = chrono::Duration::seconds((days * 86400.0) as i64);
        Some((Utc::now() - duration).timestamp())
    } else if let Some(ref since_str) = args.since {
        Some(parse_date(since_str)?.timestamp())
    } else {
        None
    };

    let until_ts = args
        .until
        .as_deref()
        .map(parse_date)
        .transpose()?
        .map(|dt| dt.timestamp());

    let read_cache = !args.no_cache;
    let write_cache = args.refresh_cache;

    let contributions = github::api::fetch_user_stats(
        &client,
        &user.node_id,
        &args.username,
        args.include_forks,
        args.include_contributed,
        since_ts,
        until_ts,
        read_cache,
        write_cache,
    )?;

    let period = args.period.unwrap_or(Period::Month);
    let mut period_stats = if matches!(args.group, GroupBy::Repo) {
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
        ExportFormat::Json => {
            let json = serde_json::json!({
                "user": user,
                "periods": period_stats,
                "totals": {
                    "total_commits": totals.total_commits,
                    "total_additions": totals.total_additions,
                    "total_deletions": totals.total_deletions,
                    "by_language": totals.by_language,
                }
            });
            let content = serde_json::to_string_pretty(&json)?;
            write_output(content, args.output.as_deref())?;
        }
        ExportFormat::Table => {
            let dedup = cli::DedupMode::None;
            let email = cli::EmailDisplay::None;
            let identity_map = HashMap::new();
            let content = output::table::render_stats_table(
                &period_stats,
                &totals,
                &args.group,
                &email,
                &dedup,
                &identity_map,
                args.sort.as_ref(),
                args.short,
                args.compact,
                args.inline_tree,
            );
            write_output(content, args.output.as_deref())?;
        }
        ExportFormat::Svg => {
            let svg = github::render_profile_card(&args.username, &user, Some(&totals))?;
            write_output(svg, args.output.as_deref())?;
        }
    }

    Ok(())
}
