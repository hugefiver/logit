use std::path::PathBuf;
use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum LogitError {
    #[error("Git error in repo '{}': {source}", repo.display())]
    Git {
        repo: PathBuf,
        #[source]
        source: git2::Error,
    },

    #[error("Scanner error: {0}")]
    Scanner(String),

    #[error("IO error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("Invalid date format '{input}': {reason}")]
    DateParse { input: String, reason: String },

    #[error("GitHub API error: {0}")]
    #[cfg(feature = "github")]
    Github(String),
}

/// Project-wide Result type alias using anyhow for ergonomic error handling.
#[allow(dead_code)]
pub type Result<T> = anyhow::Result<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_error_display() {
        let err = LogitError::Scanner("directory not found".to_string());
        assert_eq!(err.to_string(), "Scanner error: directory not found");
    }

    #[test]
    fn io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let logit_err = LogitError::from(io_err);
        assert!(logit_err.to_string().contains("IO error"));
    }

    #[test]
    fn date_parse_error_display() {
        let err = LogitError::DateParse {
            input: "not-a-date".to_string(),
            reason: "expected YYYY-MM-DD".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Invalid date format 'not-a-date': expected YYYY-MM-DD"
        );
    }

    #[test]
    fn git_error_display() {
        let git_err = git2::Error::from_str("ref not found");
        let err = LogitError::Git {
            repo: PathBuf::from("/tmp/myrepo"),
            source: git_err,
        };
        let display = err.to_string();
        assert!(display.contains("Git error"));
        assert!(display.contains("myrepo"));
    }

    #[cfg(feature = "github")]
    #[test]
    fn github_error_display() {
        let err = LogitError::Github("rate limit exceeded".to_string());
        assert_eq!(err.to_string(), "GitHub API error: rate limit exceeded");
    }
}
