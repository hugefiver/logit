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
    Github(GithubArgs),
}

#[derive(clap::Args)]
pub struct ScanArgs {
    pub path: PathBuf,

    #[arg(long, value_enum, default_value_t = ScanFormat::Table)]
    pub output: ScanFormat,
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

    #[arg(long)]
    pub author: Option<String>,

    #[arg(long)]
    pub committer: Option<String>,

    #[arg(long)]
    pub since: Option<String>,

    #[arg(long)]
    pub until: Option<String>,

    #[arg(long, value_enum)]
    pub period: Option<Period>,

    #[arg(long)]
    pub lang: Option<String>,

    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub output: OutputFormat,

    #[arg(long)]
    pub repo: Option<Vec<String>>,

    /// Group table output by language, author, or period
    #[arg(long, value_enum, default_value_t = GroupBy::Language)]
    pub group: GroupBy,

    /// Show data from last N days (can be fractional, e.g. 0.5 for 12 hours). Mutually exclusive with --since.
    #[arg(long, conflicts_with = "since")]
    pub days: Option<f64>,

    #[arg(long, value_enum, default_value_t = EmailDisplay::None, default_missing_value = "simple", num_args = 0..=1)]
    pub show_email: EmailDisplay,

    #[arg(long, value_enum, default_value_t = DedupMode::Name)]
    pub dedup: DedupMode,

    /// Filter commits by identity expression.
    /// Supports: `github:username`, `name:Name`, `email:a@b.com`, bare `Name`.
    /// Combine with `|` (or), `&` (and), `()` (grouping).
    /// Example: `github:octocat|email:me@example.com`
    #[arg(long)]
    pub me: Option<String>,

    #[arg(long, value_enum)]
    pub sort: Option<SortBy>,

    #[arg(long, default_value_t = false)]
    pub short: bool,

    #[arg(long, default_value_t = false)]
    pub inline_tree: bool,

    #[arg(long, default_value_t = false)]
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
pub struct GithubArgs {
    pub username: String,

    #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
    pub export: ExportFormat,

    #[arg(long)]
    pub svg_out: Option<PathBuf>,

    #[arg(long, help = "Only show stats since this date (YYYY-MM-DD)")]
    pub since: Option<String>,

    #[arg(
        long,
        help = "Show stats for the last N days (can be decimal)",
        conflicts_with = "since"
    )]
    pub days: Option<f64>,

    #[arg(long, value_enum, help = "Period granularity for stats")]
    pub period: Option<Period>,

    #[arg(long, help = "Include forked repos")]
    pub include_forks: bool,

    #[arg(long, value_enum, default_value_t = GroupBy::Language, help = "Group stats by")]
    pub group: GroupBy,

    #[arg(long, help = "Use short number format (1.2k, 3.4M)")]
    pub short: bool,

    #[arg(long, value_enum, help = "Sort by column")]
    pub sort: Option<SortBy>,
}

#[cfg(feature = "github")]
#[derive(Clone, ValueEnum)]
pub enum ExportFormat {
    Json,
    Svg,
    Table,
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
