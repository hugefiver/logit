use std::path::Path;

use tokei::{Config, LanguageType};

use crate::stats::models::FileChange;

/// Classify a file path into a language name using tokei.
/// Returns None for unrecognized file types.
pub fn classify_language(path: &str) -> Option<String> {
    let path = Path::new(path);
    LanguageType::from_path(path, &Config::default()).map(|lt| lt.to_string())
}

/// Apply language classification to a list of file changes.
/// Sets the `language` field on each FileChange based on its path.
pub fn apply_language_to_changes(changes: &mut [FileChange]) {
    for change in changes.iter_mut() {
        change.language = classify_language(&change.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_rust() {
        assert_eq!(classify_language("src/main.rs"), Some("Rust".to_string()));
    }

    #[test]
    fn classify_python() {
        assert_eq!(classify_language("lib.py"), Some("Python".to_string()));
    }

    #[test]
    fn classify_javascript() {
        assert_eq!(classify_language("app.js"), Some("JavaScript".to_string()));
    }

    #[test]
    fn classify_css() {
        assert_eq!(classify_language("style.css"), Some("CSS".to_string()));
    }

    #[test]
    fn classify_markdown() {
        assert_eq!(classify_language("README.md"), Some("Markdown".to_string()));
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(classify_language("unknown.xyz"), None);
    }

    #[test]
    fn classify_makefile() {
        let result = classify_language("Makefile");
        assert!(result.is_some(), "Makefile should be classified");
    }

    #[test]
    fn classify_nested_path() {
        assert_eq!(
            classify_language("src/lib/utils.rs"),
            Some("Rust".to_string())
        );
    }

    #[test]
    fn apply_to_changes() {
        let mut changes = vec![
            FileChange {
                path: "main.rs".to_string(),
                language: None,
                additions: 10,
                deletions: 0,
                net_modifications: 10,
                net_additions: 10,
            },
            FileChange {
                path: "lib.py".to_string(),
                language: None,
                additions: 5,
                deletions: 2,
                net_modifications: 5,
                net_additions: 3,
            },
        ];
        apply_language_to_changes(&mut changes);
        assert_eq!(changes[0].language, Some("Rust".to_string()));
        assert_eq!(changes[1].language, Some("Python".to_string()));
    }
}
