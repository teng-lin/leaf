use crate::{app::App, markdown::width::slice_display_columns, theme::app_theme};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

pub(super) fn render_diagram(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let theme = app_theme();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);
    app.set_diagram_area(sections[1]);
    let Some(viewer) = app.diagram_viewer() else {
        return;
    };
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(theme.ui.content_bg)),
        area,
    );
    let ready = if viewer.loading {
        "loading"
    } else if viewer.error.is_some() {
        "error"
    } else {
        "ready"
    };
    let mode = if viewer.source_mode {
        "source"
    } else if viewer.view.focus.is_some() {
        "focus ±1 hop"
    } else if viewer.view.overview {
        "overview"
    } else {
        "full"
    };
    let title = format!(
        " Mermaid #{} · {ready} · {mode} · {} · {}",
        viewer.ordinal + 1,
        if viewer
            .output
            .as_ref()
            .is_some_and(|output| !output.capabilities.direction)
        {
            "spacing n/a".to_string()
        } else {
            format!(
                "{} {}",
                if viewer.view.compact {
                    "compact"
                } else {
                    "configured"
                },
                app.diagram_spacing()
            )
        },
        if viewer.view.alternate {
            "alternate direction"
        } else {
            "authored direction"
        }
    );
    f.render_widget(
        Paragraph::new(slice_display_columns(&title, 0, area.width as usize)).style(
            Style::default()
                .fg(theme.markdown.code_label)
                .add_modifier(Modifier::BOLD),
        ),
        sections[0],
    );

    let rows: Vec<&str> = if viewer.source_mode {
        viewer.source.lines().collect()
    } else {
        viewer
            .output
            .as_ref()
            .map(|o| o.rows.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    if viewer.help {
        let help = [
            "Diagram controls (Esc or ? returns)",
            "",
            "Arrows / hjkl     Pan horizontally and vertically",
            "PageUp/PageDown   Pan one screen; Home resets",
            "Tab / Shift-Tab   Select next/previous visible node or group",
            "/                Search every original node/group by ID or label",
            "c                Toggle configured / compact 8,15,8 spacing",
            "Spacing          Node = siblings; rank = flow; edge = routing lanes",
            "Cell gaps        ceil(units/5), at least 4 columns / 3 rows",
            "d                Toggle authored / alternate direction",
            "o                Full graph / top-level group overview",
            "Space            Expand or collapse selected group",
            "a                Show or hide class member compartments",
            "f                Selected node's one-hop focus, both directions",
            "s                Original Mermaid source / diagram",
            "y                Copy original Mermaid source",
            "Esc / q          Close viewer; document position is preserved",
            "",
            "Hidden search results are revealed before centering.",
            "Unsupported controls explain the diagram-family limitation.",
        ];
        let lines = help
            .iter()
            .map(|line| Line::from(slice_display_columns(line, 0, sections[1].width as usize)))
            .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(lines).style(Style::default().fg(theme.markdown.mermaid_block_fg)),
            sections[1],
        );
    } else if rows.is_empty() {
        let message = viewer
            .error
            .as_deref()
            .unwrap_or("Preparing diagram in background…  s: original source  Esc: close");
        // Error text is prose, but it is still clipped so no canvas row ever wraps.
        f.render_widget(
            Paragraph::new(slice_display_columns(
                message,
                0,
                sections[1].width as usize,
            ))
            .style(Style::default().fg(theme.markdown.mermaid_block_fg)),
            sections[1],
        );
    } else {
        let lines: Vec<Line<'_>> = rows
            .iter()
            .skip(viewer.y)
            .take(sections[1].height as usize)
            .map(|row| {
                Line::from(slice_display_columns(
                    row,
                    viewer.x,
                    viewer.x + sections[1].width as usize,
                ))
            })
            .collect();
        f.render_widget(
            Paragraph::new(lines).style(Style::default().fg(theme.markdown.mermaid_block_fg)),
            sections[1],
        );
    }

    if !viewer.source_mode && !viewer.help {
        if let Some(object) = viewer.output.as_ref().and_then(|o| {
            o.objects
                .iter()
                .find(|o| Some(&o.id) == viewer.selected.as_ref())
        }) {
            let rect = object.bounds;
            let left = rect.x.max(viewer.x);
            let right = rect
                .x
                .saturating_add(rect.width)
                .min(viewer.x + sections[1].width as usize);
            let top = rect.y.max(viewer.y);
            let bottom = rect
                .y
                .saturating_add(rect.height)
                .min(viewer.y + sections[1].height as usize);
            for y in top..bottom {
                for x in left..right {
                    if let Some(cell) = f.buffer_mut().cell_mut((
                        sections[1].x + (x - viewer.x) as u16,
                        sections[1].y + (y - viewer.y) as u16,
                    )) {
                        cell.set_style(Style::default().bg(theme.markdown.search_highlight_bg));
                    }
                }
            }
        }
    }

    let (width, height) = viewer.extent();
    let selected = viewer
        .selected
        .as_ref()
        .map(|o| o.id.as_str())
        .unwrap_or("none");
    let counts = viewer
        .output
        .as_ref()
        .map(|o| {
            format!(
                " nodes {}/{} hidden {} boundary {}",
                o.visible_nodes, o.original_nodes, o.hidden_nodes, o.boundary_edges
            )
        })
        .unwrap_or_default();
    let stats = format!(
        " {width}×{height} cells x={} y={}{counts}",
        viewer.x, viewer.y
    );
    let detail = if let Some(query) = &viewer.search {
        let matches = viewer.search_matches();
        let selected = matches.get(viewer.search_index);
        format!(
            " /{query}  [{}/{}] {}",
            if matches.is_empty() {
                0
            } else {
                viewer.search_index + 1
            },
            matches.len(),
            selected
                .map(|e| format!("{}: {}", e.id.id, e.label.replace('\n', " ")))
                .unwrap_or_default()
        )
    } else if let Some(error) = &viewer.error {
        format!(" Error: {error} · s: source")
    } else if let Some(feedback) = &viewer.feedback {
        feedback.clone()
    } else if let Some(warning) = viewer.output.as_ref().and_then(|o| o.warnings.first()) {
        format!(" Warning: {warning}")
    } else {
        viewer
            .output
            .as_ref()
            .map(|o| format!("selected {selected} · {}", o.summary))
            .unwrap_or_else(|| " Layout runs off the UI thread; source stays available".into())
    };
    let controls = if viewer.search.is_some() {
        " ↑↓ matches · Enter reveal/center · Esc cancel"
    } else {
        " hjkl pan · / search · o overview · ? keys · s source · Esc close"
    };
    let footer = [stats, detail, controls.into()]
        .into_iter()
        .map(|line| Line::from(slice_display_columns(&line, 0, area.width as usize)))
        .collect::<Vec<_>>();
    f.render_widget(
        Paragraph::new(footer).style(Style::default().fg(theme.markdown.text)),
        sections[2],
    );
}
