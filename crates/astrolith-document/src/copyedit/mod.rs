//! Deterministic offline English classification for Copyedit mode.
//!
//! The classifier recognises exactly the upstream feature's five categories:
//! nouns, verbs, adjectives, adverbs and conjunctions. It is a bounded lexicon
//! plus ordered suffix/context rules, never a network call, model or
//! thread-local cache. Unknown words stay undecorated.
//!
//! Analysis input is the writer's *visible* projection, so hidden frontmatter
//! and MDX imports never reach the classifier. On top of that, [`analyze`]
//! excludes fenced and inline code, link destinations and HTML/MDX tags.

mod classify;
mod tokenize;

pub(crate) use tokenize::word_ranges;

use std::ops::Range;

use crate::projection::WriterProjection;
pub use astrolith_core::preferences::CopyeditCategory;

/// One classified visible word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyeditSpan {
    /// UTF-8 byte range in the analysed visible text.
    pub range: Range<usize>,
    /// The category the word matched.
    pub category: CopyeditCategory,
}

/// Classify the words of one visible writer snapshot.
///
/// The result is in document order and its ranges never overlap. Markdown and
/// MDX code, destinations and tags are skipped; hidden projection bytes are
/// already absent.
pub fn analyze(visible: &str) -> Vec<CopyeditSpan> {
    classify::classify(visible)
}

/// Classify the visible bytes of a writer projection.
///
/// Frontmatter and a proven MDX ESM prelude are hidden by
/// [`WriterProjection`], so they are excluded before classification starts.
pub fn analyze_projection(projection: &WriterProjection) -> Vec<CopyeditSpan> {
    analyze(&projection.visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{DocumentKind, project_writer};

    fn words<'a>(text: &'a str, spans: &[CopyeditSpan]) -> Vec<(&'a str, CopyeditCategory)> {
        spans
            .iter()
            .map(|span| (&text[span.range.clone()], span.category))
            .collect()
    }

    #[test]
    fn classification_uses_the_footer_token_snapshot_boundaries() {
        let text = "The editor writes words and café.";
        let snapshot = crate::token_snapshot(text);
        let spans = analyze(text);
        assert!(!spans.is_empty());
        for span in spans {
            assert!(
                snapshot.tokens.contains(&span.range),
                "every classified span is one footer token"
            );
        }
    }

    #[test]
    fn frontmatter_and_hidden_imports_never_reach_the_classifier() {
        let source = concat!(
            "---\n",
            "title: Noun\n",
            "description: Adjective\n",
            "---\n",
            "import Verb from \"./Verb.astro\";\n",
            "\n",
            "The editor writes words.\n",
        );
        let projection = project_writer(source, DocumentKind::Mdx).expect("projection");
        let spans = analyze_projection(&projection);
        assert_eq!(
            words(&projection.visible, &spans),
            [
                ("editor", CopyeditCategory::Noun),
                ("writes", CopyeditCategory::Verb),
                ("words", CopyeditCategory::Noun),
            ]
        );
    }

    #[test]
    fn code_links_and_tags_are_absent_from_the_result() {
        let text = concat!(
            "Read the guide now.\n",
            "\n",
            "```rust\n",
            "let noun = 1;\n",
            "```\n",
            "\n",
            "Use `verb` and [docs](https://example.dev/word) <Card kind=\"adverb\">text</Card>.\n",
        );
        assert_eq!(
            words(text, &analyze(text)),
            [
                ("Read", CopyeditCategory::Verb),
                ("guide", CopyeditCategory::Noun),
                ("now", CopyeditCategory::Adverb),
                ("Use", CopyeditCategory::Verb),
                ("and", CopyeditCategory::Conjunction),
                ("text", CopyeditCategory::Noun),
            ],
            "fenced and inline code, destinations and tags never contribute words"
        );
    }

    #[test]
    fn ranges_are_sorted_non_overlapping_and_char_boundaries() {
        let text = "Été déjà vu.\r\n\r\nThe editor writes café words.\r\n";
        let spans = analyze(text);
        assert!(text.is_char_boundary(0));
        for pair in spans.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start);
        }
        for span in &spans {
            assert!(text.is_char_boundary(span.range.start));
            assert!(text.is_char_boundary(span.range.end));
        }
        assert_eq!(
            words(text, &spans),
            [
                ("editor", CopyeditCategory::Noun),
                ("writes", CopyeditCategory::Verb),
                ("words", CopyeditCategory::Noun),
            ]
        );
    }
}
