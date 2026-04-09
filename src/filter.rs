use std::collections::{HashMap, VecDeque};

use crate::stats::models::CommitStats;

// ── AST ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum MeAtom {
    GitHub(String),
    Name(String),
    Email(String),
}

#[derive(Debug, Clone)]
pub enum MeExpr {
    Atom(MeAtom),
    And(Box<MeExpr>, Box<MeExpr>),
    Or(Box<MeExpr>, Box<MeExpr>),
}

// ── Tokenizer ────────────────────────────────────────────────────────

#[derive(Debug)]
enum Token {
    Or,
    And,
    LParen,
    RParen,
    Atom(MeAtom),
}

fn tokenize(input: &str) -> anyhow::Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut buf = String::new();

    for ch in input.chars() {
        match ch {
            '(' | ')' | '|' | '&' => {
                flush_buf(&mut buf, &mut tokens)?;
                tokens.push(match ch {
                    '(' => Token::LParen,
                    ')' => Token::RParen,
                    '|' => Token::Or,
                    '&' => Token::And,
                    _ => unreachable!(),
                });
            }
            _ => buf.push(ch),
        }
    }
    flush_buf(&mut buf, &mut tokens)?;
    Ok(tokens)
}

fn flush_buf(buf: &mut String, tokens: &mut Vec<Token>) -> anyhow::Result<()> {
    let trimmed = buf.trim();
    if !trimmed.is_empty() {
        tokens.push(Token::Atom(parse_atom(trimmed)));
    }
    buf.clear();
    Ok(())
}

fn parse_atom(s: &str) -> MeAtom {
    if let Some(rest) = s.strip_prefix("github:") {
        MeAtom::GitHub(rest.trim().to_string())
    } else if let Some(rest) = s.strip_prefix("name:") {
        MeAtom::Name(rest.trim().to_string())
    } else if let Some(rest) = s.strip_prefix("email:") {
        MeAtom::Email(rest.trim().to_string())
    } else {
        // Bare identifier defaults to name
        MeAtom::Name(s.to_string())
    }
}

// ── Recursive Descent Parser ─────────────────────────────────────────
// Grammar:
//   expr   = term ("|" term)*
//   term   = factor ("&" factor)*
//   factor = "(" expr ")" | atom

struct Parser {
    tokens: VecDeque<Token>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens: tokens.into(),
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.front()
    }

    fn next_token(&mut self) -> Option<Token> {
        self.tokens.pop_front()
    }

    fn parse_expr(&mut self) -> anyhow::Result<MeExpr> {
        let mut left = self.parse_term()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.next_token();
            let right = self.parse_term()?;
            left = MeExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> anyhow::Result<MeExpr> {
        let mut left = self.parse_factor()?;
        while matches!(self.peek(), Some(Token::And)) {
            self.next_token();
            let right = self.parse_factor()?;
            left = MeExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> anyhow::Result<MeExpr> {
        match self.peek() {
            Some(Token::LParen) => {
                self.next_token();
                let expr = self.parse_expr()?;
                match self.next_token() {
                    Some(Token::RParen) => Ok(expr),
                    _ => anyhow::bail!("Expected ')' in --me expression"),
                }
            }
            Some(Token::Atom(_)) => {
                let token = self.next_token().expect("peeked Some");
                match token {
                    Token::Atom(atom) => Ok(MeExpr::Atom(atom)),
                    _ => unreachable!(),
                }
            }
            other => anyhow::bail!(
                "Expected identifier or '(' in --me expression, got {:?}",
                other
                    .map(|t| format!("{t:?}"))
                    .unwrap_or("end of input".into())
            ),
        }
    }
}

pub fn parse_me_expr(input: &str) -> anyhow::Result<MeExpr> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        anyhow::bail!("Empty --me expression");
    }
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;
    if !parser.tokens.is_empty() {
        anyhow::bail!("Unexpected trailing tokens in --me expression");
    }
    Ok(expr)
}

// ── Evaluator ────────────────────────────────────────────────────────

impl MeExpr {
    pub fn matches_commit(
        &self,
        commit: &CommitStats,
        identity_map: &HashMap<String, String>,
    ) -> bool {
        match self {
            MeExpr::Atom(atom) => atom.matches_commit(commit, identity_map),
            MeExpr::And(a, b) => {
                a.matches_commit(commit, identity_map) && b.matches_commit(commit, identity_map)
            }
            MeExpr::Or(a, b) => {
                a.matches_commit(commit, identity_map) || b.matches_commit(commit, identity_map)
            }
        }
    }
}

impl MeAtom {
    fn matches_commit(&self, commit: &CommitStats, identity_map: &HashMap<String, String>) -> bool {
        match self {
            MeAtom::Name(name) => {
                let pat = name.to_lowercase();
                commit.author.name.to_lowercase().contains(&pat)
                    || commit
                        .co_authors
                        .iter()
                        .any(|a| a.name.to_lowercase().contains(&pat))
            }
            MeAtom::Email(email) => {
                let pat = email.to_lowercase();
                commit.author.email.to_lowercase().contains(&pat)
                    || commit
                        .co_authors
                        .iter()
                        .any(|a| a.email.to_lowercase().contains(&pat))
            }
            MeAtom::GitHub(username) => {
                let uname = username.to_lowercase();
                // Check author
                if is_github_match(&commit.author.email, &uname, identity_map) {
                    return true;
                }
                // Check co-authors
                commit
                    .co_authors
                    .iter()
                    .any(|a| is_github_match(&a.email, &uname, identity_map))
            }
        }
    }
}

fn is_github_match(
    email: &str,
    username_lower: &str,
    identity_map: &HashMap<String, String>,
) -> bool {
    // 1. Check identity map (email → github login)
    if let Some(login) = identity_map.get(email)
        && login.to_lowercase() == username_lower
    {
        return true;
    }
    // 2. Check noreply pattern: {id}+{username}@users.noreply.github.com
    //    or {username}@users.noreply.github.com
    let email_lower = email.to_lowercase();
    if email_lower.ends_with("@users.noreply.github.com") {
        let local = email_lower.split('@').next().unwrap_or("");
        let extracted = local.split_once('+').map(|(_, u)| u).unwrap_or(local);
        return extracted == username_lower;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::models::Author;

    fn make_commit(name: &str, email: &str) -> CommitStats {
        CommitStats {
            repo: "test".to_string(),
            oid: "abc123".to_string(),
            author: Author {
                name: name.to_string(),
                email: email.to_string(),
            },
            committer: Author {
                name: name.to_string(),
                email: email.to_string(),
            },
            co_authors: vec![],
            timestamp: chrono::Utc::now(),
            message_subject: "test commit".to_string(),
            file_changes: vec![],
        }
    }

    fn make_commit_with_co(name: &str, email: &str, co_name: &str, co_email: &str) -> CommitStats {
        let mut c = make_commit(name, email);
        c.co_authors.push(Author {
            name: co_name.to_string(),
            email: co_email.to_string(),
        });
        c
    }

    // ── Parser tests ─────────────────────────────────────────────

    #[test]
    fn parse_simple_name() {
        let expr = parse_me_expr("Hugefiver").unwrap();
        assert!(matches!(expr, MeExpr::Atom(MeAtom::Name(n)) if n == "Hugefiver"));
    }

    #[test]
    fn parse_prefixed_name() {
        let expr = parse_me_expr("name:Alice").unwrap();
        assert!(matches!(expr, MeExpr::Atom(MeAtom::Name(n)) if n == "Alice"));
    }

    #[test]
    fn parse_email() {
        let expr = parse_me_expr("email:foo@bar.com").unwrap();
        assert!(matches!(expr, MeExpr::Atom(MeAtom::Email(e)) if e == "foo@bar.com"));
    }

    #[test]
    fn parse_github() {
        let expr = parse_me_expr("github:octocat").unwrap();
        assert!(matches!(expr, MeExpr::Atom(MeAtom::GitHub(u)) if u == "octocat"));
    }

    #[test]
    fn parse_or() {
        let expr = parse_me_expr("Alice|Bob").unwrap();
        assert!(matches!(expr, MeExpr::Or(_, _)));
    }

    #[test]
    fn parse_and() {
        let expr = parse_me_expr("Alice&email:x@y.com").unwrap();
        assert!(matches!(expr, MeExpr::And(_, _)));
    }

    #[test]
    fn parse_parens() {
        let expr = parse_me_expr("(Alice|Bob)&email:x@y.com").unwrap();
        assert!(matches!(expr, MeExpr::And(_, _)));
    }

    #[test]
    fn parse_empty_errors() {
        assert!(parse_me_expr("").is_err());
    }

    #[test]
    fn parse_unmatched_paren_errors() {
        assert!(parse_me_expr("(Alice|Bob").is_err());
    }

    // ── Evaluator tests ──────────────────────────────────────────

    #[test]
    fn match_name_case_insensitive() {
        let expr = parse_me_expr("hugefiver").unwrap();
        let commit = make_commit("Hugefiver", "i@iruri.moe");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_name_partial() {
        let expr = parse_me_expr("name:huge").unwrap();
        let commit = make_commit("Hugefiver", "i@iruri.moe");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_email() {
        let expr = parse_me_expr("email:iruri.moe").unwrap();
        let commit = make_commit("Hugefiver", "i@iruri.moe");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_email_no_match() {
        let expr = parse_me_expr("email:other@example.com").unwrap();
        let commit = make_commit("Hugefiver", "i@iruri.moe");
        assert!(!expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_github_noreply() {
        let expr = parse_me_expr("github:hugefiver").unwrap();
        let commit = make_commit("Hugefiver", "18693500+hugefiver@users.noreply.github.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_github_identity_map() {
        let expr = parse_me_expr("github:hugefiver").unwrap();
        let commit = make_commit("Hugefiver", "i@iruri.moe");
        let mut map = HashMap::new();
        map.insert("i@iruri.moe".to_string(), "hugefiver".to_string());
        assert!(expr.matches_commit(&commit, &map));
    }

    #[test]
    fn match_github_no_match() {
        let expr = parse_me_expr("github:someone_else").unwrap();
        let commit = make_commit("Hugefiver", "i@iruri.moe");
        assert!(!expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_or_first_matches() {
        let expr = parse_me_expr("Alice|Bob").unwrap();
        let commit = make_commit("Alice", "alice@example.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_or_second_matches() {
        let expr = parse_me_expr("Alice|Bob").unwrap();
        let commit = make_commit("Bob", "bob@example.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_or_neither_matches() {
        let expr = parse_me_expr("Alice|Bob").unwrap();
        let commit = make_commit("Charlie", "charlie@example.com");
        assert!(!expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_and_both_match() {
        let expr = parse_me_expr("name:Alice&email:alice@example.com").unwrap();
        let commit = make_commit("Alice", "alice@example.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_and_one_fails() {
        let expr = parse_me_expr("name:Alice&email:other@example.com").unwrap();
        let commit = make_commit("Alice", "alice@example.com");
        assert!(!expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_co_author_name() {
        let expr = parse_me_expr("Charlie").unwrap();
        let commit = make_commit_with_co("Alice", "a@x.com", "Charlie", "c@x.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_co_author_email() {
        let expr = parse_me_expr("email:c@x.com").unwrap();
        let commit = make_commit_with_co("Alice", "a@x.com", "Charlie", "c@x.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_complex_expr() {
        // (github:hugefiver|name:Anthony Hoo)&email:iruri
        // This would NOT match because AND requires both sides
        let expr = parse_me_expr("(github:hugefiver|name:Anthony Hoo)&email:iruri").unwrap();
        let commit = make_commit("Hugefiver", "18693500+hugefiver@users.noreply.github.com");
        // github:hugefiver matches, but email:iruri does not match noreply email
        assert!(!expr.matches_commit(&commit, &HashMap::new()));
    }

    #[test]
    fn match_complex_expr_with_or() {
        let expr = parse_me_expr("github:hugefiver|email:i@iruri.moe").unwrap();
        let commit1 = make_commit("Hugefiver", "18693500+hugefiver@users.noreply.github.com");
        let commit2 = make_commit("Hugefiver", "i@iruri.moe");
        assert!(expr.matches_commit(&commit1, &HashMap::new()));
        assert!(expr.matches_commit(&commit2, &HashMap::new()));
    }

    #[test]
    fn precedence_and_binds_tighter() {
        // "A|B&C" = "A|(B&C)" — A matches alone
        let expr = parse_me_expr("Alice|Bob&email:bob@x.com").unwrap();
        let commit = make_commit("Alice", "alice@example.com");
        assert!(expr.matches_commit(&commit, &HashMap::new()));
    }
}
