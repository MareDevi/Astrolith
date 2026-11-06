//! Source-preserving preview pages, split only between top-level Markdown nodes.

use markdown::{ParseOptions, mdast::Node};
use std::ops::Range;

/// Source pages with shared reference definitions retained for each page parser.
#[derive(Default)]
pub struct PreviewPages {
    /// Contiguous UTF-8 source ranges in document order.
    pub ranges: Vec<Range<usize>>,
    definitions: Vec<Range<usize>>,
}

impl PreviewPages {
    /// Group complete top-level nodes up to a soft byte budget.
    /// Large individual blocks stay whole. Invalid MDX and footnote documents
    /// stay on one page, preserving parser context rather than splitting it.
    pub fn parse(source: &str, mdx: bool, target_bytes: usize) -> Self {
        if source.is_empty() {
            return Self::default();
        }
        let mut options = ParseOptions::gfm();
        options.constructs.math_text = true;
        options.constructs.math_flow = true;
        if mdx {
            options.constructs.html_flow = false;
            options.constructs.html_text = false;
            options.constructs.mdx_expression_flow = true;
            options.constructs.mdx_expression_text = true;
            options.constructs.mdx_jsx_flow = true;
            options.constructs.mdx_jsx_text = true;
        }
        let Ok(Node::Root(root)) = markdown::to_mdast(source, &options) else {
            return Self {
                ranges: std::iter::once(0..source.len()).collect(),
                definitions: Vec::new(),
            };
        };
        if root
            .children
            .iter()
            .any(|node| matches!(node, Node::FootnoteDefinition(_)))
        {
            return Self {
                ranges: std::iter::once(0..source.len()).collect(),
                definitions: Vec::new(),
            };
        }
        let mut pages = Self::default();
        let mut start = 0;
        for node in root.children {
            let Some(position) = node.position() else {
                continue;
            };
            if matches!(node, Node::Definition(_)) {
                pages
                    .definitions
                    .push(position.start.offset..position.end.offset);
            }
            let end = position.end.offset;
            if end - start >= target_bytes {
                pages.ranges.push(start..end);
                start = end;
            }
        }
        if start < source.len() {
            if let Some(last) = pages
                .ranges
                .last_mut()
                .filter(|_| source[start..].trim().is_empty())
            {
                last.end = source.len();
            } else {
                pages.ranges.push(start..source.len());
            }
        }
        pages
    }

    /// Copy one bounded page with reference definitions from other pages.
    pub fn page_source(&self, source: &str, page: usize) -> String {
        let range = &self.ranges[page];
        let mut text = source[range.clone()].to_owned();
        for definition in &self.definitions {
            if definition.start < range.start || definition.end > range.end {
                text.push_str("\n\n");
                text.push_str(&source[definition.clone()]);
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_preserve_bytes_nested_blocks_and_reference_definitions() {
        let source = "[ref]: /cover.png \"Title\"\r\n\r\n# 中文\r\n\r\n- first\r\n- second\r\n\r\n```rs\r\n\r\ncode\r\n```\r\n\r\n[link][ref]\r\n";
        let pages = PreviewPages::parse(source, false, 16);
        assert_eq!(
            pages
                .ranges
                .iter()
                .map(|r| &source[r.clone()])
                .collect::<String>(),
            source
        );
        assert!(pages.ranges.len() > 2);
        for indivisible in ["- first\r\n- second", "```rs\r\n\r\ncode\r\n```"] {
            assert!(
                pages
                    .ranges
                    .iter()
                    .any(|r| source[r.clone()].contains(indivisible))
            );
        }
        let last = pages.page_source(source, pages.ranges.len() - 1);
        let ast = markdown::to_mdast(&last, &markdown::ParseOptions::gfm()).unwrap();
        assert!(format!("{ast:?}").contains("url: \"/cover.png\""));
    }
    #[test]
    fn mdx_component_and_empty_sources_remain_whole() {
        let source = "Before\n\n<Card>\n\n**inside**\n\n</Card>\n\nAfter\n";
        let pages = PreviewPages::parse(source, true, 1);
        assert!(
            pages
                .ranges
                .iter()
                .any(|r| source[r.clone()].contains("<Card>\n\n**inside**\n\n</Card>"))
        );
        assert!(PreviewPages::parse("", false, 1).ranges.is_empty());
    }
}
