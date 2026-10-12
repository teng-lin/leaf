use crate::theme::MarkdownTheme;
use mmdflux::{prepare_text_diagram, render_prepared_text, RenderConfig, TextLimits, TextRequest};
use ratatui::{style::Style, text::Span};
use std::{collections::HashMap, fmt::Write, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MermaidOptions {
    pub(crate) node_spacing: f64,
    pub(crate) rank_spacing: f64,
    pub(crate) edge_spacing: f64,
}

impl Default for MermaidOptions {
    fn default() -> Self {
        Self {
            node_spacing: 50.0,
            rank_spacing: 50.0,
            edge_spacing: 20.0,
        }
    }
}

impl std::fmt::Display for MermaidOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{}/{}",
            self.node_spacing, self.rank_spacing, self.edge_spacing
        )
    }
}

impl MermaidOptions {
    pub(crate) const COMPACT: Self = Self {
        node_spacing: 8.0,
        rank_spacing: 15.0,
        edge_spacing: 8.0,
    };

    pub(crate) fn render_config(self) -> RenderConfig {
        let mut config = RenderConfig {
            text_layout_spacing: true,
            ..Default::default()
        };
        config.layout.node_sep = self.node_spacing;
        config.layout.rank_sep = self.rank_spacing;
        config.layout.edge_sep = self.edge_spacing;
        config
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MermaidPreview {
    pub(crate) text: String,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) warning: Option<String>,
    /// Framing candidates for graph views only. Sequence participant boxes do
    /// not bound the messages below them and must not truncate their timeline.
    pub(crate) nodes: Vec<mmdflux::TextCellRect>,
    pub(crate) overview: bool,
    pub(crate) hidden_nodes: usize,
}

impl MermaidPreview {
    /// A grouped overview is useful before a large authored graph is expanded.
    /// Never infer groups or change the authored direction to manufacture one.
    pub(crate) fn use_overview(prepared: &mmdflux::PreparedTextDiagram) -> bool {
        prepared.capabilities().collapse
            && prepared
                .catalog()
                .iter()
                .filter(|entry| matches!(entry.id, mmdflux::TextObjectId::Node(_)))
                .count()
                >= 50
            && prepared
                .catalog()
                .iter()
                .filter(|entry| {
                    matches!(entry.id, mmdflux::TextObjectId::Subgraph(_))
                        && entry.ancestors.is_empty()
                })
                .count()
                >= 2
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.text.len()
            + self.warning.as_ref().map_or(0, String::len)
            + self.nodes.len() * std::mem::size_of::<mmdflux::TextCellRect>()
    }

    /// Choose a window containing complete nodes, not the potentially empty
    /// canvas origin. Ties prefer fewer cut boxes, then normal reading order.
    pub(crate) fn preview_origin(&self, width: usize, height: usize) -> (usize, usize) {
        if (self.width <= width || self.overview) && self.height <= height {
            return (0, 0);
        }
        let mut best = None;
        for anchor in &self.nodes {
            let x = anchor
                .x
                .saturating_sub(2)
                .min(self.width.saturating_sub(width));
            let y = anchor.y.saturating_sub(1);
            let mut complete = 0usize;
            let mut partial = 0usize;
            for node in &self.nodes {
                let right = node.x.saturating_add(node.width);
                let bottom = node.y.saturating_add(node.height);
                if node.x >= x
                    && right <= x.saturating_add(width)
                    && node.y >= y
                    && bottom <= y.saturating_add(height)
                {
                    complete += 1;
                } else if node.x < x.saturating_add(width)
                    && right > x
                    && node.y < y.saturating_add(height)
                    && bottom > y
                {
                    partial += 1;
                }
            }
            let score = (std::cmp::Reverse(complete), partial, y, x);
            if best.is_none_or(|previous| score < previous) {
                best = Some(score);
            }
        }
        best.map_or((0, 0), |(_, _, y, x)| (x, y))
    }
}

pub(crate) type MermaidCache = HashMap<String, Result<Arc<MermaidPreview>, String>>;

/// `Some(cache)` is strictly cache-only: an interactive parse must never solve.
#[derive(Clone, Copy, Default)]
pub(crate) struct MermaidRenderContext<'a> {
    pub(crate) options: MermaidOptions,
    pub(crate) cache: Option<&'a MermaidCache>,
    pub(crate) complete: bool,
}

pub(crate) fn render_mermaid_preview(
    content: &str,
    options: MermaidOptions,
    complete: bool,
) -> Result<MermaidPreview, String> {
    let limits = TextLimits::default();
    if content.len() > limits.max_source_bytes {
        return Err("Mermaid source exceeds the input limit".into());
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("Empty Mermaid diagram".into());
    }
    if trimmed.split_whitespace().next() == Some("pie") {
        let text = render_pie(trimmed).ok_or("Invalid pie diagram")?;
        return Ok(MermaidPreview {
            width: text
                .lines()
                .map(super::width::display_width)
                .max()
                .unwrap_or(0),
            height: text.lines().count(),
            text,
            ..Default::default()
        });
    }
    let prepared = prepare_text_diagram(content, &limits).map_err(|e| e.to_string())?;
    let overview = !complete && MermaidPreview::use_overview(&prepared);
    let result = render_prepared_text(
        &prepared,
        &TextRequest {
            config: options.render_config(),
            view: mmdflux::TextView {
                overview,
                ..Default::default()
            },
            limits,
            ..TextRequest::default()
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(MermaidPreview {
        text: result.text,
        width: result.width,
        height: result.height,
        warning: (!result.diagnostics.is_empty()).then(|| result.diagnostics.join("; ")),
        nodes: result
            .objects
            .iter()
            .filter(|object| {
                prepared.capabilities().focus
                    && (overview || matches!(object.id, mmdflux::TextObjectId::Node(_)))
            })
            .map(|object| object.rect)
            .collect(),
        overview,
        hidden_nodes: result.accounting.hidden_nodes(),
    })
}

fn render_pie(content: &str) -> Option<String> {
    let mut title = String::new();
    let mut entries: Vec<(String, f64)> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("pie") {
            let rest = rest.trim();
            if rest.is_empty() {
                continue;
            }
            if let Some(t) = rest.strip_prefix("title") {
                title = t.trim().to_string();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("title") {
            title = rest.trim().to_string();
            continue;
        }
        if let Some((label_part, value_part)) = line.rsplit_once(':') {
            let label = label_part.trim().trim_matches('"').to_string();
            if let Ok(value) = value_part.trim().parse::<f64>() {
                if value.is_finite() && value >= 0.0 {
                    entries.push((label, value));
                }
            }
        }
    }

    if entries.is_empty() {
        return None;
    }

    let total: f64 = entries.iter().map(|(_, v)| *v).sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }

    let max_label_width = entries
        .iter()
        .map(|(l, _)| super::width::display_width(l))
        .max()
        .unwrap_or(0);
    let bar_max = 32;
    let limits = TextLimits::default();
    let width = max_label_width
        .saturating_add(bar_max + 10)
        .max(super::width::display_width(&title));
    if entries.len() > limits.max_nodes
        || width > limits.max_canvas_dimension
        || width
            .checked_mul(entries.len() + 1)
            .is_none_or(|cells| cells > limits.max_canvas_cells)
    {
        return None;
    }
    let mut out = String::new();

    if !title.is_empty() {
        let _ = writeln!(out, "{title}");
    }

    for (label, value) in &entries {
        let pct = value / total * 100.0;
        let bar_units = pct / 100.0 * bar_max as f64;
        let filled = bar_units as usize;
        let half = (bar_units * 2.0) as usize % 2 == 1;
        let bar: String = "█".repeat(filled) + if half { "▌" } else { "" };
        let bar_pad = " ".repeat((bar_max + 1).saturating_sub(super::width::display_width(&bar)));
        let label_pad =
            " ".repeat(max_label_width.saturating_sub(super::width::display_width(label)));
        let _ = writeln!(out, "{bar}{bar_pad} {label}{label_pad} {pct:>5.1}%");
    }

    Some(out)
}

pub(crate) fn colorize_line(line: &str, theme: &MarkdownTheme) -> Vec<Span<'static>> {
    let keyword_style = Style::default().fg(theme.mermaid_keyword);
    let arrow_style = Style::default().fg(theme.mermaid_arrow);
    let label_style = Style::default().fg(theme.mermaid_label);
    let default_style = Style::default().fg(theme.mermaid_block_fg);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut rest = line;

    while !rest.is_empty() {
        if let Some(pos) = rest.find('|') {
            let before = &rest[..pos];
            if !before.is_empty() {
                tokenize_segment(
                    before,
                    keyword_style,
                    arrow_style,
                    default_style,
                    &mut spans,
                );
            }
            let after_pipe = &rest[pos + 1..];
            if let Some(end) = after_pipe.find('|') {
                let label_content = &after_pipe[..end];
                spans.push(Span::styled(format!("|{label_content}|"), label_style));
                rest = &after_pipe[end + 1..];
            } else {
                spans.push(Span::styled("|".to_string(), default_style));
                rest = after_pipe;
            }
            continue;
        }

        tokenize_segment(rest, keyword_style, arrow_style, default_style, &mut spans);
        break;
    }

    if spans.is_empty() {
        spans.push(Span::styled(line.to_string(), default_style));
    }

    spans
}

fn tokenize_segment(
    segment: &str,
    keyword_style: Style,
    arrow_style: Style,
    default_style: Style,
    spans: &mut Vec<Span<'static>>,
) {
    let mut i = 0;
    let bytes = segment.as_bytes();

    while i < bytes.len() {
        if let Some((arrow, len)) = try_match_arrow(&segment[i..]) {
            spans.push(Span::styled(arrow, arrow_style));
            i += len;
            continue;
        }

        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
            {
                i += 1;
            }
            let word = &segment[start..i];
            if is_keyword(word) {
                spans.push(Span::styled(word.to_string(), keyword_style));
            } else {
                spans.push(Span::styled(word.to_string(), default_style));
            }
            continue;
        }

        let start = i;
        while i < segment.len() {
            let b = bytes[i];
            if b.is_ascii_alphabetic() || b == b'_' || b == b'|' {
                break;
            }
            if try_match_arrow(&segment[i..]).is_some() {
                break;
            }
            if b < 0x80 {
                i += 1;
            } else {
                let ch = segment[i..].chars().next().unwrap();
                i += ch.len_utf8();
            }
        }
        if i > start {
            spans.push(Span::styled(segment[start..i].to_string(), default_style));
        }
    }
}

fn try_match_arrow(s: &str) -> Option<(String, usize)> {
    for pattern in &["-.->", "==>", "-->", "---", "-.-", "-..", "->", "--"] {
        if s.starts_with(pattern) {
            return Some((pattern.to_string(), pattern.len()));
        }
    }
    None
}

fn is_keyword(word: &str) -> bool {
    is_diagram_keyword(word) || is_direction_keyword(word) || is_structure_keyword(word)
}

fn is_diagram_keyword(word: &str) -> bool {
    matches!(
        word,
        "flowchart"
            | "graph"
            | "sequenceDiagram"
            | "classDiagram"
            | "stateDiagram"
            | "stateDiagram-v2"
            | "erDiagram"
            | "gantt"
            | "pie"
            | "journey"
            | "gitGraph"
            | "mindmap"
            | "timeline"
            | "sankey-beta"
            | "quadrantChart"
            | "requirementDiagram"
            | "C4Context"
            | "block-beta"
            | "xychart-beta"
            | "kanban"
            | "architecture-beta"
    )
}

fn is_direction_keyword(word: &str) -> bool {
    matches!(word, "TB" | "TD" | "BT" | "LR" | "RL")
}

fn is_structure_keyword(word: &str) -> bool {
    matches!(
        word,
        "subgraph"
            | "end"
            | "section"
            | "title"
            | "participant"
            | "actor"
            | "loop"
            | "alt"
            | "else"
            | "opt"
            | "par"
            | "critical"
            | "break"
            | "rect"
            | "note"
            | "activate"
            | "deactivate"
            | "class"
            | "state"
            | "dateFormat"
            | "axisFormat"
            | "style"
            | "classDef"
            | "click"
    )
}
