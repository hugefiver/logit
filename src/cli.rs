use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "logit", version, about = "lines of git")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Scan(ScanArgs),
    Stats(StatsArgs),
    #[cfg(feature = "github")]
    #[command(subcommand)]
    Github(GithubSubcommand),
}

#[derive(clap::Args)]
pub struct ScanArgs {
    pub path: PathBuf,

    #[arg(short = 'f', long, value_enum, default_value_t = ScanFormat::Table)]
    pub format: ScanFormat,

    #[arg(short = 'o', long, help = "Write output to file instead of stdout")]
    pub output: Option<PathBuf>,
}

#[derive(Clone, ValueEnum)]
pub enum ScanFormat {
    Table,
    Json,
}

#[derive(clap::Args)]
pub struct StatsArgs {
    #[arg(long, default_value = ".")]
    pub path: PathBuf,

    #[arg(long, help = "Filter by author name or email")]
    pub author: Option<String>,

    #[arg(long, help = "Filter by committer name or email")]
    pub committer: Option<String>,

    #[arg(long, help = "Only show stats since this date (YYYY-MM-DD)")]
    pub since: Option<String>,

    #[arg(long, help = "Only show stats until this date (YYYY-MM-DD)")]
    pub until: Option<String>,

    #[arg(long, value_enum, help = "Period granularity for stats")]
    pub period: Option<Period>,

    #[arg(long, help = "Filter commits by programming language")]
    pub lang: Option<String>,

    #[arg(
        long,
        help = "Exclude languages from stats (comma-separated)",
        value_delimiter = ','
    )]
    pub exclude_lang: Vec<String>,

    #[arg(short = 'f', long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,

    #[arg(short = 'o', long, help = "Write output to file instead of stdout")]
    pub output: Option<PathBuf>,

    #[arg(long, help = "Restrict to specific repos (repeatable)")]
    pub repo: Option<Vec<String>>,

    #[arg(long, value_enum, default_value_t = GroupBy::Language, help = "Group stats by language, author, period, or repo")]
    pub group: GroupBy,

    #[arg(
        short = 'd',
        long,
        help = "Show data from last N days (can be fractional, e.g. 0.5 for 12 hours)",
        conflicts_with = "since"
    )]
    pub days: Option<f64>,

    #[arg(long, value_enum, default_value_t = EmailDisplay::None, default_missing_value = "simple", num_args = 0..=1)]
    pub show_email: EmailDisplay,

    #[arg(long, value_enum, default_value_t = DedupMode::Name, help = "Author deduplication mode")]
    pub dedup: DedupMode,

    /// Filter commits by identity expression.
    /// Supports: `github:username`, `name:Name`, `email:a@b.com`, bare `Name`.
    /// Combine with `|` (or), `&` (and), `()` (grouping).
    /// Example: `github:octocat|email:me@example.com`
    #[arg(long)]
    pub me: Option<String>,

    #[arg(long, value_enum, help = "Sort by column")]
    pub sort: Option<SortBy>,

    #[arg(long, help = "Use short number format (1.2k, 3.4M)")]
    pub short: bool,

    #[arg(long, help = "Show language details inline under each group")]
    pub inline_tree: bool,

    #[arg(long, help = "Use compact single-line format for changes")]
    pub compact: bool,
}

#[derive(Clone, ValueEnum)]
pub enum DedupMode {
    None,
    Name,
    #[cfg(feature = "github")]
    Remote,
}

#[derive(Clone, ValueEnum)]
pub enum EmailDisplay {
    None,
    Simple,
    Full,
}

#[derive(Clone, ValueEnum)]
pub enum GroupBy {
    Language,
    Author,
    Period,
    Repo,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SortBy {
    Commits,
    Additions,
    Deletions,
    Files,
    Name,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Period {
    Day,
    Week,
    Month,
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    #[cfg(feature = "tui")]
    Tui,
}

#[cfg(feature = "github")]
#[derive(clap::Args)]
pub struct GithubDataArgs {
    /// GitHub username
    pub username: String,

    #[arg(long, help = "Only show stats since this date (YYYY-MM-DD)")]
    pub since: Option<String>,

    #[arg(long, help = "Only show stats until this date (YYYY-MM-DD)")]
    pub until: Option<String>,

    #[arg(
        short = 'd',
        long,
        help = "Show stats for the last N days",
        conflicts_with = "since"
    )]
    pub days: Option<f64>,

    #[arg(long, help = "Include forked repos")]
    pub include_forks: bool,

    #[arg(long, help = "Include repos contributed to (full history via GraphQL)")]
    pub include_contributed: bool,

    #[arg(long, help = "Bypass disk cache (no read, no write)")]
    pub no_cache: bool,

    #[arg(
        long,
        help = "Read cached data and fetch incremental updates, then write back"
    )]
    pub refresh_cache: bool,
}

#[cfg(feature = "github")]
#[derive(Subcommand)]
pub enum GithubSubcommand {
    /// Fetch GitHub user stats and export as JSON or table
    Fetch(GithubFetchArgs),
    /// Generate SVG profile card
    Card(GithubCardArgs),
    /// Generate multi-period comparison SVG card
    Multi(GithubMultiArgs),
}

#[cfg(feature = "github")]
#[derive(clap::Args)]
pub struct GithubFetchArgs {
    #[command(flatten)]
    pub data: GithubDataArgs,

    #[arg(short = 'f', long, value_enum, default_value_t = FetchFormat::Json)]
    pub format: FetchFormat,

    #[arg(short = 'o', long, help = "Write output to file instead of stdout")]
    pub output: Option<PathBuf>,

    #[arg(long, value_enum, help = "Period granularity for stats")]
    pub period: Option<Period>,

    #[arg(long, value_enum, default_value_t = GroupBy::Language, help = "Group stats by language, author, period, or repo")]
    pub group: GroupBy,

    #[arg(long, help = "Use short number format (1.2k, 3.4M)")]
    pub short: bool,

    #[arg(long, help = "Use compact single-line format for changes")]
    pub compact: bool,

    #[arg(long, help = "Show language details inline under each group")]
    pub inline_tree: bool,

    #[arg(
        long,
        help = "Exclude languages from stats (comma-separated)",
        value_delimiter = ','
    )]
    pub exclude_lang: Vec<String>,

    #[arg(long, value_enum, help = "Sort by column")]
    pub sort: Option<SortBy>,
}

#[cfg(feature = "github")]
#[derive(Clone, ValueEnum)]
pub enum FetchFormat {
    Json,
    Table,
}

#[cfg(feature = "github")]
#[derive(clap::Args)]
pub struct GithubCardArgs {
    /// GitHub username (fetch data live). Mutually exclusive with --input.
    pub username: Option<String>,

    /// Load data from previously exported JSON file instead of fetching
    #[arg(short = 'i', long)]
    pub input: Option<PathBuf>,

    // Data-fetching options (only used when username is provided, ignored with --input):
    #[arg(long, help = "Only show stats since this date (YYYY-MM-DD)")]
    pub since: Option<String>,

    #[arg(long, help = "Only show stats until this date (YYYY-MM-DD)")]
    pub until: Option<String>,

    #[arg(
        short = 'd',
        long,
        help = "Show stats for the last N days",
        conflicts_with = "since"
    )]
    pub days: Option<f64>,

    #[arg(long, help = "Include forked repos")]
    pub include_forks: bool,

    #[arg(long, help = "Include repos contributed to (full history via GraphQL)")]
    pub include_contributed: bool,

    #[arg(long, help = "Bypass disk cache (no read, no write)")]
    pub no_cache: bool,

    #[arg(long, help = "Read cached data and fetch incremental updates")]
    pub refresh_cache: bool,

    // SVG-specific options:
    #[arg(long, help = "Custom title for the SVG card")]
    pub title: Option<String>,

    #[arg(long, help = "Use short mode (2 columns, fewer stats)")]
    pub short: bool,

    #[arg(
        long,
        default_value_t = 2,
        help = "Number of language rows in legend (default: 2)"
    )]
    pub lang_rows: usize,

    #[arg(
        long,
        help = "Exclude languages from stats (comma-separated)",
        value_delimiter = ','
    )]
    pub exclude_lang: Vec<String>,

    #[arg(short = 'o', long, help = "Write SVG to file instead of stdout")]
    pub output: Option<PathBuf>,
}

#[cfg(feature = "github")]
#[derive(clap::Args)]
pub struct GithubMultiArgs {
    pub username: String,

    #[arg(
        short = 'p',
        long,
        value_delimiter = ',',
        required = true,
        help = "Time periods to compare (e.g. 2d,7d,30d)"
    )]
    pub periods: Vec<String>,

    #[arg(long, help = "Include forked repos")]
    pub include_forks: bool,

    #[arg(long, help = "Include repos contributed to (full history via GraphQL)")]
    pub include_contributed: bool,

    #[arg(long, help = "Bypass disk cache (no read, no write)")]
    pub no_cache: bool,

    #[arg(long, help = "Read cached data and fetch incremental updates")]
    pub refresh_cache: bool,

    #[arg(
        long,
        help = "Exclude languages from stats (comma-separated)",
        value_delimiter = ','
    )]
    pub exclude_lang: Vec<String>,

    #[arg(short = 'o', long, help = "Write SVG to file instead of stdout")]
    pub output: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_cli() {
        Cli::command().debug_assert();
    }
}
