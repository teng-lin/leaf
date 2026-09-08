# Large diagrams in the terminal

This is the consolidated guide to Leaf's Mermaid controls, layout behavior,
implementation, verification, and remaining work. Leaf uses the companion
mmdflux prepared-text API; no second rendering backend is required.

## Preview and viewer

Mermaid blocks whose rendered width fits the document show every diagram row,
using normal vertical document scrolling (including tall state diagrams).
Wider Mermaid blocks show a bounded, cell-clipped preview chosen from actual node bounds,
not blindly from the top-left corner. The preview favors complete nodes and trims
blank margins and long trailing route-only areas. Graphs with at least 50 nodes
and two top-level groups show a labeled group overview in the document. This
does not change the authored direction, full viewer, original source, or full export.
Sequence messages are not cropped to participant boxes.

Diagram rows are never wrapped: wrapping would disconnect arrows from their nodes.
Clipping and preview offsets are disclosed in readable text; the original Mermaid
remains available for copying even if rendering fails. Oversized canvases still
require navigation, and a viewport boundary can cut an edge or a node.

Press `v` to open the selected Mermaid block, or the first one currently visible.
The viewer measures the full rendered diagram against its available columns.
It opens the full diagram when its width fits, allowing vertical scrolling;
otherwise it starts in overview
when the diagram supports collapsing. `o` still switches between overview and
full detail. This initial choice does not override subsequent manual choices.
`Enter` in the document still copies the selected code block.

| Viewer key | Action |
| --- | --- |
| Arrows / `hjkl` | Pan horizontally and vertically |
| PageUp / PageDown / Home | Page vertically / reset pan |
| Tab / Shift-Tab | Select objects and center them |
| `/`, query, Enter | Search all original node/group IDs and labels, reveal selection |
| `c` | Toggle configured spacing and compact `8 / 15 / 8` |
| `d` | Toggle authored direction and an explicitly allowed alternate layout |
| `o` | Toggle full diagram and top-level group overview |
| Space | Expand/collapse selected group |
| `f` | Toggle selected node's one-hop incoming/outgoing neighborhood |
| `a` | Toggle class member detail |
| `s` / `y` | Show / copy original Mermaid source |
| Esc / `q` | Close viewer; Esc first cancels active search |

Graph-only controls explain when the diagram family does not support them.
The status shows the full canvas extent, pan position, effective layout, hidden
content, and warnings. Overview/focus are semantic views, not smaller copies of
the full diagram. Searching a hidden node reveals it. Pan and visible-node
selection reuse the current canvas; layout changes run in a background worker.
Opening or resizing the viewer does not automatically re-wrap labels. For the
same semantic view, preview, viewer and full export share their layout policy.
Oversized ungrouped graphs initially select a node in a well-populated region. The
viewer allows half a screen of trailing space so nodes near the canvas edge can
be centered; source mode keeps ordinary document scroll limits.

## Spacing

These are engine layout units, not terminal columns. Defaults remain
`50 / 50 / 20`; compact uses `8 / 15 / 8`.

- **Node spacing** separates siblings across the flow direction: horizontally
  for TD/BT, vertically for LR/RL. A simple single-file chain has no siblings.
- **Rank spacing** separates successive layers along the flow direction. This
  is the control to change the gaps in the large flowchart's group overview.
- **Edge spacing** changes solver routing lanes, especially long edges and
  dummy nodes. It does not add gaps to a chain with no competing lanes or
  guarantee separate trunks for every edge.

Leaf now carries node/rank spacing into the final terminal-grid stage as well
as the solver. The cell-gap lower bound is `ceil(value / 5)`, with safety floors
of four horizontal columns and three vertical rows; labels and routing may need
more. Thus horizontal rank spacing 15 requests a minimum four-column gap, while
50 requests ten columns. Small changes can round to the same cell gap. This is
not a promise of exact gaps or monotonic total canvas size for a complex graph.
For eligible ungrouped canvases, edge spacing also supplies a minimum retained
connector-corridor length during compaction.

Document previews display `spacing N/R/E`; the viewer title displays
`configured N/R/E` or `compact 8/15/8`, even when the footer contains a warning.
`c` deliberately overrides configured values with the compact profile until
toggled back. Sequence and pie renderers do not use graph spacing; their viewer
title says `spacing n/a`.

```toml
[mermaid]
node-spacing = 8
rank-spacing = 15
edge-spacing = 8
```

Each field resolves independently: CLI > environment > config > default.
Environment names are `LEAF_MERMAID_NODE_SPACING`, `LEAF_MERMAID_RANK_SPACING`,
and `LEAF_MERMAID_EDGE_SPACING`. Valid values are finite numbers from 0 to 500.
Invalid CLI values are errors; invalid environment/config values are ignored.

```sh
leaf --node-spacing 8 --rank-spacing 15 --edge-spacing 8 architecture.md
leaf --inline plain:80 architecture.md
leaf --inline plain:80 --mermaid-full architecture.md > diagrams.txt
```

### Width and export

`--width` controls document width; `plain:80` above controls inline Markdown
width. Neither forces a full diagram into that many columns. The interactive
viewport follows the terminal size and clips in display cells without changing
diagram geometry. A dedicated Mermaid width/height or label-wrap flag is not
implemented.

For an obvious spacing comparison, open `tests/fixtures/diagrams/spacing-ranks.md`
with `--rank-spacing 15` and then `--rank-spacing 50`. The adjacent
`spacing-nodes.md` and `spacing-edges.md` fixtures isolate the other controls.

Default inline output discloses clipped previews. `--mermaid-full` emits every
diagram row unwrapped, even wider than the requested width. Ordinary Markdown
still uses that width. ANSI output supports the same option. Terminals can still
visually wrap long stdout lines: save to a file or use a horizontal-scroll pager.

## Limits and failures

Prepared text rendering limits source bytes, nodes/edges, label bytes, layout
candidates, canvas dimensions and cell allocations. Leaf bounds its worker queue
and caches. Stale work cannot replace a newer view. An already-running solver is
not forcibly interrupted; cancellation discards stale results and queued work.
Oversized/unsupported input produces an explicit error with source access.
Sequence diagrams and pie charts remain viewable but do not offer graph projection.

## Implementation and fixes

Leaf keeps Mermaid source and unwrapped diagram rows separate from ordinary
Markdown. Startup, reload, reflow, and theme-preview paths use a cache-only parse
context; layout runs on a background worker with bounded pending work and caches.
Document/request generations prevent stale jobs from replacing newer results.
Closing the viewer preserves document position and source-copy behavior.

mmdflux prepares immutable compiled data once, then projects full, overview,
collapsed, focused, or class-header views before measurement and layout. Original
node identities and edge membership remain accounted for even when hidden.
The renderer captures actual emitted cell rectangles and routed edges; Leaf does
not use SVG coordinates for terminal selection or hit testing. See the companion
[engine API reference](https://github.com/teng-lin/mmdflux/blob/1921b83eacf5d467f7d33b5f539c5d8780dab8f7/docs/text-views.md)
for its public contracts.

The implementation includes these regression fixes:

- **Blank or cut-off previews:** choose a populated window from actual node
  bounds, trim unhelpful margins, and disclose clipping. Large grouped previews
  use an explicit overview; opening their full view is intentionally different.
- **Spacing ignored by the grid:** carry node/rank settings through final cell
  placement, preserve packing minima, and use explicit ranks for opted-in
  ungrouped graphs. Legacy mmdflux defaults remain opt-out.
- **Excessive class-diagram spacing:** compare only populated ranks during gap
  repair. Reserve horizontal edge-label clearance near its endpoints without
  moving unrelated connected components. Shorten blank/straight-connector bands
  in eligible ungrouped captures, preserving boxes, text spaces, graphemes,
  arrows, corners, and junctions. Remap geometry and emission audits together.
  Compound/subgraph layouts retain their existing border/routing treatment.
- **Layout changes on `v`:** request one layout candidate. Viewport dimensions
  report overflow without implicitly selecting a new label width. Explicit
  spacing, direction, and semantic-view controls still trigger layout.
- **Routing and Unicode:** protect labels/markers from later painting, preserve
  parallel self-loop identities, repair outward ports and obstructed routes,
  use deterministic tie-breaks, and measure/crop complete terminal graphemes.
  Bounded render failures remain visible; they are not hidden by dropping edges.
- **Navigation and fallback:** retain pending recentering through loading frames,
  allow near-edge nodes to be centered, preserve Mermaid in nested Markdown and
  legacy footnotes, and avoid rectangular padding of rejected source text.

For `rookie-classes.md` at node/rank/edge spacing `4/8/4`, the reported mismatch
changed as follows:

| Full canvas | Before | After |
| --- | --- | --- |
| Document preview | 1,648 × 337 | 364 × 185 |
| Viewer opened with `v` | 1,315 × 384 | 364 × 185 |

All 31 classes, 27 relationships, member text, cardinalities, and six body labels
remain. The graph still needs panning; the reduction does not hide information.

## Build and install

This branch pins its engine dependency to `teng-lin/mmdflux` revision
`1921b83eacf5d467f7d33b5f539c5d8780dab8f7`, containing the companion
`feat/terminal-diagram-views` changes. Cargo fetches that exact revision; a sibling
checkout is not required. Both the manifest and lockfile record the dependency.
The released crate alone does not yet provide this API. A crates.io release of
Leaf will need a published mmdflux version containing these changes before the
Git dependency can be replaced with a registry dependency.

```sh
cargo +1.95.0 build --release --locked
./target/release/leaf --node-spacing 4 --rank-spacing 8 --edge-spacing 4 \
  tests/fixtures/diagrams/rookie-classes.md
```

This revision includes the local-direction subgraph-spacing fix. No local
dependency override is needed to build the fixes described in this guide.

When installing on macOS, place the new executable in a temporary file on the
destination filesystem and atomically rename it over the destination. Do not
overwrite a previously executed binary in place: cached code-signature state can
cause a launch crash. Verify the signature and launch the installed path, then
quit and reopen existing Leaf sessions; running processes keep the earlier code.

## Verification

The subgraph-spacing, width-based initial view, and full-height narrow document
follow-up was checked with 475
Leaf tests, 2,459 mmdflux library tests (8 ignored), 20 prepared-text integration
tests, Clippy in both repositories, 45 CLI cases, and 36 terminal cases (303
captures). The exact backedge fixture was also checked in 140×70 and 140×110
terminals, including visual inspection of full and overview captures.

The original functional build was verified on 2026-09-07:

| Check | Result |
| --- | --- |
| Leaf Rust suite | 473 passed |
| mmdflux library/integration suite | 3,294 passed; 8 pre-existing ignored |
| mmdflux doctests | 9 passed; 1 ignored |
| Strict all-target Clippy, both repositories | Passed |
| Formatting, diff checks, fresh engine architecture check | Passed |
| Release and installed executable CLI checks | 45 cases each |
| Release and installed terminal interaction checks | 34 cases, 272 captures each |
| Visual inspection | 12 final color terminal images reviewed |

No legacy snapshot baselines were regenerated. Engine regressions verify source
and edge conservation, terminal extents, routes, labels/markers, node interiors,
Unicode, and bounded admission. A 72-render sweep covers four real graphs, six
spacing profiles, and three widths. Only disclosed soft-target overflow warnings
are permitted for that corpus.

Leaf checks exact preview/viewer text and geometry at 80, 120, and 301 columns.
Nine terminal cases compare three class spacing profiles at those widths,
including resize stability and populated initial framing. The class regression
requires the complete `4/8/4` canvas to remain within 400 × 210 cells.

### Reproduce the checks

```sh
cargo +1.95.0 test --locked
cargo +1.95.0 clippy --all-targets -- -D warnings
cargo +1.95.0 build --release
python3 scripts/test-diagram-cli.py
python3 scripts/test-terminal-diagrams.py
```

From a mmdflux checkout at the pinned revision:

```sh
cargo +1.95.0 test --locked
cargo +1.95.0 clippy --all-targets -- -D warnings
cargo +nightly-2025-11-23 fmt --all -- --check
cargo +1.95.0 xtask architecture check --fresh
```

The terminal test requires `tmux` and uses a private server, configuration
directory, and test-only clipboard command. It sends real keys and resize events,
checks readiness without further input, verifies clean exit, and saves screen
captures (plain text and ANSI colors) plus a JSON result ledger in a fresh temporary
directory. It clears `NO_COLOR` in the test child and requires actual color output.
The large-diagram checks require early visible node labels and verify the initial
selection position, not just a ready status or diagram border. The fixtures
include the real architecture/class examples, a 200-node graph, backedges,
sequence/state/pie diagrams, Unicode labels, and unsupported input. It never
changes your normal terminal sessions, configuration, or clipboard.
Both harnesses accept an optional executable path, record its SHA-256, and reject
a binary that changes during the run. Generated reports and captures belong in
the temporary directories printed by the harnesses, not in version control.
Keep the regression fixtures, test harnesses, and capture renderer in the repo.

On macOS, turn a captured terminal grid into an image for visual inspection:

```sh
swift scripts/render-terminal-capture.swift capture.ansi capture.png 120
```

This paints the captured cells and colors without laying out the diagram again.
Inspect document previews and full views at small and large terminal widths,
including the 301-column class regression and compact/configured comparisons;
passing automated checks alone is not visual acceptance.

## Next controls, not yet implemented

Keep mmdflux as the engine and improve explicit view/layout policy before adding
another backend. The next useful controls are:

1. Preview mode: `auto`, `full`, `overview`, or `source`.
2. Maximum label width in display cells, applied before layout while retaining
   all label text; never wrap completed diagram rows.
3. Diagram-specific preferred width/height as soft targets, separate from
   Markdown width and without silently hiding content.
4. Persistent direction policy, preserving authored direction by default.
5. Configurable document preview row limit instead of the current 24-row cap.

Initial focus, hop count, traversal direction, group collapse, and class-member
detail can follow. The engine already models these; Leaf currently exposes
simpler interactive toggles. Alternative engines and terminal images remain
optional research directions, not dependencies of this implementation.
