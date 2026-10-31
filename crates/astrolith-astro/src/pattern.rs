//! Fixture-verified `glob()` collection patterns.
//!
//! Astro passes `pattern` to `tinyglobby`, which accepts a string or an array
//! of strings. Array entries beginning with `!` exclude matching paths; every
//! exclusion wins regardless of ordering. This module accepts only the
//! Markdown/MDX include patterns verified by the fixtures and a bounded glob
//! subset for exclusions, so discovery never guesses about selecting sources.

use std::fmt;
use std::path::{Component, Path};

const MAX_PATTERN_BYTES: usize = 1024;
const MAX_PATTERN_SEGMENTS: usize = 128;
const MAX_BRACE_EXPANSIONS: usize = 64;

/// A validated glob pattern set from a `glob()` loader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionPattern {
    declared: Vec<String>,
    includes_markdown: bool,
    includes_mdx: bool,
    excludes: Vec<GlobPattern>,
}

impl CollectionPattern {
    /// Validate the declared string or array entries.
    pub fn new(declared: Vec<String>) -> Result<Self, String> {
        if declared.is_empty() {
            return Err("glob loader `pattern` must not be empty".into());
        }
        let mut includes_markdown = false;
        let mut includes_mdx = false;
        let mut excludes = Vec::new();

        for entry in &declared {
            if entry.len() > MAX_PATTERN_BYTES {
                return Err(format!("glob pattern exceeds {MAX_PATTERN_BYTES} bytes"));
            }
            if entry.is_empty() {
                return Err("glob pattern entries must not be empty".into());
            }
            if let Some(exclusion) = entry.strip_prefix('!') {
                if exclusion.is_empty() || exclusion.starts_with('!') {
                    return Err(format!("unsupported glob exclusion `{entry}`"));
                }
                for expanded in expand_braces(exclusion)? {
                    excludes.push(GlobPattern::parse(&expanded)?);
                }
                continue;
            }
            match entry.as_str() {
                "**/*.md" => includes_markdown = true,
                "**/*.mdx" => includes_mdx = true,
                "**/*.{md,mdx}" => {
                    includes_markdown = true;
                    includes_mdx = true;
                }
                _ => return Err(format!("unsupported glob pattern `{entry}`")),
            }
        }
        if !includes_markdown && !includes_mdx {
            return Err(
                "glob loader `pattern` requires at least one supported Markdown include".into(),
            );
        }

        Ok(Self {
            declared,
            includes_markdown,
            includes_mdx,
            excludes,
        })
    }

    /// Return the declared entries exactly as written in the content config.
    pub fn declared(&self) -> &[String] {
        &self.declared
    }

    /// Whether the include patterns select MDX sources.
    pub fn includes_mdx(&self) -> bool {
        self.includes_mdx
    }

    /// Whether a path relative to the collection base is selected.
    pub fn matches(&self, relative: &Path) -> bool {
        let Some(extension) = relative.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        let included = match extension {
            "md" => self.includes_markdown,
            "mdx" => self.includes_mdx,
            _ => false,
        };
        if !included {
            return false;
        }
        let Some(segments) = path_segments(relative) else {
            return false;
        };
        !self
            .excludes
            .iter()
            .any(|exclude| exclude.matches(&segments))
    }
}

impl fmt::Display for CollectionPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[")?;
        for (index, entry) in self.declared.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{entry:?}")?;
        }
        formatter.write_str("]")
    }
}

fn path_segments(path: &Path) -> Option<Vec<String>> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GlobPattern {
    segments: Vec<GlobSegment>,
}

impl GlobPattern {
    fn parse(pattern: &str) -> Result<Self, String> {
        validate_relative_glob(pattern)?;
        let segments = pattern
            .split('/')
            .map(GlobSegment::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if segments.len() > MAX_PATTERN_SEGMENTS {
            return Err(format!(
                "glob pattern has more than {MAX_PATTERN_SEGMENTS} segments"
            ));
        }
        Ok(Self { segments })
    }

    fn matches(&self, path: &[String]) -> bool {
        let pattern = &self.segments;
        let mut next = vec![false; path.len() + 1];
        next[path.len()] = true;
        for segment in pattern.iter().rev() {
            let mut current = vec![false; path.len() + 1];
            for index in (0..=path.len()).rev() {
                current[index] = match segment {
                    GlobSegment::GlobStar => {
                        next[index] || (index < path.len() && current[index + 1])
                    }
                    GlobSegment::Tokens(tokens) => {
                        index < path.len() && tokens_match(tokens, &path[index]) && next[index + 1]
                    }
                };
            }
            next = current;
        }
        next[0]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GlobSegment {
    GlobStar,
    Tokens(Vec<GlobToken>),
}

impl GlobSegment {
    fn parse(segment: &str) -> Result<Self, String> {
        if segment.is_empty() {
            return Err("glob patterns must not contain empty segments".into());
        }
        if segment == "**" {
            return Ok(Self::GlobStar);
        }
        let mut tokens = Vec::new();
        let characters: Vec<char> = segment.chars().collect();
        let mut index = 0;
        while index < characters.len() {
            match characters[index] {
                '*' => {
                    if characters.get(index + 1) == Some(&'*') {
                        return Err("`**` must form a complete glob path segment".into());
                    }
                    tokens.push(GlobToken::Star);
                    index += 1;
                }
                '?' => {
                    tokens.push(GlobToken::AnyChar);
                    index += 1;
                }
                '[' => {
                    let (class, consumed) = parse_class(&characters[index..])?;
                    tokens.push(GlobToken::Class(class));
                    index += consumed;
                }
                '\\' | '{' | '}' | ']' => {
                    return Err(format!(
                        "unsupported character `{}` in glob pattern",
                        characters[index]
                    ));
                }
                literal => {
                    tokens.push(GlobToken::Literal(literal));
                    index += 1;
                }
            }
        }
        Ok(Self::Tokens(tokens))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GlobToken {
    Literal(char),
    AnyChar,
    Star,
    Class(CharClass),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CharClass {
    negated: bool,
    ranges: Vec<(char, char)>,
}

fn parse_class(characters: &[char]) -> Result<(CharClass, usize), String> {
    let closing = characters
        .iter()
        .position(|character| *character == ']')
        .ok_or_else(|| "unterminated character class in glob pattern".to_owned())?;
    if closing < 2 {
        return Err("empty character class in glob pattern".into());
    }
    let mut index = 1;
    let negated = matches!(characters[index], '!' | '^');
    if negated {
        index += 1;
    }
    let mut ranges = Vec::new();
    while index < closing {
        let start = characters[index];
        if index + 2 < closing && characters[index + 1] == '-' {
            let end = characters[index + 2];
            if end < start {
                return Err("descending character class range in glob pattern".into());
            }
            ranges.push((start, end));
            index += 3;
        } else {
            ranges.push((start, start));
            index += 1;
        }
    }
    if ranges.is_empty() {
        return Err("empty character class in glob pattern".into());
    }
    Ok((CharClass { negated, ranges }, closing + 1))
}

fn tokens_match(tokens: &[GlobToken], text: &str) -> bool {
    let characters: Vec<char> = text.chars().collect();
    let mut next = vec![false; characters.len() + 1];
    next[0] = true;
    for token in tokens {
        let mut current = vec![false; characters.len() + 1];
        match token {
            GlobToken::Star => {
                current[0] = next[0];
                for index in 0..characters.len() {
                    current[index + 1] = next[index + 1] || current[index];
                }
            }
            GlobToken::AnyChar => {
                current[1..].copy_from_slice(&next[..characters.len()]);
            }
            GlobToken::Literal(expected) => {
                for (index, actual) in characters.iter().enumerate() {
                    current[index + 1] = next[index] && actual == expected;
                }
            }
            GlobToken::Class(class) => {
                for (index, actual) in characters.iter().enumerate() {
                    current[index + 1] = next[index] && class.matches(*actual);
                }
            }
        }
        next = current;
    }
    next[characters.len()]
}

impl CharClass {
    fn matches(&self, character: char) -> bool {
        let inside = self
            .ranges
            .iter()
            .any(|(start, end)| start <= &character && &character <= end);
        inside != self.negated
    }
}

fn validate_relative_glob(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("glob exclusion must not be empty".into());
    }
    if pattern.starts_with(['/', '\\', '!'])
        || pattern
            .as_bytes()
            .get(1)
            .is_some_and(|second| pattern.as_bytes()[0].is_ascii_alphabetic() && *second == b':')
    {
        return Err(format!("glob exclusion `{pattern}` must be relative"));
    }
    if pattern.split('/').any(|segment| segment == "..") {
        return Err(format!(
            "glob exclusion `{pattern}` must not traverse parents"
        ));
    }
    if pattern.contains('\\') {
        return Err(format!(
            "glob exclusion `{pattern}` must use `/` separators"
        ));
    }
    Ok(())
}

fn expand_braces(pattern: &str) -> Result<Vec<String>, String> {
    let Some(open) = pattern.find('{') else {
        return Ok(vec![pattern.to_owned()]);
    };
    let close = pattern[open + 1..]
        .find('}')
        .map(|offset| open + 1 + offset)
        .ok_or_else(|| format!("unterminated brace expansion in glob exclusion `{pattern}`"))?;
    if pattern[close + 1..].contains(['{', '}']) {
        return Err(format!(
            "nested brace expansions are unsupported in glob exclusion `{pattern}`"
        ));
    }
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let body = &pattern[open + 1..close];
    let mut expanded = Vec::new();
    for alternative in body.split(',') {
        if alternative.is_empty() {
            return Err(format!("empty brace alternative in `{pattern}`"));
        }
        for value in expand_braces(&format!("{prefix}{alternative}{suffix}"))? {
            if expanded.len() >= MAX_BRACE_EXPANSIONS {
                return Err(format!(
                    "glob exclusion expands to more than {MAX_BRACE_EXPANSIONS} patterns"
                ));
            }
            expanded.push(value);
        }
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::CollectionPattern;
    use std::path::Path;

    fn pattern(entries: &[&str]) -> CollectionPattern {
        CollectionPattern::new(entries.iter().map(|entry| (*entry).to_owned()).collect()).unwrap()
    }

    #[test]
    fn selects_markdown_and_mdx_by_declared_extension() {
        let markdown = pattern(&["**/*.md"]);
        assert!(markdown.matches(Path::new("root.md")));
        assert!(markdown.matches(Path::new("nested/entry.md")));
        assert!(!markdown.matches(Path::new("nested/entry.mdx")));

        let both = pattern(&["**/*.{md,mdx}"]);
        assert!(both.matches(Path::new("nested/entry.mdx")));
        assert!(both.includes_mdx());
    }

    #[test]
    fn negative_patterns_win_regardless_of_declared_order() {
        let excluded = pattern(&["**/*.{md,mdx}", "!**/drafts/**"]);
        assert!(excluded.matches(Path::new("guides/entry.md")));
        assert!(!excluded.matches(Path::new("drafts/entry.md")));
        assert!(!excluded.matches(Path::new("drafts/nested/entry.mdx")));

        let reintroduced = pattern(&["!**/drafts/**", "**/*.md"]);
        assert!(!reintroduced.matches(Path::new("drafts/entry.md")));
    }

    #[test]
    fn excludes_root_files_character_classes_and_brace_alternatives() {
        let selected = pattern(&[
            "**/*.md",
            "!**/introduction.md",
            "!**/chapter-[0-9].md",
            "!**/{draft,template}.md",
        ]);
        assert!(selected.matches(Path::new("guides/entry.md")));
        assert!(!selected.matches(Path::new("introduction.md")));
        assert!(!selected.matches(Path::new("guides/chapter-3.md")));
        assert!(!selected.matches(Path::new("draft.md")));
        assert!(!selected.matches(Path::new("nested/template.md")));
        assert!(selected.matches(Path::new("chapter-x.md")));
    }

    #[test]
    fn rejects_unsupported_or_escaping_patterns() {
        for entries in [
            Vec::<String>::new(),
            vec!["**/*.json".into()],
            vec!["!**/drafts/**".into()],
            vec!["**/*.md".into(), "!../outside/**".into()],
            vec!["**/*.md".into(), "!/absolute/**".into()],
            vec!["**/*.md".into(), "!**/a**b.md".into()],
            vec!["**/*.md".into(), "!**/{a".into()],
        ] {
            assert!(
                CollectionPattern::new(entries.clone()).is_err(),
                "accepted {entries:?}"
            );
        }
    }
}
