use super::{diagram_worker::*, App};
use crate::markdown::{DiagramBlockInfo, MermaidCache, MermaidOptions, MermaidRenderContext};
use ratatui::layout::Rect;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};
use syntect::{highlighting::ThemeSet, parsing::SyntaxSet};

const PREVIEW_CACHE_BYTES: usize = 8 * 1024 * 1024;
const PREVIEW_CACHE_ENTRIES: usize = 32;

pub(crate) struct DiagramState {
    pub(crate) blocks: Vec<DiagramBlockInfo>,
    pub(crate) options: MermaidOptions,
    pub(crate) previews: MermaidCache,
    preview_order: VecDeque<String>,
    service: Option<DiagramService>,
    document: u64,
    next_request: u64,
    pub(crate) viewer: Option<DiagramViewer>,
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            options: MermaidOptions::default(),
            previews: MermaidCache::new(),
            preview_order: VecDeque::new(),
            service: None,
            document: 1,
            next_request: 0,
            viewer: None,
        }
    }
}

pub(crate) struct DiagramViewer {
    pub(crate) source: String,
    pub(crate) ordinal: usize,
    pub(crate) view: ViewOptions,
    pub(crate) output: Option<Arc<DiagramOutput>>,
    pub(crate) error: Option<String>,
    pub(crate) loading: bool,
    pub(crate) request: u64,
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) area: Rect,
    pub(crate) source_mode: bool,
    pub(crate) help: bool,
    source_offsets: (usize, usize),
    diagram_offsets: (usize, usize),
    pub(crate) selected: Option<ObjectId>,
    pub(crate) search: Option<String>,
    pub(crate) search_index: usize,
    pub(crate) feedback: Option<String>,
    resize_started: Option<Instant>,
    center_after_render: bool,
    initial_frame: bool,
    auto_overview: bool,
}

impl DiagramViewer {
    pub(crate) fn extent(&self) -> (usize, usize) {
        if self.source_mode {
            (
                self.source
                    .lines()
                    .map(crate::markdown::display_width)
                    .max()
                    .unwrap_or(0),
                self.source.lines().count(),
            )
        } else {
            self.output.as_ref().map_or((0, 0), |o| (o.width, o.height))
        }
    }

    pub(crate) fn clamp(&mut self) {
        let (width, height) = self.extent();
        let (x_limit, y_limit) = if self.source_mode {
            (
                width.saturating_sub(self.area.width as usize),
                height.saturating_sub(self.area.height as usize),
            )
        } else {
            Self::diagram_pan_limits(width, height, self.area)
        };
        self.x = self.x.min(x_limit);
        self.y = self.y.min(y_limit);
    }

    fn diagram_pan_limits(width: usize, height: usize, area: Rect) -> (usize, usize) {
        // Allow half a viewport of trailing space so edge nodes can actually
        // be centered. Source mode retains ordinary document scroll limits.
        (
            width.saturating_sub((area.width as usize / 2).max(1)),
            height.saturating_sub((area.height as usize / 2).max(1)),
        )
    }

    pub(crate) fn search_matches(&self) -> Vec<&CatalogObject> {
        let Some(output) = self.output.as_ref() else {
            return Vec::new();
        };
        let query = self.search.as_deref().unwrap_or("").to_lowercase();
        output
            .catalog
            .iter()
            .filter(|entry| {
                entry.id.id.to_lowercase().contains(&query)
                    || entry.label.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn center_selection(&mut self) {
        if self.initial_frame && self.area.width > 0 && self.area.height > 0 {
            self.initial_frame = false;
            if let Some((selected, x, y)) = self.output.as_ref().and_then(|output| {
                output.initial_view(self.area.width as usize, self.area.height as usize)
            }) {
                self.selected = Some(selected);
                if self.source_mode {
                    if let Some(output) = &self.output {
                        let (x_limit, y_limit) =
                            Self::diagram_pan_limits(output.width, output.height, self.area);
                        self.diagram_offsets = (x.min(x_limit), y.min(y_limit));
                    }
                } else {
                    self.x = x;
                    self.y = y;
                    self.clamp();
                }
                return;
            }
        }
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some(object) = self
            .output
            .as_ref()
            .and_then(|o| o.objects.iter().find(|o| &o.id == selected))
        else {
            return;
        };
        let x = (object.bounds.x + object.bounds.width / 2)
            .saturating_sub(self.area.width as usize / 2);
        let y = (object.bounds.y + object.bounds.height / 2)
            .saturating_sub(self.area.height as usize / 2);
        if self.source_mode {
            if let Some(output) = &self.output {
                let (x_limit, y_limit) =
                    Self::diagram_pan_limits(output.width, output.height, self.area);
                self.diagram_offsets = (x.min(x_limit), y.min(y_limit));
            }
        } else {
            self.x = x;
            self.y = y;
            self.clamp();
        }
    }
}

impl App {
    pub(crate) fn set_mermaid_options(&mut self, options: MermaidOptions) {
        if self.diagrams.options != options {
            self.diagrams.options = options;
            self.diagrams.previews.clear();
            self.diagrams.preview_order.clear();
            self.reset_diagram_document();
        }
    }

    pub(crate) fn set_diagrams(&mut self, blocks: Vec<DiagramBlockInfo>) {
        self.diagrams.blocks = blocks;
    }

    pub(crate) fn mermaid_context(&self) -> MermaidRenderContext<'_> {
        MermaidRenderContext {
            options: self.diagrams.options,
            cache: Some(&self.diagrams.previews),
            complete: false,
        }
    }

    pub(crate) fn reset_diagram_document(&mut self) {
        self.diagrams.document = self.diagrams.document.wrapping_add(1);
        self.diagrams.viewer = None;
        if let Some(service) = &self.diagrams.service {
            service.cancel_all();
        }
    }

    pub(crate) fn is_diagram_open(&self) -> bool {
        self.diagrams.viewer.is_some()
    }

    pub(crate) fn is_diagram_search(&self) -> bool {
        self.diagrams
            .viewer
            .as_ref()
            .is_some_and(|v| v.search.is_some())
    }

    pub(crate) fn diagram_viewer(&self) -> Option<&DiagramViewer> {
        self.diagrams.viewer.as_ref()
    }

    pub(crate) fn diagram_spacing(&self) -> MermaidOptions {
        if self
            .diagrams
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.view.compact)
        {
            MermaidOptions::COMPACT
        } else {
            self.diagrams.options
        }
    }

    pub(crate) fn open_diagram(&mut self) {
        let selected = self.code_select.and_then(|index| {
            self.diagrams
                .blocks
                .iter()
                .find(|d| d.code_block_index == index)
        });
        let block = selected.or_else(|| {
            self.diagrams
                .blocks
                .iter()
                .find(|d| d.rendered_end >= self.scroll && d.rendered_start < self.visible_end())
        });
        let Some(block) = block else {
            self.set_config_warning(Some("No Mermaid diagram in view".into()));
            return;
        };
        self.diagrams.viewer = Some(DiagramViewer {
            source: block.source.clone(),
            ordinal: block.ordinal,
            view: ViewOptions {
                width: (self.content_area.x as usize + self.content_area.width as usize)
                    .max(self.render_width),
                ..ViewOptions::default()
            },
            output: None,
            error: None,
            loading: true,
            request: 0,
            x: 0,
            y: 0,
            area: Rect::default(),
            source_mode: false,
            help: false,
            source_offsets: (0, 0),
            diagram_offsets: (0, 0),
            selected: None,
            search: None,
            search_index: 0,
            feedback: None,
            resize_started: None,
            center_after_render: false,
            initial_frame: true,
            auto_overview: true,
        });
        self.scrollbar_dragging = false;
        self.hovered_link = None;
        self.last_click = None;
        if self
            .diagrams
            .viewer
            .as_ref()
            .is_some_and(|v| v.source.len() > mmdflux::TextLimits::default().max_source_bytes)
        {
            if let Some(viewer) = self.diagrams.viewer.as_mut() {
                viewer.loading = false;
                viewer.error =
                    Some("Mermaid source exceeds the input limit; s shows original source".into());
            }
            return;
        }
        self.request_diagram_render();
    }

    pub(crate) fn close_diagram(&mut self) {
        self.diagrams.viewer = None;
        self.diagrams.next_request = self.diagrams.next_request.wrapping_add(1);
        if let Some(service) = &self.diagrams.service {
            service.cancel_foreground();
        }
    }

    pub(crate) fn toggle_diagram_help(&mut self) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            viewer.help = !viewer.help;
        }
    }

    fn submit_diagram_job(&mut self, job: DiagramJob) {
        self.diagrams
            .service
            .get_or_insert_with(DiagramService::start)
            .submit(job);
    }

    fn request_diagram_render(&mut self) {
        let Some(viewer) = self.diagrams.viewer.as_mut() else {
            return;
        };
        if viewer.source.len() > mmdflux::TextLimits::default().max_source_bytes {
            viewer.loading = false;
            viewer.resize_started = None;
            viewer.error =
                Some("Mermaid source exceeds the input limit; s shows original source".into());
            return;
        }
        self.diagrams.next_request = self.diagrams.next_request.wrapping_add(1);
        viewer.request = self.diagrams.next_request;
        viewer.loading = true;
        viewer.error = None;
        viewer.resize_started = None;
        let job = DiagramJob {
            document: self.diagrams.document,
            request: viewer.request,
            ordinal: viewer.ordinal,
            source: viewer.source.clone(),
            options: self.diagrams.options,
            view: Some(viewer.view.clone()),
        };
        self.submit_diagram_job(job);
    }

    pub(crate) fn has_diagram_work(&self) -> bool {
        self.diagrams
            .service
            .as_ref()
            .is_some_and(DiagramService::busy)
            || self
                .diagrams
                .viewer
                .as_ref()
                .is_some_and(|v| v.resize_started.is_some())
    }

    /// Called from the event loop, never from rendering a frame.
    pub(crate) fn poll_diagrams(&mut self, ss: &SyntaxSet, themes: &ThemeSet) -> bool {
        let completions = self
            .diagrams
            .service
            .as_ref()
            .map(DiagramService::drain)
            .unwrap_or_default();
        let mut changed = false;
        let mut previews_changed = false;
        for completion in completions {
            if completion.job.document != self.diagrams.document {
                continue;
            }
            if completion.job.view.is_none() {
                if self
                    .diagrams
                    .blocks
                    .iter()
                    .any(|b| b.source == completion.job.source)
                {
                    let result = completion.result.map(|output| Arc::new(output.preview()));
                    self.cache_diagram_preview(completion.job.source, result);
                    previews_changed = true;
                }
                continue;
            }
            let Some(viewer) = self.diagrams.viewer.as_mut() else {
                continue;
            };
            if completion.job.request != viewer.request
                || completion.job.ordinal != viewer.ordinal
                || completion.job.source != viewer.source
            {
                continue;
            }
            viewer.loading = false;
            match completion.result {
                Ok(output) => {
                    if !viewer.view.alternate {
                        viewer.view.authored_direction = output.direction;
                    }
                    if viewer.output.is_none() {
                        viewer.center_after_render = true;
                    }
                    if !output
                        .objects
                        .iter()
                        .any(|object| Some(&object.id) == viewer.selected.as_ref())
                    {
                        // When overview/collapse hides a selected node, select its
                        // nearest visible container instead of retaining a phantom ID.
                        let representative = viewer
                            .selected
                            .as_ref()
                            .and_then(|selected| {
                                output.catalog.iter().find(|entry| &entry.id == selected)
                            })
                            .and_then(|entry| {
                                entry.ancestors.iter().find_map(|ancestor| {
                                    output.objects.iter().find(|object| {
                                        object.id.kind == ObjectKind::Subgraph
                                            && object.id.id == *ancestor
                                    })
                                })
                            });
                        viewer.selected = representative
                            .or_else(|| {
                                output
                                    .objects
                                    .iter()
                                    .find(|o| o.id.kind == ObjectKind::Node)
                            })
                            .or_else(|| output.objects.first())
                            .map(|o| o.id.clone());
                    }
                    viewer.output = Some(output);
                    viewer.error = None;
                    if viewer.center_after_render && viewer.area.width > 0 && viewer.area.height > 0
                    {
                        viewer.center_selection();
                        viewer.center_after_render = false;
                    }
                    viewer.clamp();
                }
                Err(error) => {
                    viewer.error = Some(error);
                }
            }
            changed = true;
        }
        // Decide using the rendered width and actual modal content width.
        // Vertical overflow is easy to scroll and does not require overview.
        let mut request_overview = false;
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            if viewer.auto_overview
                && !viewer.loading
                && viewer.area.width > 0
                && viewer.area.height > 0
            {
                if let Some(output) = &viewer.output {
                    viewer.auto_overview = false;
                    if output.width > viewer.area.width as usize && output.capabilities.collapse {
                        viewer.view.overview = true;
                        viewer.initial_frame = true;
                        viewer.center_after_render = true;
                        request_overview = true;
                    } else if output.width <= viewer.area.width as usize {
                        viewer.x = 0;
                        viewer.y = 0;
                        viewer.initial_frame = false;
                        viewer.center_after_render = false;
                    }
                    changed = true;
                }
            }
        }
        if request_overview {
            self.request_diagram_render();
        }
        if previews_changed {
            // A source anchor survives preview height changes; a ratio does not.
            let anchor = self.diagram_reflow_anchor();
            let selected = self.code_select;
            self.reparse_source(ss, themes);
            self.restore_diagram_source_anchor(anchor);
            self.code_select = selected.filter(|&i| i < self.code_blocks.len());
            changed = true;
        }
        if self.diagrams.viewer.as_ref().is_some_and(|v| {
            v.resize_started
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(120))
        }) {
            if let Some(viewer) = self.diagrams.viewer.as_mut() {
                viewer.view.width = viewer.area.width.max(1) as usize;
            }
            self.request_diagram_render();
            changed = true;
        }
        if self.diagrams.viewer.is_none() && !self.has_diagram_work() {
            let next = self.diagrams.blocks.iter().find(|block| {
                block.rendered_end >= self.scroll
                    && block.rendered_start < self.visible_end().saturating_add(20)
                    && block.source.len() <= mmdflux::TextLimits::default().max_source_bytes
                    && !self.diagrams.previews.contains_key(&block.source)
            });
            if let Some(block) = next {
                self.diagrams.next_request = self.diagrams.next_request.wrapping_add(1);
                let job = DiagramJob {
                    document: self.diagrams.document,
                    request: self.diagrams.next_request,
                    ordinal: block.ordinal,
                    source: block.source.clone(),
                    options: self.diagrams.options,
                    view: None,
                };
                self.submit_diagram_job(job);
                changed = true;
            }
        }
        changed
    }

    fn cache_diagram_preview(
        &mut self,
        source: String,
        mut result: Result<Arc<crate::markdown::MermaidPreview>, String>,
    ) {
        let result_size = |r: &Result<Arc<crate::markdown::MermaidPreview>, String>| match r {
            Ok(p) => p.estimated_bytes(),
            Err(e) => e.len(),
        };
        if source.len() + result_size(&result) > PREVIEW_CACHE_BYTES / 2 {
            result = Err("Diagram exceeds preview cache budget; press v for bounded full-view rendering or s for source".into());
        }
        self.diagrams.preview_order.retain(|key| key != &source);
        self.diagrams.previews.remove(&source);
        while self.diagrams.previews.len() >= PREVIEW_CACHE_ENTRIES
            || self
                .diagrams
                .previews
                .iter()
                .map(|(k, v)| k.len() + result_size(v))
                .sum::<usize>()
                + source.len()
                + result_size(&result)
                > PREVIEW_CACHE_BYTES
        {
            let Some(oldest) = self.diagrams.preview_order.pop_front() else {
                break;
            };
            self.diagrams.previews.remove(&oldest);
        }
        self.diagrams.preview_order.push_back(source.clone());
        self.diagrams.previews.insert(source, result);
    }

    pub(super) fn restore_diagram_source_anchor(&mut self, anchor: usize) {
        if let Some(block) = self
            .diagrams
            .blocks
            .iter()
            .find(|block| block.source_line == anchor)
        {
            self.scroll = block.rendered_start.min(self.max_scroll());
            return;
        }
        if let Some(index) = self.source_line_map.iter().position(|&line| line >= anchor) {
            self.scroll = index.min(self.max_scroll());
        }
    }

    pub(super) fn diagram_reflow_anchor(&self) -> usize {
        self.diagrams
            .blocks
            .iter()
            .find(|block| self.scroll >= block.rendered_start && self.scroll <= block.rendered_end)
            .map(|block| block.source_line)
            .unwrap_or_else(|| self.source_line_at(self.scroll))
    }

    pub(crate) fn set_diagram_area(&mut self, area: Rect) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            let width_changed = viewer.area.width != area.width;
            viewer.area = area;
            if viewer.center_after_render
                && !viewer.loading
                && viewer.error.is_none()
                && viewer.output.is_some()
                && area.width > 0
                && area.height > 0
            {
                viewer.center_selection();
                viewer.center_after_render = false;
            }
            viewer.clamp();
            if width_changed && area.width > 0 && area.width as usize != viewer.view.width {
                viewer.resize_started = Some(Instant::now());
            }
        }
    }

    pub(crate) fn pan_diagram(&mut self, dx: isize, dy: isize) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            viewer.x = viewer.x.saturating_add_signed(dx);
            viewer.y = viewer.y.saturating_add_signed(dy);
            viewer.clamp();
        }
    }

    pub(crate) fn reset_diagram_pan(&mut self) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            viewer.x = 0;
            viewer.y = 0;
        }
    }

    pub(crate) fn diagram_page(&mut self, down: bool) {
        let step = self
            .diagrams
            .viewer
            .as_ref()
            .map_or(1, |v| v.area.height.max(1) as isize);
        self.pan_diagram(0, if down { step } else { -step });
    }

    pub(crate) fn select_next_diagram_object(&mut self, backwards: bool) {
        let Some(viewer) = self.diagrams.viewer.as_mut() else {
            return;
        };
        let Some(output) = viewer.output.as_ref() else {
            return;
        };
        if output.objects.is_empty() {
            return;
        }
        let current = output
            .objects
            .iter()
            .position(|o| Some(&o.id) == viewer.selected.as_ref());
        let length = output.objects.len();
        let next = match (current, backwards) {
            (Some(i), true) => (i + length - 1) % length,
            (Some(i), false) => (i + 1) % length,
            (None, _) => 0,
        };
        viewer.selected = Some(output.objects[next].id.clone());
        viewer.center_selection();
    }

    pub(crate) fn select_diagram_at(&mut self, column: u16, row: u16) {
        let Some(viewer) = self.diagrams.viewer.as_mut() else {
            return;
        };
        if viewer.source_mode || !viewer.area.contains((column, row).into()) {
            return;
        }
        let x = viewer.x + (column - viewer.area.x) as usize;
        let y = viewer.y + (row - viewer.area.y) as usize;
        let selected = viewer.output.as_ref().and_then(|o| {
            o.objects
                .iter()
                .filter(|o| {
                    x >= o.bounds.x
                        && x < o.bounds.x.saturating_add(o.bounds.width)
                        && y >= o.bounds.y
                        && y < o.bounds.y.saturating_add(o.bounds.height)
                })
                .min_by_key(|o| o.bounds.width.saturating_mul(o.bounds.height))
        });
        if let Some(object) = selected {
            viewer.selected = Some(object.id.clone());
        }
    }

    pub(crate) fn begin_diagram_search(&mut self) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            viewer.search = Some(String::new());
            viewer.search_index = 0;
        }
    }

    pub(crate) fn edit_diagram_search(&mut self, character: Option<char>) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            if let Some(query) = viewer.search.as_mut() {
                if let Some(c) = character {
                    if query.len() < 1024 {
                        query.push(c);
                    }
                } else {
                    query.pop();
                }
                viewer.search_index = 0;
            }
        }
    }

    pub(crate) fn move_diagram_search(&mut self, backwards: bool) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            let count = viewer.search_matches().len();
            if count > 0 {
                viewer.search_index = if backwards {
                    (viewer.search_index + count - 1) % count
                } else {
                    (viewer.search_index + 1) % count
                };
            }
        }
    }

    pub(crate) fn cancel_diagram_search(&mut self) {
        if let Some(viewer) = self.diagrams.viewer.as_mut() {
            viewer.search = None;
        }
    }

    pub(crate) fn confirm_diagram_search(&mut self) {
        let Some(viewer) = self.diagrams.viewer.as_mut() else {
            return;
        };
        let found = viewer
            .search_matches()
            .get(viewer.search_index)
            .map(|entry| (*entry).clone());
        let Some(entry) = found else {
            viewer.feedback = Some("No matching diagram node or group".into());
            return;
        };
        viewer.selected = Some(entry.id.clone());
        viewer.search = None;
        viewer.source_mode = false;
        let visible = viewer
            .output
            .as_ref()
            .is_some_and(|o| o.objects.iter().any(|o| o.id == entry.id));
        if visible {
            viewer.center_selection();
            return;
        }
        for ancestor in &entry.ancestors {
            viewer.view.collapsed.remove(ancestor);
            viewer.view.expanded.insert(ancestor.clone());
        }
        if entry.id.kind == ObjectKind::Subgraph {
            viewer.view.collapsed.remove(&entry.id.id);
            viewer.view.expanded.insert(entry.id.id.clone());
        }
        viewer.view.focus = None;
        viewer.center_after_render = true;
        self.request_diagram_render();
    }

    pub(crate) fn diagram_control(&mut self, control: char) {
        let Some(viewer) = self.diagrams.viewer.as_mut() else {
            return;
        };
        if control == 's' {
            if viewer.source_mode {
                viewer.source_offsets = (viewer.x, viewer.y);
                (viewer.x, viewer.y) = viewer.diagram_offsets;
            } else {
                viewer.diagram_offsets = (viewer.x, viewer.y);
                (viewer.x, viewer.y) = viewer.source_offsets;
            }
            viewer.source_mode = !viewer.source_mode;
            viewer.clamp();
            return;
        }
        if control == 'y' {
            viewer.feedback = Some(if crate::clipboard::copy_to_clipboard(&viewer.source) {
                "Original Mermaid copied".into()
            } else {
                "Could not copy original Mermaid".into()
            });
            return;
        }
        let caps = viewer
            .output
            .as_ref()
            .map(|o| o.capabilities)
            .unwrap_or_default();
        viewer.feedback = None;
        viewer.auto_overview = false;
        match control {
            'c' => viewer.view.compact = !viewer.view.compact,
            'd' if caps.direction => viewer.view.alternate = !viewer.view.alternate,
            'o' if caps.collapse => {
                viewer.view.overview = !viewer.view.overview;
                viewer.view.collapsed.clear();
                viewer.view.expanded.clear();
                viewer.view.focus = None;
            }
            'a' if caps.members => viewer.view.members = !viewer.view.members,
            'f' if caps.focus => {
                let Some(selected) = viewer
                    .selected
                    .as_ref()
                    .filter(|id| id.kind == ObjectKind::Node)
                else {
                    viewer.feedback = Some(
                        "Select an original node to focus; groups cannot be focus anchors".into(),
                    );
                    return;
                };
                viewer.view.focus = if viewer.view.focus.as_ref() == Some(&selected.id) {
                    None
                } else {
                    Some(selected.id.clone())
                };
                viewer.center_after_render = true;
            }
            ' ' if caps.collapse => {
                let Some(selected) = viewer
                    .selected
                    .as_ref()
                    .filter(|id| id.kind == ObjectKind::Subgraph)
                else {
                    viewer.feedback =
                        Some("Select a group with Tab or / before expanding/collapsing".into());
                    return;
                };
                let collapsed = viewer.view.collapsed.contains(&selected.id)
                    || (viewer.view.overview && !viewer.view.expanded.contains(&selected.id));
                if collapsed {
                    viewer.view.collapsed.remove(&selected.id);
                    viewer.view.expanded.insert(selected.id.clone());
                } else {
                    viewer.view.expanded.remove(&selected.id);
                    viewer.view.collapsed.insert(selected.id.clone());
                }
                viewer.center_after_render = true;
            }
            _ => {
                viewer.feedback = Some(
                    "This control is unavailable for this diagram family (or while preparing)"
                        .into(),
                );
                return;
            }
        }
        // Explicit view changes relayout the graph; keep the selected object
        // visible in its new position. Ordinary pan and resize do not set this.
        viewer.center_after_render = true;
        self.request_diagram_render();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::AppConfig,
        markdown::{parse_markdown_with_options, MermaidRenderContext},
        theme::app_theme,
    };
    use ratatui::{backend::TestBackend, Terminal};

    fn fixture() -> (App, SyntaxSet, ThemeSet) {
        let ss = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let source = "# Header\n\n```mermaid\nflowchart LR\nsubgraph G[Group]\nA[Alpha] --> B[Beta]\nend\n```\n\nTail";
        let mut app = App::new_with_source(
            Vec::new(),
            Vec::new(),
            AppConfig {
                filename: "diagrams.md".into(),
                source: source.into(),
                debug_input: false,
                watch: false,
                filepath: None,
                last_file_state: None,
            },
        );
        let parsed = parse_markdown_with_options(
            source,
            &ss,
            &themes.themes["base16-ocean.dark"],
            80,
            &app_theme().markdown,
            false,
            false,
            MermaidRenderContext {
                cache: Some(&MermaidCache::new()),
                ..Default::default()
            },
        );
        app.replace_content(parsed);
        app.content_area = Rect::new(0, 0, 80, 30);
        app.diagrams.service = Some(DiagramService::test_idle());
        (app, ss, themes)
    }

    fn id(kind: ObjectKind, id: &str) -> ObjectId {
        ObjectId {
            kind,
            id: id.into(),
        }
    }

    fn output(marker: &str) -> Arc<DiagramOutput> {
        Arc::new(DiagramOutput {
            rows: vec![format!("{marker}{}", "─".repeat(198)); 70],
            width: 200,
            height: 70,
            catalog: vec![
                CatalogObject {
                    id: id(ObjectKind::Subgraph, "G"),
                    label: "Group".into(),
                    ancestors: vec![],
                },
                CatalogObject {
                    id: id(ObjectKind::Node, "A"),
                    label: "Alpha".into(),
                    ancestors: vec!["G".into()],
                },
                CatalogObject {
                    id: id(ObjectKind::Node, "B"),
                    label: "Beta".into(),
                    ancestors: vec!["G".into()],
                },
            ],
            objects: vec![
                VisibleObject {
                    id: id(ObjectKind::Subgraph, "G"),
                    bounds: CellRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 50,
                    },
                },
                VisibleObject {
                    id: id(ObjectKind::Node, "A"),
                    bounds: CellRect {
                        x: 20,
                        y: 10,
                        width: 10,
                        height: 5,
                    },
                },
            ],
            capabilities: Capabilities {
                collapse: true,
                focus: true,
                direction: true,
                members: true,
            },
            direction: Some(mmdflux::graph::Direction::LeftRight),
            summary: marker.into(),
            warnings: vec![],
            overview: false,
            visible_nodes: 1,
            original_nodes: 2,
            hidden_nodes: 1,
            boundary_edges: 1,
        })
    }

    fn completion(app: &App, request: u64, marker: &str) -> DiagramCompletion {
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        DiagramCompletion {
            job: DiagramJob {
                document: app.diagrams.document,
                request,
                ordinal: viewer.ordinal,
                source: viewer.source.clone(),
                options: app.diagrams.options,
                view: Some(viewer.view.clone()),
            },
            result: Ok(output(marker)),
        }
    }

    fn make_ready(app: &mut App, ss: &SyntaxSet, themes: &ThemeSet) {
        let request = app.diagrams.viewer.as_ref().unwrap().request;
        app.diagrams
            .service
            .as_ref()
            .unwrap()
            .test_complete(completion(app, request, "ready"));
        assert!(app.poll_diagrams(ss, themes));
        app.set_diagram_area(Rect::new(0, 1, 80, 26));
    }

    #[test]
    fn diagram_initial_view_uses_width_and_preserves_manual_choice() {
        for (width, height, overview) in [(200, 70, false), (199, 70, true), (200, 20, false)] {
            let (mut app, ss, themes) = fixture();
            app.open_diagram();
            app.set_diagram_area(Rect::new(0, 1, width, height));
            let request = app.diagrams.viewer.as_ref().unwrap().request;
            app.diagrams
                .service
                .as_ref()
                .unwrap()
                .test_complete(completion(&app, request, "full"));
            app.poll_diagrams(&ss, &themes);
            let viewer = app.diagrams.viewer.as_ref().unwrap();
            assert_eq!(viewer.view.overview, overview);
            assert!(!viewer.auto_overview);
            if !overview {
                assert_eq!((viewer.x, viewer.y), (0, 0));
            } else {
                app.diagram_control('o');
                assert!(!app.diagrams.viewer.as_ref().unwrap().view.overview);
                app.poll_diagrams(&ss, &themes);
                assert!(!app.diagrams.viewer.as_ref().unwrap().view.overview);
            }
        }
    }

    #[test]
    fn diagram_modal_preserves_document_and_source_copy_selection() {
        let (mut app, ss, themes) = fixture();
        app.code_select = Some(app.diagrams.blocks[0].code_block_index);
        let selected = app.code_select;
        let scroll = app.scroll;
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        app.pan_diagram(1000, 1000);
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        assert_eq!((viewer.x, viewer.y), (160, 57));
        app.close_diagram();
        assert_eq!(app.scroll, scroll);
        assert_eq!(app.code_select, selected);
    }

    #[test]
    fn diagram_stale_a_cannot_replace_requested_b() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        let request_a = app.diagrams.viewer.as_ref().unwrap().request;
        let stale = completion(&app, request_a, "STALE-A");
        app.diagram_control('c');
        let request_b = app.diagrams.viewer.as_ref().unwrap().request;
        assert_ne!(request_a, request_b);
        app.diagrams.service.as_ref().unwrap().test_complete(stale);
        app.poll_diagrams(&ss, &themes);
        assert!(app.diagrams.viewer.as_ref().unwrap().output.is_none());
        app.diagrams
            .service
            .as_ref()
            .unwrap()
            .test_complete(completion(&app, request_b, "CURRENT-B"));
        app.poll_diagrams(&ss, &themes);
        assert_eq!(
            app.diagrams
                .viewer
                .as_ref()
                .unwrap()
                .output
                .as_ref()
                .unwrap()
                .summary,
            "CURRENT-B"
        );
    }

    #[test]
    fn diagram_closed_and_reloaded_completions_are_ignored() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        let stale = completion(
            &app,
            app.diagrams.viewer.as_ref().unwrap().request,
            "CLOSED",
        );
        app.close_diagram();
        app.diagrams.service.as_ref().unwrap().test_complete(stale);
        app.poll_diagrams(&ss, &themes);
        assert!(!app.is_diagram_open());
        app.open_diagram();
        let stale = completion(
            &app,
            app.diagrams.viewer.as_ref().unwrap().request,
            "OLD-DOCUMENT",
        );
        app.reset_diagram_document();
        app.open_diagram();
        app.diagrams.service.as_ref().unwrap().test_complete(stale);
        app.poll_diagrams(&ss, &themes);
        assert!(app.diagrams.viewer.as_ref().unwrap().output.is_none());
    }

    #[test]
    fn diagram_pan_visible_selection_and_search_do_not_request_layout() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        let request = app.diagrams.next_request;
        app.pan_diagram(10, 3);
        app.select_next_diagram_object(false);
        app.begin_diagram_search();
        for c in "Alpha".chars() {
            app.edit_diagram_search(Some(c));
        }
        app.confirm_diagram_search();
        assert_eq!(app.diagrams.next_request, request);
        assert_eq!(
            app.diagrams.viewer.as_ref().unwrap().selected,
            Some(id(ObjectKind::Node, "A"))
        );
    }

    #[test]
    fn diagram_search_hidden_node_reveals_ancestors_and_relayouts_once() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        app.diagrams.viewer.as_mut().unwrap().view.overview = true;
        let request = app.diagrams.next_request;
        app.begin_diagram_search();
        for c in "Beta".chars() {
            app.edit_diagram_search(Some(c));
        }
        app.confirm_diagram_search();
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        assert_eq!(app.diagrams.next_request, request + 1);
        assert!(viewer.view.expanded.contains("G"));
        assert_eq!(viewer.selected, Some(id(ObjectKind::Node, "B")));
    }

    #[test]
    fn diagram_source_toggle_never_replaces_original_and_has_separate_pan() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        let original = app.diagrams.blocks[0].source.clone();
        app.pan_diagram(30, 10);
        app.diagram_control('s');
        assert_eq!(app.diagrams.viewer.as_ref().unwrap().source, original);
        assert!(app.diagrams.viewer.as_ref().unwrap().source_mode);
        app.diagram_control('s');
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        assert_eq!((viewer.x, viewer.y), (30, 10));
    }

    #[test]
    fn diagram_viewport_uses_cell_clipping_without_wrapping_and_small_sizes_are_safe() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| crate::render::ui(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 1)).unwrap().symbol(), "r");
        assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "r");
        assert_eq!(app.content_area, Rect::new(0, 0, 80, 30));
        let mut tiny = Terminal::new(TestBackend::new(1, 1)).unwrap();
        tiny.draw(|f| crate::render::ui(f, &mut app)).unwrap();
    }

    #[test]
    fn diagram_theme_roundtrip_keeps_metadata_and_deferred_rendering() {
        let _guard = crate::tests::lock_theme_test_state();
        let (mut app, ss, themes) = fixture();
        let original = app.diagrams.blocks[0].source.clone();
        app.open_theme_picker();
        let preset = crate::theme::THEME_PRESETS
            .iter()
            .find(|p| **p != crate::theme::current_theme_selection().preset_hint())
            .unwrap();
        app.preview_theme_preset(*preset, &ss, &themes);
        assert_eq!(app.diagrams.blocks[0].source, original);
        app.restore_theme_picker_preview(&ss, &themes);
        assert_eq!(app.diagrams.blocks[0].source, original);
        assert!(app.diagrams.previews.is_empty());
    }

    #[test]
    fn diagram_identical_blocks_share_preview_but_keep_selection_identity() {
        let (mut app, ss, themes) = fixture();
        let source = app.diagrams.blocks[0].source.clone();
        app.source = format!("```mermaid\n{source}```\n\n```mermaid\n{source}```");
        app.reparse_source(&ss, &themes);
        assert_eq!(app.diagrams.blocks.len(), 2);
        assert_eq!(app.diagrams.blocks[0].source, app.diagrams.blocks[1].source);
        app.cache_diagram_preview(source, Ok(Arc::new(output("shared").preview())));
        app.reparse_source(&ss, &themes);
        assert_eq!(app.diagrams.previews.len(), 1);
        app.code_select = Some(app.diagrams.blocks[1].code_block_index);
        app.open_diagram();
        assert_eq!(app.diagrams.viewer.as_ref().unwrap().ordinal, 1);
    }

    #[test]
    fn diagram_preview_completion_after_close_remains_valid_for_document() {
        let (mut app, ss, themes) = fixture();
        let block = &app.diagrams.blocks[0];
        let mut rendered = (*output("PREVIEW")).clone();
        // This synthetic marker is the node; its bounds must match its cells.
        rendered.objects[1].bounds.x = 0;
        let preview = DiagramCompletion {
            job: DiagramJob {
                document: app.diagrams.document,
                request: 0,
                ordinal: block.ordinal,
                source: block.source.clone(),
                options: app.diagrams.options,
                view: None,
            },
            result: Ok(Arc::new(rendered)),
        };
        app.open_diagram();
        app.close_diagram();
        app.diagrams
            .service
            .as_ref()
            .unwrap()
            .test_complete(preview);
        app.poll_diagrams(&ss, &themes);
        assert!(!app.is_diagram_open());
        assert_eq!(app.diagrams.previews.len(), 1);
        assert!(app
            .lines
            .iter()
            .any(|line| line.to_string().contains("PREVIEW")));
    }

    #[test]
    fn diagram_preview_cache_evicts_entries_within_budget() {
        let (mut app, _, _) = fixture();
        for i in 0..100 {
            app.cache_diagram_preview(format!("source{i}"), Err("error".into()));
        }
        assert_eq!(app.diagrams.previews.len(), PREVIEW_CACHE_ENTRIES);
        assert!(!app.diagrams.previews.contains_key("source0"));
        assert!(app.diagrams.previews.contains_key("source99"));
    }

    #[test]
    fn diagram_initial_open_centers_first_node_outside_empty_origin() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        let mut rendered = (*output("far")).clone();
        rendered.rows = vec![" ".repeat(200); 70];
        rendered.rows[60] = format!("{}NODE{}", " ".repeat(150), " ".repeat(46));
        rendered.objects[1].bounds = CellRect {
            x: 150,
            y: 60,
            width: 10,
            height: 5,
        };
        let request = app.diagrams.viewer.as_ref().unwrap().request;
        let mut done = completion(&app, request, "far");
        done.result = Ok(Arc::new(rendered));
        app.diagrams.service.as_ref().unwrap().test_complete(done);
        app.poll_diagrams(&ss, &themes);
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| crate::render::ui(f, &mut app)).unwrap();
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        assert_eq!(viewer.selected, Some(id(ObjectKind::Node, "A")));
        assert!(viewer.x > 0 && viewer.y > 0);
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("NODE"));
    }

    #[test]
    fn diagram_short_canvas_bottom_node_is_centered_not_pinned_to_screen_bottom() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        let mut rendered = (*output("bottom")).clone();
        rendered.height = 41;
        rendered.rows = vec![String::new(); 41];
        rendered.rows[37] = "                    Node 1".into();
        rendered.objects[1].bounds = CellRect {
            x: 20,
            y: 36,
            width: 10,
            height: 3,
        };
        let request = app.diagram_viewer().unwrap().request;
        let mut done = completion(&app, request, "bottom");
        done.result = Ok(Arc::new(rendered));
        app.diagrams.service.as_ref().unwrap().test_complete(done);
        app.poll_diagrams(&ss, &themes);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| crate::render::ui(frame, &mut app))
            .unwrap();
        let viewer = app.diagram_viewer().unwrap();
        assert_eq!(37 - viewer.y, viewer.area.height as usize / 2);
        let pan = (viewer.x, viewer.y);
        app.diagram_control('s');
        assert_eq!(app.diagram_viewer().unwrap().y, 0);
        app.diagram_control('s');
        assert_eq!(
            (
                app.diagram_viewer().unwrap().x,
                app.diagram_viewer().unwrap().y
            ),
            pan
        );
    }

    #[test]
    fn diagram_explicit_layout_controls_recenter_selected_node() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        for (control, x, y) in [('c', 150, 60), ('d', 5, 5), ('a', 145, 40)] {
            let previous_pan = app
                .diagram_viewer()
                .map(|viewer| (viewer.x, viewer.y))
                .unwrap();
            app.diagram_control(control);
            // The event loop redraws the retained OLD output while its replacement
            // is loading. That frame must not consume the new-layout center intent.
            terminal
                .draw(|frame| crate::render::ui(frame, &mut app))
                .unwrap();
            let loading = app.diagram_viewer().unwrap();
            assert!(loading.loading && loading.center_after_render);
            assert_eq!((loading.x, loading.y), previous_pan);
            let mut relocated = (*output("relocated")).clone();
            relocated.objects[1].bounds = CellRect {
                x,
                y,
                width: 10,
                height: 5,
            };
            let request = app.diagrams.viewer.as_ref().unwrap().request;
            let mut done = completion(&app, request, "relocated");
            done.result = Ok(Arc::new(relocated));
            app.diagrams.service.as_ref().unwrap().test_complete(done);
            app.poll_diagrams(&ss, &themes);
            let viewer = app.diagrams.viewer.as_ref().unwrap();
            assert_eq!(viewer.selected, Some(id(ObjectKind::Node, "A")));
            assert!(
                x >= viewer.x && x + 10 <= viewer.x + viewer.area.width as usize,
                "{control} should keep the selected node horizontally visible"
            );
            assert!(
                y >= viewer.y && y + 5 <= viewer.y + viewer.area.height as usize,
                "{control} should keep the selected node vertically visible"
            );
        }
    }

    #[test]
    fn diagram_overview_selects_and_centers_visible_ancestor_of_hidden_node() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        assert_eq!(
            app.diagrams.viewer.as_ref().unwrap().selected,
            Some(id(ObjectKind::Node, "A"))
        );
        app.diagram_control('o');
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal
            .draw(|frame| crate::render::ui(frame, &mut app))
            .unwrap();
        assert!(app.diagram_viewer().unwrap().center_after_render);
        let mut overview = (*output("overview")).clone();
        overview
            .objects
            .retain(|object| object.id.kind == ObjectKind::Subgraph);
        overview.objects[0].bounds = CellRect {
            x: 150,
            y: 50,
            width: 20,
            height: 10,
        };
        let request = app.diagrams.viewer.as_ref().unwrap().request;
        let mut done = completion(&app, request, "overview");
        done.result = Ok(Arc::new(overview));
        app.diagrams.service.as_ref().unwrap().test_complete(done);
        app.poll_diagrams(&ss, &themes);
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        assert_eq!(viewer.selected, Some(id(ObjectKind::Subgraph, "G")));
        assert_eq!((viewer.x, viewer.y), (120, 42));
    }

    #[test]
    fn diagram_resize_layout_preserves_pan_except_clamping() {
        let (mut app, ss, themes) = fixture();
        app.open_diagram();
        make_ready(&mut app, &ss, &themes);
        app.pan_diagram(100, 30);
        app.set_diagram_area(Rect::new(0, 1, 90, 26));
        app.diagrams.viewer.as_mut().unwrap().view.width = 90;
        app.request_diagram_render();
        let request = app.diagrams.viewer.as_ref().unwrap().request;
        app.diagrams
            .service
            .as_ref()
            .unwrap()
            .test_complete(completion(&app, request, "resized"));
        app.poll_diagrams(&ss, &themes);
        let viewer = app.diagrams.viewer.as_ref().unwrap();
        assert_eq!((viewer.x, viewer.y), (100, 30));
    }
}
