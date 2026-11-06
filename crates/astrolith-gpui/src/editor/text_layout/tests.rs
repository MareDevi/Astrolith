use super::*;
use gpui_kit::TestAppContext;
use std::sync::Arc;

fn test_style() -> Arc<LayoutStyle> {
    Arc::new(LayoutStyle {
        font: font(".SystemUIFont"),
        mono_font: font(".SystemUIFont"),
        font_size: px(16.),
        line_height: 1.5,
        foreground: gpui_kit::black(),
        muted_foreground: gpui_kit::black(),
        link: gpui_kit::black(),
        code_background: gpui_kit::black(),
        selection: gpui_kit::black(),
        caret: gpui_kit::black(),
        border: gpui_kit::black(),
        background: gpui_kit::black(),
        heading: gpui_kit::black(),
        render_images: false,
    })
}

fn parse(text: &str) -> Arc<WysiwygDocument> {
    Arc::new(WysiwygDocument::parse(text))
}

fn prepare_all(layout: &mut TextLayout, text_system: &WindowTextSystem, height: f32) -> Prepared {
    layout.prepare_with(text_system, Viewport { scroll: 0., height }, None, 0.)
}

#[gpui_kit::test]
fn rows_stay_on_grapheme_and_break_boundaries(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let source = "The quick brown fox jumps over the lazy dog. 中文没有空格的长段落\n\
                      a\textra e\u{301}👍🏽👩\u{200d}💻 tail\n\
                      aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let document = parse(source);
        let mut layout = TextLayout::new(document.clone(), test_style(), px(160.), &ImageMetrics::default());
        prepare_all(&mut layout, &text_system, 10_000.);
        assert_eq!(layout.blocks.len(), document.blocks.len());
        for (index, block) in layout.blocks.iter().enumerate() {
            let text = &document.blocks[index].text.text;
            assert!(block.complete, "the whole small document is wrapped");
            assert!(!block.rows.is_empty());
            let mut covered = 0usize;
            for (row_index, row) in block.rows.iter().enumerate() {
                assert!(
                    text.is_char_boundary(row.range.start) && text.is_char_boundary(row.range.end),
                    "row boundaries are grapheme boundaries"
                );
                assert!(row.range.start >= covered, "rows advance");
                if row.range.start > covered {
                    assert_eq!(
                        &text[covered..row.range.start],
                        "\n",
                        "only a mandatory newline separates rows"
                    );
                }
                if row.range.start < row.range.end && row.range.end < text.len() {
                    let is_opportunity = block.breaks.binary_search(&row.range.end).is_ok()
                        || text.as_bytes().get(row.range.end) == Some(&b'\n');
                    let emergency = !block
                        .breaks
                        .iter()
                        .any(|offset| *offset > row.range.start && *offset <= row.range.end);
                    assert!(
                        is_opportunity || emergency,
                        "block {index} row {row_index} {:?} ends only at a break opportunity, newline or inside an unbreakable run",
                        row.range
                    );
                }
                covered = row.range.end;
            }
            assert_eq!(covered, text.len(), "rows cover the block text");
        }
    });
}

#[gpui_kit::test]
fn long_block_wraps_and_shapes_only_the_viewport(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let unit = "这只是一个没有空格的中文长段落用于验证可见区布局。";
        let mut source = String::new();
        while source.len() < 1_048_576 {
            source.push_str(unit);
        }
        let document = parse(&source);
        assert_eq!(document.blocks.len(), 1, "one semantic paragraph");
        let mut layout = TextLayout::new(
            document.clone(),
            test_style(),
            px(600.),
            &ImageMetrics::default(),
        );
        let prepared = layout.prepare_with(
            &text_system,
            Viewport {
                scroll: 0.,
                height: 600.,
            },
            None,
            300.,
        );
        let stats = layout.stats();
        assert!(
            stats.wrapped_rows >= 3,
            "a screen of rows is wrapped (pending={})",
            prepared.pending
        );
        assert!(
            stats.wrapped_bytes < 64 * 1024,
            "wrapping stops at the rows the viewport needs: {} bytes",
            stats.wrapped_bytes
        );
        assert!(
            stats.shaped_bytes < 64 * 1024,
            "shaping is limited to visible rows: {} bytes",
            stats.shaped_bytes
        );
        let first_rows = layout.blocks[0].rows.len();

        let half = layout.total_height() * 0.5;
        let _prepared = layout.prepare_with(
            &text_system,
            Viewport {
                scroll: half,
                height: 600.,
            },
            Some(Hit {
                block: 0,
                rendered: source.len() / 2,
            }),
            300.,
        );
        assert!(
            layout.blocks[0].rows.len() > first_rows,
            "a deep viewport resumes wrapping instead of restarting"
        );
        assert!(
            layout.stats().wrapped_bytes < 512 * 1024,
            "one frame never walks the whole 1 MiB paragraph"
        );
    });
}

#[gpui_kit::test]
fn caret_hit_test_and_selection_use_the_same_row_geometry(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let document =
            parse("A short paragraph with enough words to wrap onto a second visual row.\n");
        let mut layout = TextLayout::new(
            document.clone(),
            test_style(),
            px(120.),
            &ImageMetrics::default(),
        );
        prepare_all(&mut layout, &text_system, 2000.);
        let block = &layout.blocks[0];
        assert!(block.rows.len() >= 2, "the fixture wraps");
        let row = &block.rows[1];
        let middle = row.range.start + (row.range.len() / 2);
        let hit = Hit {
            block: 0,
            rendered: middle,
        };
        let caret = layout.caret(hit).expect("visible row has a caret");
        let back = layout
            .hit_test(point(caret.origin.x + px(0.5), caret.origin.y + px(1.)))
            .expect("caret position hits the shaped row");
        assert_eq!(back, hit, "hit-test and caret agree");

        let selection = Selection {
            anchor: Hit {
                block: 0,
                rendered: row.range.start,
            },
            focus: hit,
        };
        let quads = layout.selection_quads(selection);
        assert_eq!(quads.len(), 1, "one row is selected");
        let bounds = layout
            .bounds_for_range(selection.anchor, selection.focus)
            .expect("selection bounds");
        assert!(
            caret.origin.x >= bounds.left()
                && caret.origin.x <= bounds.right()
                && caret.origin.y >= bounds.top()
                && caret.origin.y < bounds.bottom(),
            "the caret sits inside the selected row: bounds={bounds:?} caret={caret:?}"
        );
        let ime = layout
            .bounds_for_range(hit, hit)
            .expect("collapsed IME range has a caret box");
        assert_eq!(ime.origin, caret.origin);
    });
}

#[gpui_kit::test]
fn unresolved_geometry_fails_closed(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let source = "长".repeat(400_000);
        let document = parse(&source);
        let mut layout =
            TextLayout::new(document, test_style(), px(600.), &ImageMetrics::default());
        let prepared = layout.prepare_with(
            &text_system,
            Viewport {
                scroll: 0.,
                height: 400.,
            },
            None,
            100.,
        );
        assert!(!prepared.pending || layout.stats().pending);
        let far = Hit {
            block: 0,
            rendered: source.len() - 1,
        };
        assert!(
            layout.caret(far).is_none() && layout.bounds_for_range(far, far).is_none(),
            "an unmeasured row exposes no invented geometry"
        );
    });
}

#[test]
fn height_index_updates_locally_and_finds_blocks_by_position() {
    let mut index = HeightIndex::new(4);
    for (slot, height) in [10., 20., 30., 40.].into_iter().enumerate() {
        index.set(slot, height);
    }
    assert_eq!(index.total(), 100.);
    assert_eq!(index.prefix(2), 30.);
    assert_eq!(index.block_at(-1.), Some(0));
    assert_eq!(index.block_at(0.), Some(0));
    assert_eq!(index.block_at(9.9), Some(0));
    assert_eq!(index.block_at(10.), Some(1));
    assert_eq!(index.block_at(30.), Some(2));
    assert_eq!(index.block_at(99.9), Some(3));
    assert_eq!(index.block_at(100.), Some(3));
    index.set(1, 5.);
    assert_eq!(index.total(), 85.);
    assert_eq!(index.block_at(10.), Some(1));
}

#[gpui_kit::test]
fn local_edit_reuses_unchanged_blocks_and_the_safe_prefix(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let source = "First paragraph stays untouched.\n\n\
                      Second paragraph changes here.\n\n\
                      Third paragraph stays untouched.";
        let mut layout = TextLayout::new(
            parse(source),
            test_style(),
            px(320.),
            &ImageMetrics::default(),
        );
        prepare_all(&mut layout, &text_system, 800.);
        let first_rows = layout.blocks[0].rows.len();
        let third_rows = layout.blocks[2].rows.len();
        assert!(first_rows > 0 && third_rows > 0);

        let edited = source.replace("changes here", "changes here now");
        let document = parse(&edited);
        let outcome = layout.update(document.clone());
        assert_eq!(outcome.reused_blocks, 2, "unaffected blocks keep identity");
        assert_eq!(outcome.rebuilt_blocks, 1);
        assert_eq!(layout.blocks.len(), document.blocks.len());
        assert_eq!(layout.blocks[0].rows.len(), first_rows);
        assert_eq!(layout.blocks[2].rows.len(), third_rows);
        assert_eq!(
            layout.blocks[1].text.as_ref(),
            "Second paragraph changes here now."
        );
    });
}

#[gpui_kit::test]
fn appending_to_a_long_block_reuses_the_wrapped_prefix(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let mut source = "长".repeat(50_000);
        let mut layout = TextLayout::new(
            parse(&source),
            test_style(),
            px(400.),
            &ImageMetrics::default(),
        );
        prepare_all(&mut layout, &text_system, 300.);
        let measured = layout.blocks[0].rows.len();
        assert!(measured > 0, "the viewport measured part of the block");

        source.push('末');
        let outcome = layout.update(parse(&source));
        assert_eq!(outcome.reused_blocks, 0, "the edited block is rebuilt");
        assert!(
            outcome.reused_rows >= measured,
            "rows before the edit prefix are carried: {} of {}",
            outcome.reused_rows,
            measured
        );
        assert!(
            layout.blocks[0].rows.len() >= measured,
            "carried rows stay in the block"
        );
        assert_eq!(
            layout.blocks[0].text.as_ref().len(),
            source.len(),
            "the block tracks the new text"
        );
    });
}

#[gpui_kit::test]
fn resolved_runs_carry_heading_size_bold_italic_code_and_link(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let document = parse("## Head **bold** *italic* `code` [link](https://example.com)\n");
        let style = test_style();
        let layout = TextLayout::new(
            document.clone(),
            style.clone(),
            px(600.),
            &ImageMetrics::default(),
        );
        let block = &layout.blocks[0];
        let text = block.text.as_str();
        assert_eq!(
            block.font_size,
            16. * 1.6,
            "a level-2 heading shapes at the heading size"
        );
        let run_covering = |needle: &str| {
            let start = text.find(needle).expect("fixture text");
            block
                .runs
                .iter()
                .find(|run| run.range.start <= start && start < run.range.end)
                .expect("a run covers the styled text")
        };
        assert_eq!(run_covering("bold").font.weight, FontWeight::BOLD);
        assert_eq!(run_covering("italic").font.style, FontStyle::Italic);
        assert_eq!(run_covering("code").font.family, style.mono_font.family);
        assert_eq!(run_covering("code").background, Some(style.code_background));
        assert_eq!(run_covering("link").color, style.link);
    });
}

#[gpui_kit::test]
fn table_cells_wrap_and_share_one_row_geometry(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let source =
            "| 中文长段落 | b |\n| --- | --- |\n| 一行很长的表格单元格内容需要换行 | 文 |\n";
        let document = parse(source);
        let mut layout = TextLayout::new(
            document.clone(),
            test_style(),
            px(240.),
            &ImageMetrics::default(),
        );
        prepare_all(&mut layout, &text_system, 4_000.);
        let row = &layout.blocks[1];
        assert!(row.is_table());
        assert_eq!(row.cells.len(), 2);
        assert!(row.cells.iter().all(|cell| !cell.rows.is_empty()));
        let top = layout.block_top(1);
        let cell_start = row.cell_starts[1];
        let hit = layout
            .hit_test(point(px(row.cells[1].x_offset + 2.), px(top + 10.)))
            .expect("the second cell has shaped geometry");
        assert_eq!(hit.block, 1);
        assert!(hit.rendered >= cell_start);
        let caret = layout
            .caret(Hit {
                block: 1,
                rendered: cell_start,
            })
            .expect("the cell start has a caret");
        assert!(
            f32::from(caret.origin.x) >= row.cells[1].x_offset,
            "cell carets use the column origin"
        );
        let quads = layout.selection_quads(Selection {
            anchor: Hit {
                block: 1,
                rendered: 0,
            },
            focus: Hit {
                block: 1,
                rendered: cell_start,
            },
        });
        assert!(!quads.is_empty(), "selection covers the first cell");
    });
}

#[gpui_kit::test]
fn image_blocks_reserve_loaded_and_fallback_geometry(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let source = "![cover](./cover.png)\n";
        let document = parse(source);
        let style = Arc::new((*test_style()).clone().with_images(true));
        let mut images = ImageMetrics::default();
        images.insert("./cover.png".to_owned(), (800., 400.));
        let mut layout = TextLayout::new(document.clone(), style, px(400.), &images);
        assert_eq!(layout.blocks[0].image.as_ref().unwrap().height, 200.);
        assert!(layout.set_image_metrics(&ImageMetrics::default()));
        assert_eq!(
            layout.blocks[0].image.as_ref().unwrap().height,
            400. * IMAGE_FALLBACK_ASPECT
        );
        assert!(layout.blocks[0].image.is_some());
        layout.prepare_with(
            &WindowTextSystem::new(cx.text_system().clone()),
            Viewport {
                scroll: 0.,
                height: 1_000.,
            },
            None,
            0.,
        );
    });
}

#[gpui_kit::test]
fn style_edits_invalidate_carried_rows(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let source = "plain text across several visual rows. ".repeat(20);
        let mut layout = TextLayout::new(
            parse(&source),
            test_style(),
            px(160.),
            &ImageMetrics::default(),
        );
        prepare_all(&mut layout, &text_system, 800.);
        assert!(layout.blocks[0].rows[0].shaped.is_some());
        let before_weight = layout.blocks[0].run_at(0).font.weight;
        layout.update(parse(&format!("**{}**", source.trim_end())));
        assert_ne!(layout.blocks[0].run_at(0).font.weight, before_weight);
        assert!(
            layout.blocks[0].rows.is_empty(),
            "changed font metrics invalidate old wrapping and glyphs"
        );
        prepare_all(&mut layout, &text_system, 800.);
        layout.update(parse(&format!("~~**{}**~~", source.trim_end())));
        assert!(
            layout.blocks[0].rows.is_empty(),
            "changed decorations invalidate old glyphs too"
        );
    });
}

#[gpui_kit::test]
fn theme_changes_restyle_cached_table_cells(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let mut layout = TextLayout::new(
            parse("| header |\n| --- |\n| body |\n"),
            test_style(),
            px(400.),
            &ImageMetrics::default(),
        );
        prepare_all(&mut layout, &text_system, 800.);
        let cell = &layout.blocks[0].cells[0];
        assert!(cell.rows[0].shaped.is_some());
        let old_rows = cell.rows.len();
        let mut style = (*test_style()).clone();
        style.foreground = gpui_kit::white();
        layout.set_style(Arc::new(style));
        let cell = &layout.blocks[0].cells[0];
        assert_eq!(cell.rows.len(), old_rows, "colour changes retain wrapping");
        assert!(
            cell.rows[0].shaped.is_none(),
            "old table glyph colours must be invalidated"
        );
        assert_eq!(cell.run_at(0).color, gpui_kit::white());
    });
}
