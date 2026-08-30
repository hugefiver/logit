//! --exclude rule parsing and commit/contribution filtering.
//!
//! `,` = OR (separate groups), `+` = AND (same group, all qualifiers must match).
//! Qualifiers: lang/l, path/p, author/a, committer/c.

use glob::Pattern;
use regex::Regex;

use crate::analyze::platform_repo_key;
use crate::stats::models::Author;
use crate::stats::models::CommitStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoMatchMode {
    LocalPlatform { windows: bool },
    Github,
}

impl RepoMatchMode {
    fn repo_key(self, value: &str) -> String {
        match self {
            Self::LocalPlatform { windows } => platform_repo_key(value, windows),
            Self::Github => platform_repo_key(value, true),
        }
    }
}

fn local_repo_match_mode() -> RepoMatchMode {
    RepoMatchMode::LocalPlatform {
        windows: cfg!(windows),
    }
}

#[derive(Debug, Clone)]
enum AuthorPattern {
    Glob(Pattern),
    #[allow(dead_code)]
    GitHubUser(String),
}

impl AuthorPattern {
    fn matches(&self, name: &str, email: &str) -> bool {
        match self {
            AuthorPattern::Glob(pat) => pat.matches(name) || pat.matches(email),
            AuthorPattern::GitHubUser(_) => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AndGroup {
    lang: Option<String>,
    path_pattern: Option<Pattern>,
    path_regex: Option<Regex>,
    author_pattern: Option<AuthorPattern>,
    committer_pattern: Option<AuthorPattern>,
    author_emails: Vec<String>,
    committer_emails: Vec<String>,
}

impl AndGroup {
    fn has_file_qualifiers(&self) -> bool {
        self.lang.is_some() || self.path_pattern.is_some() || self.path_regex.is_some()
    }

    fn has_commit_qualifiers(&self) -> bool {
        self.author_pattern.is_some() || self.committer_pattern.is_some()
    }

    fn is_empty(&self) -> bool {
        !self.has_file_qualifiers() && !self.has_commit_qualifiers()
    }

    fn matches_commit(&self, author: &Author, committer: &Author) -> bool {
        let a = match &self.author_pattern {
            Some(pat) => pat.matches(&author.name, &author.email),
            None if !self.author_emails.is_empty() => self
                .author_emails
                .iter()
                .any(|e| e.eq_ignore_ascii_case(&author.email)),
            None => true,
        };
        let c = match &self.committer_pattern {
            Some(pat) => pat.matches(&committer.name, &committer.email),
            None if !self.committer_emails.is_empty() => self
                .committer_emails
                .iter()
                .any(|e| e.eq_ignore_ascii_case(&committer.email)),
            None => true,
        };
        a && c
    }

    fn matches_file(&self, path: &str, language: Option<&str>) -> bool {
        let lang_ok = self
            .lang
            .as_ref()
            .is_none_or(|l| language.is_some_and(|fl| l.eq_ignore_ascii_case(fl)));
        let path_ok = if self.path_pattern.is_none() && self.path_regex.is_none() {
            true
        } else {
            let normalized = path.replace('\\', "/");
            let p = self
                .path_pattern
                .as_ref()
                .is_some_and(|pat| pat.matches(&normalized));
            let r = self
                .path_regex
                .as_ref()
                .is_some_and(|re| re.is_match(&normalized));
            p || r
        };
        lang_ok && path_ok
    }
}

#[derive(Debug, Clone)]
pub struct ExcludeRule {
    pub repo: Option<String>,
    and_groups: Vec<AndGroup>,
}

impl ExcludeRule {
    pub fn parse_many(value: &str) -> anyhow::Result<Vec<ExcludeRule>> {
        let mut rules: Vec<ExcludeRule> = Vec::new();

        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                anyhow::bail!("Invalid exclude rule '{value}': empty rule component");
            }

            if is_qualifier_prefix(part) {
                let groups = parse_all_qualifiers(part, value)?;
                if let Some(rule) = rules.last_mut() {
                    rule.and_groups.extend(groups);
                } else {
                    rules.push(ExcludeRule {
                        repo: None,
                        and_groups: groups,
                    });
                }
                continue;
            }

            if let Some(username) = part.strip_prefix('@') {
                if username.trim().is_empty() {
                    anyhow::bail!(
                        "Invalid exclude rule '{value}': empty author qualifier '{part}'"
                    );
                }
                let group = AndGroup {
                    author_pattern: Some(AuthorPattern::GitHubUser(username.trim().to_string())),
                    ..Default::default()
                };
                let groups = vec![group];
                if let Some(rule) = rules.last_mut() {
                    rule.and_groups.extend(groups);
                } else {
                    rules.push(ExcludeRule {
                        repo: None,
                        and_groups: groups,
                    });
                }
                continue;
            }

            if let Some((repo_part, quals)) = part.split_once(':') {
                let repo = if repo_part.is_empty() {
                    None
                } else {
                    Some(repo_part.to_string())
                };
                let groups = parse_all_qualifiers(quals, value)?;
                rules.push(ExcludeRule {
                    repo,
                    and_groups: groups,
                });
            } else {
                rules.push(ExcludeRule {
                    repo: Some(part.to_string()),
                    and_groups: vec![],
                });
            }
        }

        Ok(rules)
    }

    pub fn matches_repo_with_mode(&self, repo_name: &str, mode: RepoMatchMode) -> bool {
        match &self.repo {
            Some(r) => {
                let repo_name = mode.repo_key(repo_name);
                let rule_name = mode.repo_key(r);
                repo_name == rule_name
                    || repo_name.starts_with(&format!("{rule_name}/"))
                    || repo_name.ends_with(&format!("/{rule_name}"))
            }
            None => true,
        }
    }

    fn matches_repo(&self, repo_name: &str) -> bool {
        self.matches_repo_with_mode(repo_name, local_repo_match_mode())
    }

    pub fn is_repo_exclusion(&self) -> bool {
        self.and_groups.is_empty() || self.and_groups.iter().all(|g| g.is_empty())
    }

    #[allow(dead_code)]
    pub fn has_lang(&self) -> bool {
        self.and_groups.iter().any(|g| g.lang.is_some())
    }

    pub fn has_path(&self) -> bool {
        self.and_groups
            .iter()
            .any(|g| g.path_pattern.is_some() || g.path_regex.is_some())
    }

    pub fn all_langs_for_repo_with_mode(
        &self,
        repo_name: &str,
        mode: RepoMatchMode,
    ) -> Vec<String> {
        if !self.matches_repo_with_mode(repo_name, mode) {
            return Vec::new();
        }
        self.and_groups
            .iter()
            .filter_map(|g| g.lang.clone())
            .collect()
    }

    #[allow(dead_code)]
    pub fn all_langs_for_repo(&self, repo_name: &str) -> Vec<String> {
        self.all_langs_for_repo_with_mode(repo_name, local_repo_match_mode())
    }

    pub fn collect_github_users(&self) -> Vec<String> {
        let mut users = Vec::new();
        for group in &self.and_groups {
            if let Some(AuthorPattern::GitHubUser(ref u)) = group.author_pattern
                && !users.contains(u)
            {
                users.push(u.clone());
            }
            if let Some(AuthorPattern::GitHubUser(ref u)) = group.committer_pattern
                && !users.contains(u)
            {
                users.push(u.clone());
            }
        }
        users
    }

    pub fn resolve_github_user(&mut self, username: &str, emails: &[String]) {
        for group in &mut self.and_groups {
            if let Some(AuthorPattern::GitHubUser(ref u)) = group.author_pattern
                && u == username
            {
                group.author_emails = emails.to_vec();
                group.author_pattern = None;
            }
            if let Some(AuthorPattern::GitHubUser(ref u)) = group.committer_pattern
                && u == username
            {
                group.committer_emails = emails.to_vec();
                group.committer_pattern = None;
            }
        }
    }
}

fn is_qualifier_prefix(s: &str) -> bool {
    s.starts_with("lang:")
        || s.starts_with("language:")
        || s.starts_with("l:")
        || s.starts_with("path:")
        || s.starts_with("p:")
        || s.starts_with("user:")
        || s.starts_with("u:")
        || s.starts_with("author:")
        || s.starts_with("a:")
        || s.starts_with("committer:")
        || s.starts_with("c:")
}

fn parse_all_qualifiers(rest: &str, source: &str) -> anyhow::Result<Vec<AndGroup>> {
    if rest.trim().is_empty() {
        anyhow::bail!("Invalid exclude rule '{source}': empty repo-less rule");
    }

    let mut groups = Vec::new();
    for or_part in rest.split(',') {
        let or_part = or_part.trim();
        if or_part.is_empty() {
            anyhow::bail!("Invalid exclude rule '{source}': empty qualifier component");
        }
        let mut group = AndGroup::default();
        for and_part in or_part.split('+') {
            let and_part = and_part.trim();
            if and_part.is_empty() {
                anyhow::bail!("Invalid exclude rule '{source}': empty '+' component");
            }
            let (key, value) = and_part.split_once(':').ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid exclude rule '{source}': qualifier '{and_part}' is missing ':'"
                )
            })?;
            let key = key.trim();
            let value = value.trim();
            if value.is_empty() {
                anyhow::bail!(
                    "Invalid exclude rule '{source}': qualifier '{and_part}' has an empty value"
                );
            }
            match key {
                "lang" | "language" | "l" => group.lang = Some(value.to_string()),
                "path" | "p" => set_path(&mut group, value)?,
                "user" | "u" => {
                    let pat = parse_author(value)?;
                    group.author_pattern = Some(pat.clone());
                    group.committer_pattern = Some(pat);
                }
                "author" | "a" => group.author_pattern = Some(parse_author(value)?),
                "committer" | "c" => group.committer_pattern = Some(parse_author(value)?),
                _ => anyhow::bail!(
                    "Invalid exclude rule '{source}': unknown qualifier '{key}' in '{and_part}'"
                ),
            }
        }
        if !group.is_empty() {
            groups.push(group);
        }
    }
    if groups.is_empty() {
        anyhow::bail!("Invalid exclude rule '{source}': empty repo-less rule");
    }
    Ok(groups)
}

fn set_path(group: &mut AndGroup, glob_str: &str) -> anyhow::Result<()> {
    group.path_pattern = Pattern::new(glob_str).ok();
    group.path_regex = glob_to_regex(glob_str)?;
    Ok(())
}

fn parse_author(pattern: &str) -> anyhow::Result<AuthorPattern> {
    if pattern.is_empty() {
        anyhow::bail!("empty author qualifier value");
    }
    if let Some(username) = pattern.strip_prefix("github:") {
        if username.is_empty() {
            anyhow::bail!("empty GitHub username in author qualifier '{pattern}'");
        }
        Ok(AuthorPattern::GitHubUser(username.to_string()))
    } else if let Some(username) = pattern.strip_prefix('@') {
        if username.is_empty() {
            anyhow::bail!("empty GitHub username in author qualifier '{pattern}'");
        }
        Ok(AuthorPattern::GitHubUser(username.to_string()))
    } else {
        Pattern::new(pattern)
            .map(AuthorPattern::Glob)
            .map_err(|e| anyhow::anyhow!("invalid author qualifier '{pattern}': {e}"))
    }
}

fn glob_to_regex(glob: &str) -> anyhow::Result<Option<Regex>> {
    if Pattern::new(glob).is_ok() {
        return Ok(None);
    }
    let mut regex_str = String::from("^");
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    regex_str.push_str(".*");
                    i += 2;
                    if i < chars.len() && chars[i] == '/' {
                        regex_str.push('/');
                        i += 1;
                    }
                    continue;
                }
                regex_str.push_str("[^/]*");
            }
            '?' => regex_str.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' | '\\' => {
                regex_str.push('\\');
                regex_str.push(chars[i]);
            }
            c => regex_str.push(c),
        }
        i += 1;
    }
    regex_str.push('$');
    Regex::new(&regex_str)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("invalid path qualifier '{glob}': {e}"))
}

pub fn filter_commits(commits: Vec<CommitStats>, rules: &[ExcludeRule]) -> Vec<CommitStats> {
    if rules.is_empty() {
        return commits;
    }

    commits
        .into_iter()
        .filter_map(|mut commit| {
            for rule in rules {
                if !rule.matches_repo(&commit.repo) {
                    continue;
                }
                if rule.is_repo_exclusion() {
                    return None;
                }
                let had_files = !commit.file_changes.is_empty();
                for group in &rule.and_groups {
                    let commit_ok = group.matches_commit(&commit.author, &commit.committer);
                    if group.has_commit_qualifiers() && !commit_ok {
                        continue;
                    }
                    if group.has_file_qualifiers() {
                        commit
                            .file_changes
                            .retain(|fc| !group.matches_file(&fc.path, fc.language.as_deref()));
                    } else if commit_ok {
                        return None;
                    }
                }
                if commit.file_changes.is_empty() && had_files {
                    return None;
                }
            }
            Some(commit)
        })
        .collect()
}

#[allow(dead_code)]
pub fn is_repo_excluded(repo_name: &str, rules: &[ExcludeRule]) -> bool {
    is_repo_excluded_with_mode(repo_name, rules, local_repo_match_mode())
}

pub fn is_repo_excluded_with_mode(
    repo_name: &str,
    rules: &[ExcludeRule],
    mode: RepoMatchMode,
) -> bool {
    rules
        .iter()
        .any(|r| r.matches_repo_with_mode(repo_name, mode) && r.is_repo_exclusion())
}

#[allow(dead_code)]
pub fn excluded_langs_for_repo(repo_name: &str, rules: &[ExcludeRule]) -> Vec<String> {
    excluded_langs_for_repo_with_mode(repo_name, rules, local_repo_match_mode())
}

pub fn excluded_langs_for_repo_with_mode(
    repo_name: &str,
    rules: &[ExcludeRule],
    mode: RepoMatchMode,
) -> Vec<String> {
    rules
        .iter()
        .flat_map(|r| r.all_langs_for_repo_with_mode(repo_name, mode))
        .collect()
}

pub fn any_path_rules(rules: &[ExcludeRule]) -> bool {
    rules.iter().any(|r| r.has_path())
}

pub fn collect_github_users(rules: &[ExcludeRule]) -> Vec<String> {
    let mut users = Vec::new();
    for rule in rules {
        for user in rule.collect_github_users() {
            if !users.contains(&user) {
                users.push(user);
            }
        }
    }
    users
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_repo() {
        let rules = ExcludeRule::parse_many("my-repo").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert!(rules[0].is_repo_exclusion());
    }

    #[test]
    fn parse_repo_with_lang() {
        let rules = ExcludeRule::parse_many("my-repo:lang:rust").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert!(rules[0].has_lang());
        let langs = rules[0].all_langs_for_repo("my-repo");
        assert_eq!(langs, vec!["rust"]);
    }

    #[test]
    fn parse_repo_with_short_lang() {
        let rules = ExcludeRule::parse_many("my-repo:l:rust").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].has_lang());
    }

    #[test]
    fn parse_repo_with_path() {
        let rules = ExcludeRule::parse_many("my-repo:path:src/**").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("my-repo"));
        assert!(rules[0].has_path());
    }

    #[test]
    fn parse_multiple_repos() {
        let rules = ExcludeRule::parse_many("repo1,repo2:lang:js").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].repo.as_deref(), Some("repo1"));
        assert!(rules[0].is_repo_exclusion());
        assert_eq!(rules[1].repo.as_deref(), Some("repo2"));
        assert!(rules[1].has_lang());
    }

    #[test]
    fn parse_global_lang() {
        let rules = ExcludeRule::parse_many(":lang:markdown").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].repo.is_none());
        assert!(rules[0].has_lang());
    }

    #[test]
    fn parse_global_path() {
        let rules = ExcludeRule::parse_many(":p:**/*.md").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].repo.is_none());
        assert!(rules[0].has_path());
    }

    #[test]
    fn parse_and_semantics() {
        let rules = ExcludeRule::parse_many("my-repo:lang:rust+path:src/**").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].and_groups.len(), 1);
        assert!(rules[0].and_groups[0].lang.is_some());
        assert!(rules[0].and_groups[0].path_pattern.is_some());
    }

    #[test]
    fn parse_or_semantics() {
        let rules = ExcludeRule::parse_many("my-repo:lang:rust,path:src/**").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].and_groups.len(), 2);
    }

    #[test]
    fn parse_author_glob() {
        let rules = ExcludeRule::parse_many(":author:*bot*").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].repo.is_none());
        assert!(rules[0].and_groups[0].author_pattern.is_some());
    }

    #[test]
    fn parse_committer_glob() {
        let rules = ExcludeRule::parse_many("repo:c:dependabot*").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo.as_deref(), Some("repo"));
        assert!(rules[0].and_groups[0].committer_pattern.is_some());
    }

    #[test]
    fn parse_author_github() {
        let rules = ExcludeRule::parse_many(":author:github:torvalds").unwrap();
        assert_eq!(rules.len(), 1);
        let pattern = &rules[0].and_groups[0].author_pattern;
        assert!(pattern.is_some());
    }

    #[test]
    fn repo_match_exact() {
        let rule = ExcludeRule {
            repo: Some("my-repo".into()),
            and_groups: vec![],
        };
        assert!(rule.matches_repo("my-repo"));
        assert!(!rule.matches_repo("other-repo"));
    }

    #[test]
    fn repo_match_prefix() {
        let rule = ExcludeRule {
            repo: Some("owner".into()),
            and_groups: vec![],
        };
        assert!(rule.matches_repo("owner/repo-name"));
        assert!(!rule.matches_repo("other/repo"));
    }

    #[test]
    fn explicit_repo_match_modes_apply_platform_and_github_rules() {
        let exact = ExcludeRule {
            repo: Some("Team/Service".into()),
            and_groups: vec![],
        };
        assert!(exact.matches_repo_with_mode(
            "team/service",
            RepoMatchMode::LocalPlatform { windows: true }
        ));
        assert!(!exact.matches_repo_with_mode(
            "team/service",
            RepoMatchMode::LocalPlatform { windows: false }
        ));
        assert!(exact.matches_repo_with_mode("team/service", RepoMatchMode::Github));

        let component = ExcludeRule {
            repo: Some("Team".into()),
            and_groups: vec![],
        };
        assert!(component.matches_repo_with_mode(
            "team/service",
            RepoMatchMode::LocalPlatform { windows: true }
        ));
        assert!(component.matches_repo_with_mode("owner/team", RepoMatchMode::Github));
    }

    #[cfg(feature = "github")]
    #[test]
    fn github_repository_language_and_repo_excludes_are_case_insensitive_on_every_host() {
        let rules = ExcludeRule::parse_many("Acme/Widget:lang:Rust,Acme/Hidden").unwrap();

        assert!(is_repo_excluded_with_mode(
            "acme/hidden",
            &rules,
            RepoMatchMode::Github
        ));
        assert_eq!(
            excluded_langs_for_repo_with_mode("ACME/WIDGET", &rules, RepoMatchMode::Github),
            vec!["Rust"]
        );
    }

    #[test]
    fn global_rule_matches_all() {
        let rule = ExcludeRule {
            repo: None,
            and_groups: vec![AndGroup {
                lang: Some("rust".into()),
                ..Default::default()
            }],
        };
        assert!(rule.matches_repo("any-repo"));
    }

    #[test]
    fn path_match_glob() {
        let mut group = AndGroup::default();
        set_path(&mut group, "src/**").unwrap();
        assert!(group.matches_file("src/main.rs", None));
        assert!(group.matches_file("src/lib/mod.rs", None));
        assert!(!group.matches_file("tests/main.rs", None));
    }

    #[test]
    fn path_match_wildcard() {
        let mut group = AndGroup::default();
        set_path(&mut group, "*.md").unwrap();
        assert!(group.matches_file("README.md", None));
        assert!(group.matches_file("docs/guide.md", None));
        assert!(!group.matches_file("src/main.rs", None));
    }

    #[test]
    fn author_glob_matches_name() {
        let pat = parse_author("*bot*").unwrap();
        assert!(pat.matches("dependabot[bot]", "bot@github.com"));
        assert!(pat.matches("ci-bot", "ci@example.com"));
        assert!(!pat.matches("alice", "alice@example.com"));
    }

    #[test]
    fn author_and_group_matches_commit() {
        let group = AndGroup {
            author_pattern: Some(parse_author("*bot*").unwrap()),
            ..Default::default()
        };
        let author = Author {
            name: "renovate[bot]".into(),
            email: "bot@renovate.com".into(),
        };
        let committer = Author {
            name: "ci".into(),
            email: "ci@example.com".into(),
        };
        assert!(group.matches_commit(&author, &committer));
    }

    #[test]
    fn author_and_group_not_matches() {
        let group = AndGroup {
            author_pattern: Some(parse_author("*bot*").unwrap()),
            ..Default::default()
        };
        let author = Author {
            name: "alice".into(),
            email: "alice@example.com".into(),
        };
        let committer = Author {
            name: "alice".into(),
            email: "alice@example.com".into(),
        };
        assert!(!group.matches_commit(&author, &committer));
    }

    #[test]
    fn parse_author_at_shorthand() {
        let rules = ExcludeRule::parse_many(":author:@torvalds").unwrap();
        assert_eq!(rules.len(), 1);
        assert!(
            matches!(&rules[0].and_groups[0].author_pattern, Some(AuthorPattern::GitHubUser(u)) if u == "torvalds")
        );
    }

    #[test]
    fn resolve_github_user_replaces_pattern_with_emails() {
        let mut rule = ExcludeRule {
            repo: None,
            and_groups: vec![AndGroup {
                author_pattern: Some(AuthorPattern::GitHubUser("testuser".into())),
                ..Default::default()
            }],
        };
        rule.resolve_github_user("testuser", &["a@b.com".into(), "c@d.com".into()]);
        assert!(rule.and_groups[0].author_pattern.is_none());
        assert_eq!(rule.and_groups[0].author_emails.len(), 2);
    }

    #[test]
    fn author_emails_match_exact() {
        let group = AndGroup {
            author_emails: vec!["bot@example.com".into()],
            ..Default::default()
        };
        let author = Author {
            name: "Bot".into(),
            email: "bot@example.com".into(),
        };
        let committer = Author {
            name: "ci".into(),
            email: "ci@example.com".into(),
        };
        assert!(group.matches_commit(&author, &committer));
    }

    #[test]
    fn author_emails_case_insensitive() {
        let group = AndGroup {
            author_emails: vec!["Bot@Example.com".into()],
            ..Default::default()
        };
        let author = Author {
            name: "bot".into(),
            email: "bot@example.com".into(),
        };
        let committer = Author {
            name: "ci".into(),
            email: "ci@example.com".into(),
        };
        assert!(group.matches_commit(&author, &committer));
    }

    #[test]
    fn unknown_or_empty_exclude_qualifiers_are_errors() {
        for value in [
            "repo:nope:value",
            "repo:lang:",
            "repo:author:",
            "repo:lang:rust+",
        ] {
            assert!(
                ExcludeRule::parse_many(value).is_err(),
                "expected '{value}' to be rejected"
            );
        }
    }
}
