//! A single replaceable-job worker. No parsing or layout runs on the UI thread.
use crate::markdown::{MermaidOptions, MermaidPreview};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ObjectKind {
    Node,
    Subgraph,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ObjectId {
    pub(crate) kind: ObjectKind,
    pub(crate) id: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogObject {
    pub(crate) id: ObjectId,
    pub(crate) label: String,
    pub(crate) ancestors: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CellRect {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct VisibleObject {
    pub(crate) id: ObjectId,
    pub(crate) bounds: CellRect,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Capabilities {
    pub(crate) collapse: bool,
    pub(crate) focus: bool,
    pub(crate) direction: bool,
    pub(crate) members: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DiagramOutput {
    pub(crate) rows: Vec<String>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) catalog: Vec<CatalogObject>,
    pub(crate) objects: Vec<VisibleObject>,
    pub(crate) capabilities: Capabilities,
    pub(crate) direction: Option<mmdflux::graph::Direction>,
    pub(crate) summary: String,
    pub(crate) visible_nodes: usize,
    pub(crate) original_nodes: usize,
    pub(crate) hidden_nodes: usize,
    pub(crate) boundary_edges: usize,
    pub(crate) warnings: Vec<String>,
    pub(crate) overview: bool,
}

impl DiagramOutput {
    pub(crate) fn initial_view(
        &self,
        width: usize,
        height: usize,
    ) -> Option<(ObjectId, usize, usize)> {
        if !self.capabilities.focus
            || self.capabilities.collapse
            || (self.width <= width && self.height <= height)
        {
            return None;
        }
        let geometry = MermaidPreview {
            width: self.width,
            height: self.height,
            nodes: self
                .objects
                .iter()
                .filter(|object| object.id.kind == ObjectKind::Node)
                .map(|object| mmdflux::TextCellRect {
                    x: object.bounds.x,
                    y: object.bounds.y,
                    width: object.bounds.width,
                    height: object.bounds.height,
                })
                .collect(),
            ..Default::default()
        };
        let (x, y) = geometry.preview_origin(width, height);
        self.objects
            .iter()
            .filter(|object| {
                let rect = object.bounds;
                object.id.kind == ObjectKind::Node
                    && rect.x >= x
                    && rect.y >= y
                    && rect.x + rect.width <= x + width
                    && rect.y + rect.height <= y + height
            })
            .min_by_key(|object| {
                let rect = object.bounds;
                (rect.x + rect.width / 2).abs_diff(x + width / 2)
                    + (rect.y + rect.height / 2).abs_diff(y + height / 2)
            })
            .map(|object| (object.id.clone(), x, y))
    }

    pub(crate) fn preview(&self) -> MermaidPreview {
        MermaidPreview {
            text: self.rows.join("\n"),
            width: self.width,
            height: self.height,
            warning: (!self.warnings.is_empty()).then(|| self.warnings.join("; ")),
            nodes: self
                .objects
                .iter()
                .filter(|object| {
                    self.capabilities.focus && (self.overview || object.id.kind == ObjectKind::Node)
                })
                .map(|object| mmdflux::TextCellRect {
                    x: object.bounds.x,
                    y: object.bounds.y,
                    width: object.bounds.width,
                    height: object.bounds.height,
                })
                .collect(),
            overview: self.overview,
            hidden_nodes: self.hidden_nodes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ViewOptions {
    pub(crate) compact: bool,
    pub(crate) alternate: bool,
    pub(crate) authored_direction: Option<mmdflux::graph::Direction>,
    pub(crate) overview: bool,
    pub(crate) collapsed: BTreeSet<String>,
    pub(crate) expanded: BTreeSet<String>,
    pub(crate) focus: Option<String>,
    pub(crate) members: bool,
    pub(crate) width: usize,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            compact: false,
            alternate: false,
            authored_direction: None,
            overview: false,
            collapsed: BTreeSet::new(),
            expanded: BTreeSet::new(),
            focus: None,
            members: true,
            width: 80,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiagramJob {
    pub(crate) document: u64,
    pub(crate) request: u64,
    pub(crate) ordinal: usize,
    pub(crate) source: String,
    pub(crate) options: MermaidOptions,
    /// None means a configured inline preview, not a modal variant.
    pub(crate) view: Option<ViewOptions>,
}

pub(crate) struct DiagramCompletion {
    pub(crate) job: DiagramJob,
    pub(crate) result: Result<Arc<DiagramOutput>, String>,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<DiagramJob>,
    active: bool,
    active_foreground: bool,
    cancellation: Option<Arc<AtomicBool>>,
    completed: VecDeque<DiagramCompletion>,
    shutdown: bool,
}

pub(crate) struct DiagramService {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}

impl DiagramService {
    #[cfg(test)]
    pub(super) fn test_idle() -> Self {
        Self {
            shared: Arc::new((Mutex::new(Mailbox::default()), Condvar::new())),
        }
    }

    #[cfg(test)]
    pub(super) fn test_complete(&self, completion: DiagramCompletion) {
        let mut state = self.shared.0.lock().unwrap();
        state.pending = None;
        state.completed.push_back(completion);
    }

    pub(crate) fn start() -> Self {
        Self::start_with_processor(EngineProcessor::default())
    }

    fn start_with_processor(mut processor: impl JobProcessor + Send + 'static) -> Self {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("leaf-diagrams".into())
            .spawn(move || {
                loop {
                    let (job, cancellation) = {
                        let (lock, wake) = &*worker;
                        let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
                        while state.pending.is_none() && !state.shutdown {
                            state = wake.wait(state).unwrap_or_else(|p| p.into_inner());
                        }
                        if state.shutdown {
                            break;
                        }
                        state.active = true;
                        let job = state.pending.take().expect("pending job");
                        let cancellation = Arc::new(AtomicBool::new(false));
                        state.active_foreground = job.view.is_some();
                        state.cancellation = Some(Arc::clone(&cancellation));
                        (job, cancellation)
                    };
                    processor.set_cancellation(Arc::clone(&cancellation));
                    // Keep malformed input from terminating the worker. Resource admission is
                    // enforced by the engine; unwind recovery is not a timeout mechanism.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        processor.render(&job)
                    }))
                    .unwrap_or_else(|_| {
                        Err("Diagram renderer panicked; original source remains available".into())
                    });
                    let mut state = worker.0.lock().unwrap_or_else(|p| p.into_inner());
                    state.active = false;
                    state.cancellation = None;
                    if state.shutdown {
                        break;
                    }
                    if cancellation.load(Ordering::Relaxed) {
                        continue;
                    }
                    // At most two full canvases can wait for the UI. A discarded preview
                    // can be scheduled again; request generations reject stale modal work.
                    if state.completed.len() == 2 {
                        state.completed.pop_front();
                    }
                    state.completed.push_back(DiagramCompletion { job, result });
                }
            })
            .expect("start diagram worker");
        Self { shared }
    }

    pub(crate) fn submit(&self, job: DiagramJob) {
        let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
        if job.view.is_some() && state.active_foreground {
            if let Some(token) = &state.cancellation {
                token.store(true, Ordering::Relaxed);
            }
        }
        state.pending = Some(job);
        self.shared.1.notify_one();
    }

    pub(crate) fn cancel_foreground(&self) {
        let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.pending.as_ref().is_some_and(|job| job.view.is_some()) {
            state.pending = None;
        }
        if state.active_foreground {
            if let Some(token) = &state.cancellation {
                token.store(true, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn cancel_all(&self) {
        let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
        state.pending = None;
        if let Some(token) = &state.cancellation {
            token.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn busy(&self) -> bool {
        let state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
        state.active || state.pending.is_some() || !state.completed.is_empty()
    }

    pub(crate) fn drain(&self) -> Vec<DiagramCompletion> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .completed
            .drain(..)
            .collect()
    }
}

impl Drop for DiagramService {
    fn drop(&mut self) {
        let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
        state.shutdown = true;
        if let Some(token) = &state.cancellation {
            token.store(true, Ordering::Relaxed);
        }
        state.pending = None;
        state.completed.clear();
        self.shared.1.notify_one();
        // Never join a solve on the event loop or during terminal teardown.
    }
}

trait JobProcessor {
    fn set_cancellation(&mut self, _token: Arc<AtomicBool>) {}
    fn render(&mut self, job: &DiagramJob) -> Result<Arc<DiagramOutput>, String>;
}

#[derive(Default)]
struct EngineProcessor {
    prepared: VecDeque<mmdflux::PreparedTextDiagram>,
    cancellation: Option<Arc<AtomicBool>>,
}

impl JobProcessor for EngineProcessor {
    fn set_cancellation(&mut self, token: Arc<AtomicBool>) {
        self.cancellation = Some(token);
    }

    fn render(&mut self, job: &DiagramJob) -> Result<Arc<DiagramOutput>, String> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(|token| token.load(Ordering::Relaxed))
        {
            return Err("Diagram request cancelled".into());
        }
        if job.source.trim_start().starts_with("pie") {
            let preview = crate::markdown::render_mermaid_preview(&job.source, job.options, true)?;
            return Ok(Arc::new(DiagramOutput {
                rows: preview.text.lines().map(str::to_owned).collect(),
                width: preview.width,
                height: preview.height,
                catalog: Vec::new(),
                objects: Vec::new(),
                capabilities: Capabilities::default(),
                direction: None,
                summary: "pie · graph controls unavailable".into(),
                visible_nodes: 0,
                original_nodes: 0,
                hidden_nodes: 0,
                boundary_edges: 0,
                warnings: preview.warning.into_iter().collect(),
                overview: false,
            }));
        }
        let limits = mmdflux::TextLimits::default();
        if let Some(index) = self.prepared.iter().position(|p| p.source() == job.source) {
            let prepared = self
                .prepared
                .remove(index)
                .expect("existing prepared diagram");
            self.prepared.push_back(prepared);
        } else {
            let prepared =
                mmdflux::prepare_text_diagram(&job.source, &limits).map_err(|e| e.to_string())?;
            while self.prepared.len() >= 8
                || self
                    .prepared
                    .iter()
                    .map(|p| p.source().len())
                    .sum::<usize>()
                    + job.source.len()
                    > 4 * 1024 * 1024
            {
                if self.prepared.pop_front().is_none() {
                    break;
                }
            }
            self.prepared.push_back(prepared);
        }
        let prepared = self.prepared.back().expect("prepared diagram");
        let caps = prepared.capabilities();
        let mut request = mmdflux::TextRequest {
            config: job.options.render_config(),
            cancellation: self.cancellation.clone(),
            // Opening/resizing a viewport must not silently re-wrap labels and
            // pick a different layout from the document preview/full export.
            // Width/height remain overflow targets, not permission to reflow.
            max_candidates: 1,
            ..Default::default()
        };
        request.view.overview = job.view.is_none() && MermaidPreview::use_overview(prepared);
        if let Some(view) = &job.view {
            if view.compact {
                request.config = MermaidOptions::COMPACT.render_config();
            }
            request.view.overview = view.overview;
            request.view.collapsed_subgraphs = view.collapsed.clone();
            request.view.expanded_subgraphs = view.expanded.clone();
            request.view.focus = view.focus.as_ref().map(mmdflux::TextFocus::one_hop);
            request.view.show_class_members = view.members;
            request.direction = if view.alternate {
                use mmdflux::graph::Direction::*;
                view.authored_direction
                    .map(|direction| {
                        mmdflux::TextDirectionPolicy::Override(match direction {
                            TopDown => LeftRight,
                            BottomTop => RightLeft,
                            LeftRight => TopDown,
                            RightLeft => BottomTop,
                        })
                    })
                    .unwrap_or(mmdflux::TextDirectionPolicy::AllowAlternate)
            } else {
                mmdflux::TextDirectionPolicy::Preserve
            };
            request.preferred_width = Some(view.width.max(1));
            request.preferred_height = Some(80);
        }
        let result =
            mmdflux::render_prepared_text(prepared, &request).map_err(|e| e.to_string())?;
        let convert_id = |id: &mmdflux::TextObjectId| match id {
            mmdflux::TextObjectId::Node(id) => ObjectId {
                kind: ObjectKind::Node,
                id: id.clone(),
            },
            mmdflux::TextObjectId::Subgraph(id) => ObjectId {
                kind: ObjectKind::Subgraph,
                id: id.clone(),
            },
        };
        let catalog = prepared
            .catalog()
            .iter()
            .map(|entry| CatalogObject {
                id: convert_id(&entry.id),
                label: entry.label.clone(),
                ancestors: entry.ancestors.clone(),
            })
            .collect();
        let objects = result
            .objects
            .iter()
            .map(|object| VisibleObject {
                id: convert_id(&object.id),
                bounds: CellRect {
                    x: object.rect.x,
                    y: object.rect.y,
                    width: object.rect.width,
                    height: object.rect.height,
                },
            })
            .collect();
        let a = &result.accounting;
        let summary = format!(
            "nodes {}/{} · hidden {} · edges {} · boundary {} · {:?} · {}/{}/{}",
            a.visible_nodes,
            a.original_nodes,
            a.hidden_nodes(),
            a.original_edges,
            a.boundary_edges.len(),
            result.effective.direction,
            result.effective.node_spacing,
            result.effective.rank_spacing,
            result.effective.edge_spacing
        );
        Ok(Arc::new(DiagramOutput {
            rows: result.text.lines().map(str::to_owned).collect(),
            width: result.width,
            height: result.height,
            direction: result.effective.direction,
            catalog,
            objects,
            capabilities: Capabilities {
                collapse: caps.collapse,
                focus: caps.focus,
                direction: caps.direction,
                members: caps.class_members,
            },
            summary,
            warnings: result.diagnostics,
            overview: request.view.overview,
            visible_nodes: a.visible_nodes,
            original_nodes: a.original_nodes,
            hidden_nodes: a.hidden_nodes(),
            boundary_edges: a.boundary_edges.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct Delayed {
        started: mpsc::Sender<u64>,
        proceed: mpsc::Receiver<()>,
    }

    impl JobProcessor for Delayed {
        fn render(&mut self, job: &DiagramJob) -> Result<Arc<DiagramOutput>, String> {
            self.started.send(job.request).unwrap();
            self.proceed.recv_timeout(Duration::from_secs(3)).unwrap();
            Err(format!("controlled {}", job.request))
        }
    }

    fn job(request: u64) -> DiagramJob {
        DiagramJob {
            document: 1,
            request,
            ordinal: 0,
            source: "flowchart TD; A-->B".into(),
            options: MermaidOptions::default(),
            view: Some(ViewOptions::default()),
        }
    }

    #[test]
    fn diagram_worker_replaces_pending_jobs_and_keeps_one_active() {
        let (started_tx, started_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        let worker = DiagramService::start_with_processor(Delayed {
            started: started_tx,
            proceed: proceed_rx,
        });
        worker.submit(job(1));
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 1);
        worker.submit(job(2));
        worker.submit(job(3));
        assert!(worker.busy());
        assert!(worker
            .shared
            .0
            .lock()
            .unwrap()
            .cancellation
            .as_ref()
            .unwrap()
            .load(Ordering::Relaxed));
        proceed_tx.send(()).unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 3);
        let first = worker.drain();
        assert!(
            first.is_empty(),
            "superseded active result is not published"
        );
        proceed_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let ready = worker.drain();
            if !ready.is_empty() {
                assert_eq!(ready[0].job.request, 3);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn diagram_worker_drop_does_not_join_active_solve() {
        let (started_tx, started_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        let worker = DiagramService::start_with_processor(Delayed {
            started: started_tx,
            proceed: proceed_rx,
        });
        worker.submit(job(1));
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let started = Instant::now();
        drop(worker);
        assert!(started.elapsed() < Duration::from_millis(100));
        proceed_tx.send(()).unwrap();
    }

    #[test]
    fn diagram_worker_close_preserves_preview_but_document_change_cancels_it() {
        let (started_tx, started_rx) = mpsc::channel();
        let (proceed_tx, proceed_rx) = mpsc::channel();
        let worker = DiagramService::start_with_processor(Delayed {
            started: started_tx,
            proceed: proceed_rx,
        });
        let mut preview = job(1);
        preview.view = None;
        worker.submit(preview);
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let token = Arc::clone(
            worker
                .shared
                .0
                .lock()
                .unwrap()
                .cancellation
                .as_ref()
                .unwrap(),
        );
        worker.cancel_foreground();
        assert!(
            !token.load(Ordering::Relaxed),
            "closing does not cancel a valid configured preview"
        );
        worker.cancel_all();
        assert!(
            token.load(Ordering::Relaxed),
            "new document cancels its predecessor's preview"
        );
        proceed_tx.send(()).unwrap();
    }

    #[test]
    fn diagram_prepared_adapter_keeps_catalog_through_overview_expansion_and_focus() {
        let mut processor = EngineProcessor::default();
        let mut request = job(1);
        request.source =
            "flowchart LR\nsubgraph S[Services]\nA[Alpha] --> B[Beta]\nend\nB --> C[Client]".into();
        let full = processor.render(&request).unwrap();
        assert!(full.objects.iter().any(|o| o.id
            == ObjectId {
                kind: ObjectKind::Node,
                id: "A".into()
            }));
        request.view.as_mut().unwrap().overview = true;
        let overview = processor.render(&request).unwrap();
        assert_eq!(overview.catalog.len(), full.catalog.len());
        assert!(overview.hidden_nodes > 0);
        assert!(overview
            .objects
            .iter()
            .any(|o| o.id.kind == ObjectKind::Subgraph && o.id.id == "S"));
        request.view.as_mut().unwrap().expanded.insert("S".into());
        let expanded = processor.render(&request).unwrap();
        assert!(expanded
            .objects
            .iter()
            .any(|o| o.id.kind == ObjectKind::Node && o.id.id == "A"));
        request.view.as_mut().unwrap().focus = Some("A".into());
        let focused = processor.render(&request).unwrap();
        assert_eq!(focused.catalog.len(), full.catalog.len());
        assert!(focused.boundary_edges > 0);
        assert_eq!(
            processor.prepared.len(),
            1,
            "view changes reuse prepared data"
        );
    }

    #[test]
    fn diagram_class_preview_viewer_and_resize_preserve_the_same_layout() {
        let mut processor = EngineProcessor::default();
        let source = include_str!("../../tests/fixtures/diagrams/rookie-classes.md");
        let raw = source
            .split("```mermaid\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let mut request = job(1);
        request.source = raw.into();
        request.options = MermaidOptions {
            node_spacing: 4.0,
            rank_spacing: 8.0,
            edge_spacing: 4.0,
        };
        request.view = None;
        let preview = processor.render(&request).unwrap();
        let sync = crate::markdown::render_mermaid_preview(raw, request.options, false).unwrap();
        assert_eq!(sync.text, preview.rows.join("\n"));
        for width in [80, 120, 301] {
            request.view = Some(ViewOptions {
                width,
                ..Default::default()
            });
            let viewer = processor.render(&request).unwrap();
            assert_eq!(
                viewer.rows, preview.rows,
                "opening v at width {width} must not reflow"
            );
            assert_eq!(
                (viewer.width, viewer.height),
                (preview.width, preview.height)
            );
            assert_eq!(viewer.visible_nodes, 31);
            assert!(
                viewer.initial_view(width, 36).is_some(),
                "oversized class views need a populated starting frame"
            );
        }
    }

    #[test]
    fn diagram_large_preview_matches_sync_overview_and_viewer_keeps_full_graph() {
        let mut processor = EngineProcessor::default();
        let source = include_str!("../../tests/fixtures/diagrams/flowchart_large.md");
        let raw = source
            .split("```mermaid\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let mut request = job(1);
        request.source = raw.into();
        request.options = MermaidOptions::COMPACT;
        request.view = None;
        let preview = processor.render(&request).unwrap();
        assert!(preview.overview);
        assert_eq!(preview.hidden_nodes, 200);
        let sync = crate::markdown::render_mermaid_preview(raw, request.options, false).unwrap();
        assert_eq!(sync.text, preview.preview().text);
        assert_eq!(sync.nodes, preview.preview().nodes);
        request.view = Some(ViewOptions::default());
        let full = processor.render(&request).unwrap();
        assert!(!full.overview);
        assert_eq!(full.visible_nodes, 200);
        assert!(full.rows.iter().any(|row| row.contains("Node 200")));
        assert_eq!(processor.prepared.len(), 1);
    }

    #[test]
    fn diagram_dense_initial_selection_shows_a_populated_region() {
        let mut processor = EngineProcessor::default();
        let source = include_str!("../../tests/fixtures/diagrams/dense-cyclic-200.md");
        let mut request = job(1);
        request.source = source
            .split("```mermaid\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap()
            .into();
        request.options = MermaidOptions::COMPACT;
        let output = processor.render(&request).unwrap();
        for (width, height) in [(80, 26), (120, 36), (200, 46), (270, 36)] {
            let (selected, x, y) = output.initial_view(width, height).unwrap();
            assert_ne!(selected.id, "N0", "do not start on the isolated first ID");
            let rect = output
                .objects
                .iter()
                .find(|object| object.id == selected)
                .unwrap()
                .bounds;
            assert!(rect.x >= x && rect.y >= y);
            let complete = output
                .objects
                .iter()
                .filter(|object| {
                    let rect = object.bounds;
                    rect.x >= x
                        && rect.x + rect.width <= x + width
                        && rect.y >= y
                        && rect.y + rect.height <= y + height
                })
                .count();
            assert!(
                complete >= 5,
                "{width}: only {complete} complete nodes in initial view"
            );
        }
    }

    #[test]
    fn diagram_prepared_adapter_class_members_are_reversible() {
        let mut processor = EngineProcessor::default();
        let mut request = job(1);
        request.source =
            "classDiagram\nclass Cookie {\n+String domain\n+decrypt() String\n}".into();
        let full = processor.render(&request).unwrap();
        assert!(full.rows.join("\n").contains("domain"));
        request.view.as_mut().unwrap().members = false;
        let hidden = processor.render(&request).unwrap();
        assert!(!hidden.rows.join("\n").contains("domain"));
        assert!(hidden.catalog.iter().any(|o| o.id.id == "Cookie"));
        request.view.as_mut().unwrap().members = true;
        let restored = processor.render(&request).unwrap();
        assert_eq!(full.rows, restored.rows);
        assert_eq!(processor.prepared.len(), 1);
    }
}
