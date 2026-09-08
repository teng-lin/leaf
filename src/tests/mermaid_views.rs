use super::*;
use crate::markdown::{
    parse_markdown_with_options, MermaidCache, MermaidOptions, MermaidPreview, MermaidRenderContext,
};
use std::sync::Arc;

#[test]
fn narrow_state_diagram_preserves_every_row_in_document() {
    let (ss, theme) = test_assets();
    let source = include_str!("../../tests/fixtures/diagrams/state_medium.md");
    let raw = source
        .split("```mermaid\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let preview =
        crate::markdown::render_mermaid_preview(raw, MermaidOptions::default(), false).unwrap();
    assert!(preview.height > 24);
    let mut cache = MermaidCache::new();
    cache.insert(raw.into(), Ok(Arc::new(preview.clone())));
    for width in [40, 80, 140] {
        let parsed = parse_markdown_with_options(
            source,
            &ss,
            &theme,
            width,
            &test_md_theme(),
            false,
            false,
            MermaidRenderContext {
                cache: Some(&cache),
                ..Default::default()
            },
        );
        let block = &parsed.diagrams[0];
        let rows: Vec<_> = parsed.lines[block.rendered_start..=block.rendered_end]
            .iter()
            .map(line_plain_text)
            .collect();
        let text = rows.join("\n");
        assert!(text.contains("State10"), "{text}");
        assert!(!text.contains("clipped"), "{text}");
        assert!(rows.len() >= preview.height);
        for (i, expected) in preview.text.lines().enumerate() {
            assert!(rows[i + 1].contains(expected), "lost row {i}: {expected:?}");
        }
    }
}

#[test]
fn grouped_large_preview_is_an_overview_but_full_export_keeps_all_nodes() {
    let source = include_str!("../../tests/fixtures/diagrams/flowchart_large.md");
    let raw = source
        .split("```mermaid\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let preview =
        crate::markdown::render_mermaid_preview(raw, MermaidOptions::COMPACT, false).unwrap();
    assert!(preview.overview);
    assert_eq!(preview.hidden_nodes, 200);
    assert!(preview.text.contains("Segment 1") && preview.text.contains("Segment 10"));
    assert_eq!(preview.nodes.len(), 10);
    let full = crate::markdown::render_mermaid_preview(raw, MermaidOptions::COMPACT, true).unwrap();
    assert!(!full.overview);
    assert_eq!(full.hidden_nodes, 0);
    assert_eq!(full.nodes.len(), 200);
    assert!(full.text.contains("Node 1") && full.text.contains("Node 200"));
}

#[test]
fn dense_preview_contains_complete_nodes_and_readable_navigation_at_terminal_widths() {
    let (ss, theme) = test_assets();
    let source = include_str!("../../tests/fixtures/diagrams/dense-cyclic-200.md");
    let raw = source
        .split("```mermaid\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let preview =
        crate::markdown::render_mermaid_preview(raw, MermaidOptions::COMPACT, false).unwrap();
    assert!(
        !preview.overview,
        "ungrouped graphs must not invent an overview"
    );
    let mut cache = MermaidCache::new();
    cache.insert(raw.into(), Ok(Arc::new(preview.clone())));
    for width in [80, 120, 200, 270] {
        let (x, y) = preview.preview_origin(width - 4, 24);
        let complete = preview
            .nodes
            .iter()
            .filter(|node| {
                node.x >= x
                    && node.x + node.width <= x + width - 4
                    && node.y >= y
                    && node.y + node.height <= y + 24
            })
            .count();
        assert!(
            complete >= 3,
            "{width}: only {complete} complete nodes at {x},{y}"
        );
        let md_theme = test_md_theme();
        let parsed = parse_markdown_with_options(
            source,
            &ss,
            &theme,
            width,
            &md_theme,
            false,
            false,
            MermaidRenderContext {
                cache: Some(&cache),
                ..Default::default()
            },
        );
        let block = &parsed.diagrams[0];
        let rows = &parsed.lines[block.rendered_start..=block.rendered_end];
        let first_label = rows
            .iter()
            .position(|row| {
                line_plain_text(row).split_whitespace().any(|word| {
                    word.strip_prefix('N').is_some_and(|number| {
                        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                    })
                })
            })
            .unwrap();
        assert!(
            first_label <= 5,
            "{width}: empty initial view, first label on row {first_label}"
        );
        assert!(rows
            .iter()
            .all(|row| display_width(&line_plain_text(row)) <= width));
        let hint = rows
            .iter()
            .flat_map(|row| &row.spans)
            .find(|span| span.content.contains("v open"))
            .unwrap();
        assert_eq!(hint.style.fg, Some(md_theme.text));
        assert_ne!(hint.style.fg, Some(md_theme.code_frame));
    }
}

#[test]
fn preview_origin_finds_a_node_below_and_right_of_an_empty_corner() {
    let preview = MermaidPreview {
        width: 400,
        height: 41,
        nodes: vec![mmdflux::TextCellRect {
            x: 300,
            y: 36,
            width: 10,
            height: 3,
        }],
        ..Default::default()
    };
    let (x, y) = preview.preview_origin(76, 24);
    assert!(x <= 300 && x + 76 >= 310);
    assert!(y <= 36 && y + 24 >= 39);
    assert!(36 - y < 3, "do not spend most of the preview on blank rows");
}

#[test]
fn spacing_cli_validation_and_full_mode() {
    let parse = |args: &[&str]| parse_cli(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let options = parse(&[
        "leaf",
        "--node-spacing=8",
        "--rank-spacing",
        "15",
        "--edge-spacing",
        "8",
    ])
    .unwrap();
    assert_eq!(
        (
            options.node_spacing,
            options.rank_spacing,
            options.edge_spacing
        ),
        (Some(8.0), Some(15.0), Some(8.0))
    );
    for invalid in ["NaN", "inf", "-1", "501", "", "wat"] {
        assert!(
            parse(&["leaf", "--node-spacing", invalid]).is_err(),
            "{invalid}"
        );
    }
    assert!(parse(&["leaf", "--mermaid-full"]).is_err());
    assert!(
        parse(&["leaf", "--inline", "--mermaid-full"])
            .unwrap()
            .mermaid_full
    );
}

#[test]
fn spacing_resolution_is_per_field_and_lenient() {
    let config: LeafConfig =
        toml::from_str("[mermaid]\nnode-spacing=8\nrank-spacing=15.5\nedge-spacing='bad'\n")
            .unwrap();
    let result = config
        .mermaid
        .resolve([None, Some(30.0), None], [Some("9"), Some("NaN"), None]);
    assert_eq!(
        result,
        MermaidOptions {
            node_spacing: 9.0,
            rank_spacing: 30.0,
            edge_spacing: 20.0
        }
    );
    let config: LeafConfig =
        toml::from_str("[mermaid]\nnode-spacing=nan\nrank-spacing=-1\nedge-spacing=501\n").unwrap();
    assert_eq!(
        config.mermaid.resolve([None; 3], [None; 3]),
        MermaidOptions::default()
    );
    let cfg = MermaidOptions::COMPACT.render_config();
    assert_eq!(
        (
            cfg.layout.node_sep,
            cfg.layout.rank_sep,
            cfg.layout.edge_sep
        ),
        (8.0, 15.0, 8.0)
    );
}

#[test]
fn cell_slice_keeps_graphemes_and_partial_glyph_alignment() {
    use crate::markdown::width::slice_display_columns as slice;
    assert_eq!(slice("Ae\u{301}界👩‍💻Z", 0, 7), "Ae\u{301}界👩‍💻Z");
    assert_eq!(slice("Ae\u{301}界👩‍💻Z", 3, 7), " 👩‍💻Z");
    assert_eq!(slice("A界Z", 0, 2), "A ");
    assert_eq!(slice("A\tZ", 2, 5), "  Z");
    assert_eq!(slice("abc", 20, 30), "");
    assert!(!slice("A\x1b[31mB", 0, 80).contains('\x1b'));
}

#[test]
fn deferred_preview_preserves_two_block_identities_without_solving() {
    let (ss, theme) = test_assets();
    let source = "# Start\n\n```mermaid\ninvalid ENGINE INPUT\n```\n\n```mermaid\ninvalid ENGINE INPUT\n```\n";
    let cache = MermaidCache::new();
    let parsed = parse_markdown_with_options(
        source,
        &ss,
        &theme,
        40,
        &test_md_theme(),
        false,
        true,
        MermaidRenderContext {
            cache: Some(&cache),
            ..Default::default()
        },
    );
    assert_eq!(parsed.diagrams.len(), 2);
    assert_eq!(parsed.diagrams[0].source_line, 3);
    assert_eq!(parsed.diagrams[1].source_line, 7);
    assert_eq!(parsed.diagrams[0].source, parsed.diagrams[1].source);
    assert_eq!(parsed.diagrams[1].ordinal, 1);
    assert_eq!(parsed.diagrams[1].code_block_index, 1);
    let text = parsed
        .lines
        .iter()
        .map(line_plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Loading diagram"));
    assert!(!text.contains("warning"));
}

#[test]
fn cached_canvas_is_clipped_not_rewrapped_and_full_export_is_unwrapped() {
    let (ss, theme) = test_assets();
    let raw = "flowchart LR\nA-->B\n";
    let source = format!("```mermaid\n{raw}```\n");
    let row = format!("LEFT{}RIGHT", "─".repeat(120));
    let canvas = std::iter::repeat_n(row.clone(), 40)
        .collect::<Vec<_>>()
        .join("\n");
    let mut cache = MermaidCache::new();
    cache.insert(
        raw.into(),
        Ok(Arc::new(MermaidPreview {
            text: canvas,
            width: 129,
            height: 40,
            warning: None,
            ..Default::default()
        })),
    );
    for width in [1, 4, 20, 40, 80] {
        let preview = parse_markdown_with_options(
            &source,
            &ss,
            &theme,
            width,
            &test_md_theme(),
            false,
            true,
            MermaidRenderContext {
                cache: Some(&cache),
                ..Default::default()
            },
        );
        let rows: Vec<_> = preview.lines.iter().map(line_plain_text).collect();
        assert!(
            rows.iter().all(|row| display_width(row) <= width),
            "{width}: {rows:?}"
        );
        assert!(!rows.iter().any(|row| row.contains("RIGHT")));
        assert!(preview.diagrams[0].rendered_end - preview.diagrams[0].rendered_start <= 28);
        if width >= 40 {
            assert!(rows.iter().any(|row| row.contains("clipped 129×40")));
        }
    }
    let full = parse_markdown_with_options(
        &source,
        &ss,
        &theme,
        40,
        &test_md_theme(),
        false,
        true,
        MermaidRenderContext {
            cache: Some(&cache),
            complete: true,
            ..Default::default()
        },
    );
    let range = full.diagrams[0].rendered_start..=full.diagrams[0].rendered_end;
    for format in [
        crate::inline::ResolvedFormat::Plain,
        crate::inline::ResolvedFormat::Ansi,
    ] {
        let mut output = Vec::new();
        crate::inline::write_lines_with_unwrapped(
            &full.lines,
            format,
            40,
            std::slice::from_ref(&range),
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.matches(&row).count(), 40);
    }
}

#[test]
fn nested_preview_prefixes_fit_and_original_source_is_preserved() {
    let (ss, theme) = test_assets();
    let raw = "A-->B\n";
    let mut cache = MermaidCache::new();
    cache.insert(
        raw.into(),
        Ok(Arc::new(MermaidPreview {
            text: "界👩‍💻e\u{301}".repeat(25),
            width: 125,
            height: 1,
            warning: None,
            ..Default::default()
        })),
    );
    for source in [
        "> ```mermaid\n> A-->B\n> ```\n",
        "- item\n\n  ```mermaid\n  A-->B\n  ```\n",
    ] {
        let parsed = parse_markdown_with_options(
            source,
            &ss,
            &theme,
            30,
            &test_md_theme(),
            false,
            true,
            MermaidRenderContext {
                cache: Some(&cache),
                ..Default::default()
            },
        );
        assert_eq!(parsed.diagrams[0].source, raw);
        assert_eq!(parsed.code_blocks[0].raw_content, raw);
        for line in &parsed.lines {
            assert!(display_width(&line_plain_text(line)) <= 30);
        }
    }
}

#[test]
fn full_writer_only_exempts_diagram_rows_and_does_not_split_emoji() {
    let lines = [
        Line::raw("0123456789"),
        Line::raw("👩‍💻👩‍💻👩‍💻"),
        Line::raw("0123456789"),
    ];
    let mut output = Vec::new();
    crate::inline::write_lines_with_unwrapped(
        &lines,
        crate::inline::ResolvedFormat::Plain,
        4,
        &[0..=0],
        &mut output,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "0123456789\n👩‍💻👩‍💻\n👩‍💻\n0123\n4567\n89\n"
    );
}

#[test]
fn complete_source_fallback_does_not_allocate_a_padded_rectangle() {
    let (ss, theme) = test_assets();
    let raw = format!("{}\n{}", "x".repeat(50_000), "short\n".repeat(1000));
    let source = format!("```mermaid\n{raw}```\n");
    let mut cache = MermaidCache::new();
    cache.insert(raw.clone(), Err("Input limit exceeded".into()));
    let parsed = parse_markdown_with_options(
        &source,
        &ss,
        &theme,
        80,
        &test_md_theme(),
        false,
        true,
        MermaidRenderContext {
            cache: Some(&cache),
            complete: true,
            ..Default::default()
        },
    );
    let output: String = parsed
        .lines
        .iter()
        .map(line_plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        output.len() < raw.len() + 1000,
        "fallback must be proportional to source bytes"
    );
    assert_eq!(output.matches("short").count(), 1000);
    assert!(output.contains(&"x".repeat(50_000)));
}

#[test]
fn footnote_diagram_is_deferred_searchable_openable_and_never_wrapped() {
    let (ss, theme) = test_assets();
    let raw = "flowchart LR\nA-->B\n";
    // Keep Leaf's existing legacy-footnote dialect: a blank line closes a note.
    let source = "See note[^diagram].\n\n[^diagram]:\n```mermaid\nflowchart LR\nA-->B\n```\n";
    let mut cache = MermaidCache::new();
    let deferred = parse_markdown_with_options(
        source,
        &ss,
        &theme,
        40,
        &test_md_theme(),
        false,
        true,
        MermaidRenderContext {
            cache: Some(&cache),
            ..Default::default()
        },
    );
    assert_eq!(
        deferred.diagrams.len(),
        1,
        "{:?}",
        deferred
            .lines
            .iter()
            .map(line_plain_text)
            .collect::<Vec<_>>()
    );
    assert_eq!(deferred.diagrams[0].source, raw);
    assert_eq!(deferred.diagrams[0].source_line, 4);
    assert_eq!(deferred.code_blocks[0].raw_content, raw);
    let row = format!("LEFT{}RIGHT", "─".repeat(80));
    cache.insert(
        raw.into(),
        Ok(Arc::new(MermaidPreview {
            text: row.clone(),
            width: 89,
            height: 1,
            warning: None,
            ..Default::default()
        })),
    );
    let preview = parse_markdown_with_options(
        source,
        &ss,
        &theme,
        40,
        &test_md_theme(),
        false,
        true,
        MermaidRenderContext {
            cache: Some(&cache),
            ..Default::default()
        },
    );
    assert!(preview
        .lines
        .iter()
        .all(|line| display_width(&line_plain_text(line)) <= 40));
    let block = &preview.diagrams[0];
    assert!(line_plain_text(&preview.lines[block.rendered_start]).contains("┌─ mermaid"));
    let full = parse_markdown_with_options(
        source,
        &ss,
        &theme,
        40,
        &test_md_theme(),
        false,
        true,
        MermaidRenderContext {
            cache: Some(&cache),
            complete: true,
            ..Default::default()
        },
    );
    assert!(full
        .lines
        .iter()
        .any(|line| line_plain_text(line).contains(&row)));
}
