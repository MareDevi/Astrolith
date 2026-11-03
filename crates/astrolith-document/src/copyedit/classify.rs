//! Ordered lexicon and suffix/context rules for the five Copyedit categories.
//!
//! This is a bounded, deterministic approximation of English parts of speech,
//! not a linguistic model. A word is classified only when a compact lexicon or
//! a conservative suffix/context rule matches; everything else stays
//! undecorated. The exact category does not need to agree with a third-party
//! NLP library, only with this crate's documented rule order.

use std::cmp::Ordering;

use astrolith_core::preferences::CopyeditCategory;

use super::CopyeditSpan;
use super::tokenize::{excluded_ranges, is_excluded, word_ranges};

/// Classify every eligible word in visible writer text.
pub(crate) fn classify(text: &str) -> Vec<CopyeditSpan> {
    let excluded = excluded_ranges(text);
    let tokens: Vec<_> = word_ranges(text)
        .into_iter()
        .filter(|range| !is_excluded(range, &excluded))
        .collect();
    let mut spans = Vec::new();
    for (index, range) in tokens.iter().enumerate() {
        let word = &text[range.clone()];
        let previous = index
            .checked_sub(1)
            .map(|previous| &text[tokens[previous].clone()]);
        let next = tokens.get(index + 1).map(|next| &text[next.clone()]);
        if let Some(category) = classify_word(word, previous, next) {
            spans.push(CopyeditSpan {
                range: range.clone(),
                category,
            });
        }
    }
    spans
}

fn classify_word(
    word: &str,
    previous: Option<&str>,
    next: Option<&str>,
) -> Option<CopyeditCategory> {
    let word = lemma(word);
    if word.is_empty() || !word.is_ascii() {
        return None;
    }
    // Exact vocabulary wins over every suffix rule.
    if contains(CONJUNCTIONS, word) {
        return Some(CopyeditCategory::Conjunction);
    }
    if contains(ADVERBS, word) {
        return Some(CopyeditCategory::Adverb);
    }
    if contains(ADJECTIVES, word) {
        return Some(CopyeditCategory::Adjective);
    }
    if contains(VERBS, word) {
        return Some(CopyeditCategory::Verb);
    }
    if contains(NOUNS, word) {
        return Some(CopyeditCategory::Noun);
    }
    suffix_category(word, previous, next)
}

/// Strip a possessive or surrounding apostrophes without allocating.
fn lemma(word: &str) -> &str {
    let mut word = word;
    while let Some(stripped) = word.strip_prefix('\'') {
        word = stripped;
    }
    if let Some(stripped) = word.strip_suffix("'s").or_else(|| word.strip_suffix("'S")) {
        word = stripped;
    } else if let Some(stripped) = word.strip_suffix('\'') {
        word = stripped;
    }
    word
}

fn suffix_category(
    word: &str,
    previous: Option<&str>,
    next: Option<&str>,
) -> Option<CopyeditCategory> {
    if ends_with(word, "ly") && word.len() > 3 && !contains_any(ADVERB_EXCEPTIONS, word) {
        return Some(CopyeditCategory::Adverb);
    }
    if (ends_with(word, "ical") || ends_with(word, "ic")) && word.len() > 4
        || ends_with(word, "ous")
        || ends_with(word, "ful")
        || ends_with(word, "less")
        || ends_with(word, "able")
        || ends_with(word, "ible")
        || ends_with(word, "ive")
        || ends_with(word, "ish")
        || ends_with(word, "al") && word.len() > 4
    {
        return Some(CopyeditCategory::Adjective);
    }
    if (ends_with(word, "tion")
        || ends_with(word, "sion")
        || ends_with(word, "ment")
        || ends_with(word, "ness")
        || ends_with(word, "ity")
        || ends_with(word, "ance")
        || ends_with(word, "ence")
        || ends_with(word, "ship")
        || ends_with(word, "hood")
        || ends_with(word, "ism")
        || ends_with(word, "ist"))
        && word.len() > 4
    {
        return Some(CopyeditCategory::Noun);
    }
    if ends_with(word, "ize") || ends_with(word, "ise") || ends_with(word, "ify") {
        return Some(CopyeditCategory::Verb);
    }
    if ends_with(word, "ing") && word.len() > 4 {
        if contains_any(ING_UNCLASSIFIED, word) {
            return None;
        }
        if contains_any(ING_NOUNS, word) || precedes_determiner(previous) {
            return Some(CopyeditCategory::Noun);
        }
        return Some(CopyeditCategory::Verb);
    }
    if ends_with(word, "ed") && word.len() > 3 {
        if next.is_some_and(|next| contains(NOUNS, lemma(next))) {
            return Some(CopyeditCategory::Adjective);
        }
        return Some(CopyeditCategory::Verb);
    }
    if (ends_with(word, "er") || ends_with(word, "est")) && word.len() > 3 {
        let suffix = if ends_with(word, "est") { 3 } else { 2 };
        if contains(ADJECTIVES, &word[..word.len() - suffix]) {
            return Some(CopyeditCategory::Adjective);
        }
    }
    // Plural and third-person forms reuse the singular's category.
    if ends_with(word, "ies") && word.len() > 4 {
        let base = &word[..word.len() - 3];
        if matches_with_suffix(NOUNS, base, "y") {
            return Some(CopyeditCategory::Noun);
        }
        if matches_with_suffix(VERBS, base, "y") {
            return Some(CopyeditCategory::Verb);
        }
    }
    if ends_with(word, "es") && word.len() > 4 {
        let base = &word[..word.len() - 2];
        if contains(NOUNS, base) {
            return Some(CopyeditCategory::Noun);
        }
        if contains(VERBS, base) {
            return Some(CopyeditCategory::Verb);
        }
    }
    if ends_with(word, "s") && !ends_with(word, "ss") && word.len() > 3 {
        let base = &word[..word.len() - 1];
        if contains(NOUNS, base) {
            return Some(CopyeditCategory::Noun);
        }
        if contains(VERBS, base) {
            return Some(CopyeditCategory::Verb);
        }
    }
    None
}

fn precedes_determiner(previous: Option<&str>) -> bool {
    previous.is_some_and(|word| contains_any(DETERMINERS, lemma(word)))
}

fn ends_with(word: &str, suffix: &str) -> bool {
    word.len() >= suffix.len()
        && word.as_bytes()[word.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

fn contains(list: &[&str], word: &str) -> bool {
    list.binary_search_by(|entry| compare(entry, word)).is_ok()
}

/// Compare `base` plus `suffix` against a lowercase lexicon entry without
/// building the plural form on the heap.
fn matches_with_suffix(list: &[&str], base: &str, suffix: &str) -> bool {
    list.binary_search_by(|entry| compare_with_suffix(entry, base, suffix))
        .is_ok()
}

fn compare(entry: &str, word: &str) -> Ordering {
    entry
        .bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(word.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn compare_with_suffix(entry: &str, base: &str, suffix: &str) -> Ordering {
    entry.bytes().map(|byte| byte.to_ascii_lowercase()).cmp(
        base.bytes()
            .chain(suffix.bytes())
            .map(|byte| byte.to_ascii_lowercase()),
    )
}

fn contains_any(list: &[&str], word: &str) -> bool {
    list.iter().any(|entry| entry.eq_ignore_ascii_case(word))
}

/// Words whose `-ly` ending does not make them adverbs.
const ADVERB_EXCEPTIONS: &[&str] = &[
    "assembly",
    "battery",
    "binary",
    "category",
    "company",
    "daily",
    "early",
    "family",
    "friendly",
    "history",
    "italy",
    "library",
    "likely",
    "lonely",
    "lovely",
    "memory",
    "monthly",
    "only",
    "primary",
    "reply",
    "salary",
    "secondary",
    "silly",
    "supply",
    "temporary",
    "ugly",
    "weekly",
    "yearly",
];

/// `-ing` words that are nouns or otherwise not verbs.
const ING_NOUNS: &[&str] = &[
    "beginning",
    "building",
    "clothing",
    "drawing",
    "evening",
    "feeling",
    "heading",
    "housing",
    "meaning",
    "meeting",
    "morning",
    "opening",
    "painting",
    "setting",
    "thing",
    "warning",
    "wedding",
    "wing",
];

/// `-ing` words that are neither verbs nor nouns.
const ING_UNCLASSIFIED: &[&str] = &["anything", "during", "everything", "nothing", "something"];

const DETERMINERS: &[&str] = &[
    "a", "an", "any", "each", "every", "her", "his", "its", "my", "no", "our", "some", "that",
    "the", "their", "these", "this", "those", "your",
];

/// Words that are conjunctions wherever they appear.
const CONJUNCTIONS: &[&str] = &[
    "although", "and", "as", "because", "both", "but", "either", "for", "if", "neither", "nor",
    "or", "since", "so", "than", "that", "though", "unless", "until", "when", "where", "whereas",
    "whether", "while", "yet",
];

/// Common adverbs; suffix rules cover the rest.
const ADVERBS: &[&str] = &[
    "again",
    "ago",
    "almost",
    "already",
    "also",
    "always",
    "away",
    "carefully",
    "certainly",
    "clearly",
    "correctly",
    "currently",
    "down",
    "easily",
    "enough",
    "even",
    "ever",
    "exactly",
    "far",
    "forward",
    "here",
    "however",
    "just",
    "later",
    "never",
    "not",
    "now",
    "often",
    "once",
    "only",
    "out",
    "perhaps",
    "quickly",
    "quite",
    "rather",
    "really",
    "recently",
    "simply",
    "slowly",
    "sometimes",
    "soon",
    "still",
    "then",
    "there",
    "therefore",
    "thus",
    "today",
    "together",
    "too",
    "twice",
    "usually",
    "very",
    "well",
    "why",
    "yesterday",
];

/// Common adjectives; suffix rules cover the rest.
const ADJECTIVES: &[&str] = &[
    "able",
    "active",
    "actual",
    "available",
    "bad",
    "best",
    "better",
    "big",
    "brief",
    "bright",
    "clear",
    "compact",
    "complete",
    "complex",
    "correct",
    "current",
    "dark",
    "default",
    "deterministic",
    "different",
    "early",
    "easy",
    "empty",
    "english",
    "entire",
    "exact",
    "false",
    "fast",
    "few",
    "final",
    "first",
    "fixed",
    "free",
    "full",
    "good",
    "great",
    "happy",
    "hard",
    "hidden",
    "high",
    "important",
    "known",
    "large",
    "last",
    "late",
    "light",
    "little",
    "local",
    "long",
    "low",
    "main",
    "major",
    "many",
    "new",
    "next",
    "old",
    "online",
    "other",
    "own",
    "previous",
    "private",
    "public",
    "quick",
    "real",
    "recent",
    "red",
    "same",
    "second",
    "selected",
    "semantic",
    "short",
    "simple",
    "small",
    "stable",
    "stale",
    "strong",
    "sure",
    "third",
    "tired",
    "true",
    "unknown",
    "used",
    "usual",
    "valid",
    "visible",
    "whole",
    "wide",
    "young",
];

/// Common verbs; inflection rules cover the rest.
const VERBS: &[&str] = &[
    "accept",
    "add",
    "allow",
    "am",
    "appear",
    "apply",
    "are",
    "ask",
    "be",
    "become",
    "been",
    "begin",
    "being",
    "believe",
    "bring",
    "build",
    "call",
    "can",
    "carry",
    "choose",
    "classify",
    "click",
    "close",
    "come",
    "could",
    "create",
    "decide",
    "describe",
    "did",
    "display",
    "do",
    "does",
    "edit",
    "expect",
    "feel",
    "find",
    "follow",
    "get",
    "give",
    "go",
    "had",
    "happen",
    "has",
    "have",
    "hear",
    "help",
    "hold",
    "include",
    "insert",
    "is",
    "keep",
    "know",
    "leave",
    "let",
    "live",
    "look",
    "make",
    "may",
    "mean",
    "meet",
    "might",
    "move",
    "must",
    "need",
    "open",
    "pass",
    "play",
    "provide",
    "put",
    "read",
    "remain",
    "remember",
    "remove",
    "return",
    "run",
    "say",
    "see",
    "seem",
    "select",
    "send",
    "set",
    "shall",
    "should",
    "show",
    "sit",
    "sleep",
    "speak",
    "stand",
    "start",
    "stop",
    "take",
    "talk",
    "tell",
    "think",
    "try",
    "turn",
    "understand",
    "use",
    "want",
    "was",
    "were",
    "will",
    "would",
    "write",
];

/// Common nouns; plural and suffix rules cover the rest.
const NOUNS: &[&str] = &[
    "action",
    "address",
    "age",
    "answer",
    "application",
    "area",
    "art",
    "article",
    "author",
    "body",
    "book",
    "button",
    "case",
    "category",
    "change",
    "child",
    "code",
    "collection",
    "color",
    "command",
    "company",
    "content",
    "copyedit",
    "cursor",
    "day",
    "document",
    "editor",
    "end",
    "entry",
    "error",
    "example",
    "eye",
    "fact",
    "family",
    "feature",
    "file",
    "form",
    "function",
    "group",
    "guide",
    "hand",
    "header",
    "idea",
    "image",
    "index",
    "information",
    "item",
    "language",
    "life",
    "line",
    "link",
    "list",
    "man",
    "menu",
    "mode",
    "name",
    "number",
    "page",
    "panel",
    "paragraph",
    "part",
    "people",
    "place",
    "point",
    "preference",
    "problem",
    "process",
    "project",
    "reader",
    "result",
    "row",
    "sentence",
    "side",
    "site",
    "story",
    "system",
    "table",
    "text",
    "thing",
    "time",
    "token",
    "value",
    "way",
    "week",
    "window",
    "woman",
    "word",
    "work",
    "world",
    "writer",
    "year",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(text: &str) -> Vec<(String, CopyeditCategory)> {
        classify(text)
            .into_iter()
            .map(|span| (text[span.range].to_owned(), span.category))
            .collect()
    }

    #[test]
    fn every_lexicon_is_sorted_for_binary_search() {
        for list in [CONJUNCTIONS, ADVERBS, ADJECTIVES, VERBS, NOUNS] {
            assert!(
                list.windows(2).all(|pair| pair[0] < pair[1]),
                "lexicon order is broken: {list:?}"
            );
        }
    }

    #[test]
    fn each_category_has_a_representative_corpus() {
        assert_eq!(
            collect("The editor opens the document for the project."),
            [
                ("editor".into(), CopyeditCategory::Noun),
                ("opens".into(), CopyeditCategory::Verb),
                ("document".into(), CopyeditCategory::Noun),
                ("for".into(), CopyeditCategory::Conjunction),
                ("project".into(), CopyeditCategory::Noun),
            ]
        );
        assert_eq!(
            collect("A small, clear sentence is important and exact."),
            [
                ("small".into(), CopyeditCategory::Adjective),
                ("clear".into(), CopyeditCategory::Adjective),
                ("sentence".into(), CopyeditCategory::Noun),
                ("is".into(), CopyeditCategory::Verb),
                ("important".into(), CopyeditCategory::Adjective),
                ("and".into(), CopyeditCategory::Conjunction),
                ("exact".into(), CopyeditCategory::Adjective),
            ]
        );
        assert_eq!(
            collect("Editors quickly read and write clearly today."),
            [
                ("Editors".into(), CopyeditCategory::Noun),
                ("quickly".into(), CopyeditCategory::Adverb),
                ("read".into(), CopyeditCategory::Verb),
                ("and".into(), CopyeditCategory::Conjunction),
                ("write".into(), CopyeditCategory::Verb),
                ("clearly".into(), CopyeditCategory::Adverb),
                ("today".into(), CopyeditCategory::Adverb),
            ]
        );
    }

    #[test]
    fn suffix_and_context_rules_cover_inflections() {
        assert_eq!(
            collect("Writers are writing selected words beautifully."),
            [
                ("Writers".into(), CopyeditCategory::Noun),
                ("are".into(), CopyeditCategory::Verb),
                ("writing".into(), CopyeditCategory::Verb),
                ("selected".into(), CopyeditCategory::Adjective),
                ("words".into(), CopyeditCategory::Noun),
                ("beautifully".into(), CopyeditCategory::Adverb),
            ]
        );
        assert_eq!(
            collect("The building is a meeting of writers."),
            [
                ("building".into(), CopyeditCategory::Noun),
                ("is".into(), CopyeditCategory::Verb),
                ("meeting".into(), CopyeditCategory::Noun),
                ("writers".into(), CopyeditCategory::Noun),
            ],
            "a determiner makes an -ing word a noun"
        );
    }

    #[test]
    fn unknown_tokens_stay_undecorated() {
        assert!(collect("xyzzy plugh quux 2026").is_empty());
        assert!(collect("café naïve 中文").is_empty());
    }

    #[test]
    fn possessive_apostrophes_classify_the_stem() {
        assert_eq!(
            collect("The writer's document's words."),
            [
                ("writer's".into(), CopyeditCategory::Noun),
                ("document's".into(), CopyeditCategory::Noun),
                ("words".into(), CopyeditCategory::Noun),
            ]
        );
    }
}
