use std::collections::{BTreeSet, HashMap};

use crate::config::RoadmapConfig;
use crate::error::Result;
use crate::model::Roadmap;

#[derive(Debug, Clone, Default)]
pub struct BodyRefResolver {
    refs_by_id: HashMap<String, String>,
    full_repo_by_alias: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutcome {
    pub body: String,
    pub unresolved: Vec<String>,
}

impl BodyRefResolver {
    pub fn from_roadmap(roadmap: &Roadmap, config: &RoadmapConfig) -> Result<Self> {
        let mut refs_by_id = HashMap::new();
        let mut full_repo_by_alias = HashMap::new();
        let github = config.github()?;
        let owner = github.owner.clone();
        let hub_full_repo = config.hub_full_repo()?;
        full_repo_by_alias.insert(github.hub_repo.clone(), hub_full_repo.clone());
        for (repo_id, slug) in &github.repos {
            full_repo_by_alias.insert(repo_id.clone(), format!("{owner}/{slug}"));
            full_repo_by_alias.insert(slug.clone(), format!("{owner}/{slug}"));
        }

        for milestone in &roadmap.milestones {
            if let Some(number) = milestone.github_number() {
                refs_by_id.insert(milestone.id().to_string(), format!("{hub_full_repo}#{number}"));
            }
        }
        for issue in &roadmap.issues {
            if let Some(number) = issue.github_number() {
                let repo = config.full_repo(&issue.file.frontmatter.repo)?;
                refs_by_id.insert(issue.id().to_string(), format!("{repo}#{number}"));
            }
        }

        Ok(Self { refs_by_id, full_repo_by_alias })
    }

    pub fn insert(&mut self, id: &str, github_ref: String) {
        self.refs_by_id.insert(id.to_string(), github_ref);
    }

    pub fn resolve(&self, body: &str) -> ResolveOutcome {
        let mut out = String::with_capacity(body.len());
        let mut unresolved: BTreeSet<String> = BTreeSet::new();

        let chars: Vec<(usize, char)> = body.char_indices().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            let (start_byte, ch) = chars[idx];
            if ch != '%' {
                out.push(ch);
                idx += 1;
                continue;
            }
            let token_start_char = idx + 1;
            let mut end_char = token_start_char;
            while end_char < chars.len() && chars[end_char].1 != '%' {
                end_char += 1;
            }
            if end_char >= chars.len() {
                out.push('%');
                idx += 1;
                continue;
            }

            let token_start_byte = if token_start_char < chars.len() {
                chars[token_start_char].0
            } else {
                start_byte + ch.len_utf8()
            };
            let token_end_byte = chars[end_char].0;
            let token = &body[token_start_byte..token_end_byte];
            if is_valid_local_ref_token(token) {
                if let Some(github_ref) = self.refs_by_id.get(token) {
                    out.push_str(github_ref);
                } else {
                    unresolved.insert(token.to_string());
                    out.push('%');
                    out.push_str(token);
                    out.push('%');
                }
                idx = end_char + 1;
                continue;
            }

            out.push('%');
            out.push_str(token);
            out.push('%');
            idx = end_char + 1;
        }

        ResolveOutcome {
            body: normalize_github_ticket_refs(&out, &self.full_repo_by_alias),
            unresolved: unresolved.into_iter().collect(),
        }
    }
}

fn is_valid_local_ref_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    token
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn find_backticked_github_refs(body: &str) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for token in backticked_tokens(body) {
        if is_valid_github_ref_token(token) {
            refs.insert(token.to_string());
        }
    }
    refs.into_iter().collect()
}

pub fn find_unqualified_github_refs(body: &str) -> Vec<String> {
    let refs = collect_github_ref_tokens(body);
    let mut out = BTreeSet::new();
    for reference in refs {
        let Some((repo, _number)) = reference.split_once('#') else {
            continue;
        };
        if !repo.contains('/') {
            out.insert(reference);
        }
    }
    out.into_iter().collect()
}

fn normalize_github_ticket_refs(body: &str, full_repo_by_alias: &HashMap<String, String>) -> String {
    let unquoted = unquote_backticked_github_refs(body, full_repo_by_alias);
    rewrite_plain_github_refs(&unquoted, full_repo_by_alias)
}

fn unquote_backticked_github_refs(body: &str, full_repo_by_alias: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut idx = 0usize;

    while idx < chars.len() {
        let (_, ch) = chars[idx];
        if ch != '`' {
            out.push(ch);
            idx += 1;
            continue;
        }

        let token_start_char = idx + 1;
        let mut end_char = token_start_char;
        while end_char < chars.len() && chars[end_char].1 != '`' {
            end_char += 1;
        }
        if end_char >= chars.len() {
            out.push('`');
            idx += 1;
            continue;
        }

        let token_start_byte = if token_start_char < chars.len() {
            chars[token_start_char].0
        } else {
            body.len()
        };
        let token_end_byte = chars[end_char].0;
        let token = &body[token_start_byte..token_end_byte];
        if is_valid_github_ref_token(token) {
            out.push_str(&normalize_ref_token(token, full_repo_by_alias));
        } else {
            out.push('`');
            out.push_str(token);
            out.push('`');
        }
        idx = end_char + 1;
    }

    out
}

fn rewrite_plain_github_refs(body: &str, full_repo_by_alias: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut idx = 0usize;

    while idx < chars.len() {
        let (start_byte, ch) = chars[idx];
        if !is_ref_char(ch) {
            out.push(ch);
            idx += 1;
            continue;
        }

        let mut end = idx;
        while end < chars.len() && is_ref_char(chars[end].1) {
            end += 1;
        }
        let token = &body[start_byte..byte_at_or_end(&chars, end, body.len())];
        if is_valid_github_ref_token(token) {
            out.push_str(&normalize_ref_token(token, full_repo_by_alias));
        } else {
            out.push_str(token);
        }
        idx = end;
    }

    out
}

fn backticked_tokens(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut idx = 0usize;
    while idx < chars.len() {
        let (_, ch) = chars[idx];
        if ch != '`' {
            idx += 1;
            continue;
        }
        let token_start_char = idx + 1;
        let mut end_char = token_start_char;
        while end_char < chars.len() && chars[end_char].1 != '`' {
            end_char += 1;
        }
        if end_char >= chars.len() {
            idx += 1;
            continue;
        }
        let token_start_byte = if token_start_char < chars.len() {
            chars[token_start_char].0
        } else {
            body.len()
        };
        let token_end_byte = chars[end_char].0;
        out.push(&body[token_start_byte..token_end_byte]);
        idx = end_char + 1;
    }
    out
}

fn collect_github_ref_tokens(body: &str) -> Vec<String> {
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut idx = 0usize;
    let mut refs = BTreeSet::new();

    while idx < chars.len() {
        let (start_byte, ch) = chars[idx];
        if !is_ref_char(ch) {
            idx += 1;
            continue;
        }
        let mut end = idx;
        while end < chars.len() && is_ref_char(chars[end].1) {
            end += 1;
        }
        let token = &body[start_byte..byte_at_or_end(&chars, end, body.len())];
        if is_valid_github_ref_token(token) {
            refs.insert(token.to_string());
        }
        idx = end;
    }

    refs.into_iter().collect()
}

fn is_valid_github_ref_token(token: &str) -> bool {
    let Some((repo, number)) = token.split_once('#') else {
        return false;
    };
    if repo.is_empty() || number.is_empty() {
        return false;
    }
    if !repo.chars().all(|c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '/'
    }) {
        return false;
    }
    number.chars().all(|c| c.is_ascii_digit())
}

fn normalize_ref_token(token: &str, full_repo_by_alias: &HashMap<String, String>) -> String {
    let Some((repo, number)) = token.split_once('#') else {
        return token.to_string();
    };
    if repo.contains('/') {
        return token.to_string();
    }
    if let Some(full_repo) = full_repo_by_alias.get(repo) {
        return format!("{full_repo}#{number}");
    }
    token.to_string()
}

fn is_ref_char(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_' || ch == '/' || ch == '#'
}

fn byte_at_or_end(chars: &[(usize, char)], idx: usize, end: usize) -> usize {
    if idx < chars.len() {
        chars[idx].0
    } else {
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_local_refs() {
        let mut resolver = BodyRefResolver::default();
        resolver.insert("app-foo", "app#12".into());
        let outcome = resolver.resolve("Depends on %app-foo%.");
        assert_eq!(outcome.body, "Depends on app#12.");
        assert!(outcome.unresolved.is_empty());
    }

    #[test]
    fn preserves_unknown_local_refs() {
        let resolver = BodyRefResolver::default();
        let outcome = resolver.resolve("Depends on %app-bar%.");
        assert_eq!(outcome.body, "Depends on %app-bar%.");
        assert_eq!(outcome.unresolved, vec!["app-bar".to_string()]);
    }

    #[test]
    fn ignores_invalid_tokens() {
        let resolver = BodyRefResolver::default();
        let outcome = resolver.resolve("Ignore %not valid% and %%.");
        assert_eq!(outcome.body, "Ignore %not valid% and %%.");
        assert!(outcome.unresolved.is_empty());
    }

    #[test]
    fn unquotes_backticked_github_refs() {
        let resolver = BodyRefResolver::default();
        let outcome = resolver.resolve("Depends on (`app#43`) and `platform#95`.");
        assert_eq!(outcome.body, "Depends on (app#43) and platform#95.");
    }

    #[test]
    fn finds_backticked_github_refs() {
        let refs = find_backticked_github_refs("See `app#43`, `x`, and `platform#95`.");
        assert_eq!(refs, vec!["app#43".to_string(), "platform#95".to_string()]);
    }

    #[test]
    fn normalizes_to_full_repo_syntax() {
        let mut resolver = BodyRefResolver::default();
        resolver.full_repo_by_alias.insert("platform".to_string(), "example/platform".to_string());
        resolver.full_repo_by_alias.insert("app".to_string(), "example/app".to_string());
        let outcome = resolver.resolve("Depends on platform#95 and `app#43`.");
        assert_eq!(outcome.body, "Depends on example/platform#95 and example/app#43.");
    }

    #[test]
    fn finds_unqualified_refs() {
        let refs = find_unqualified_github_refs(
            "Use app#43 and example/platform#95 plus `platform#94`.",
        );
        assert_eq!(refs, vec!["app#43".to_string(), "platform#94".to_string()]);
    }
}
