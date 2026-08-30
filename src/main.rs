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

    let identity_map = build_identity_map(&args.dedup, &repos, &commits);

    let me_expr = args.me.as_deref().map(filter::parse_me_expr).transpose()?;

    let commits = if let Some(ref expr) = me_expr {
        commits
            .into_iter()
            .filter(|c| expr.matches_commit(c, &identity_map))
            .collect()
    } else {
        commits
    };

    #[cfg(feature = "github")]
    {
        let github_users = exclude::collect_github_users(&exclude_rules);
        if !github_users.is_empty() {
            match github::GithubClient::new() {
                Ok(client) => {
                    for username in &github_users {
                        match client.resolve_user_emails(username) {
                            Ok(emails) => {
                                eprintln!("Resolved @{username} → {} email(s)", emails.len());
                                for rule in &mut exclude_rules {
                                    rule.resolve_github_user(username, &emails);
                                }
                            }
                            Err(e) => eprintln!("Warning: failed to resolve @{username}: {e}"),
                        }
                    }
                }
                Err(_) => {
                    eprintln!(
                        "Warning: --exclude with @user requires GITHUB_TOKEN; skipping GitHub resolution"
                    );
                }
            }
        }
    }

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
    let mut github_info = None;
    for repo in repos {
        if let Some(url) = git::repo::get_remote_origin(&repo.path)
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

    eprintln!(
        "Resolving {} email(s) via GitHub API ({}/{})...",
        all_emails.len(),
        info.owner,
        info.repo
    );
    client
        .resolve_emails(&info.owner, &info.repo, &all_emails)
        .into_iter()
        .map(|(email, login)| (email.to_ascii_lowercase(), login))
        .collect()
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

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 2, 10, 12, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn reversed_date_range_is_rejected_before_analysis() {
        let error = resolve_time_range(None, Some("2025-02-02"), Some("2025-02-01"), fixed_now())
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("--since must not be after --until")
        );
    }

    #[test]
    fn date_only_until_becomes_exclusive_next_midnight() {
        let range = resolve_time_range(None, None, Some("2025-02-01"), fixed_now()).unwrap();

        assert_eq!(
            range.until_exclusive,
            Some(Utc.with_ymd_and_hms(2025, 2, 2, 0, 0, 0).single().unwrap())
        );
    }

    #[test]
    fn invalid_days_are_rejected() {
        for days in [-1.0, 0.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(
                resolve_time_range(Some(days), None, None, fixed_now()).is_err(),
                "expected {days:?} to be rejected"
            );
        }
    }

    #[test]
    fn positive_fractional_days_round_up_to_one_second() {
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
