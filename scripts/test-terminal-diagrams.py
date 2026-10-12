#!/usr/bin/env python3
"""Exercise the actual release executable in a private tmux server.

Requires tmux and Python 3; no user sessions, config, or clipboard are modified.
Screens and a JSON result ledger are retained in a freshly created temp directory.
Run: python3 scripts/test-terminal-diagrams.py [path/to/leaf]
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "target/release/leaf"
ARTIFACTS = Path(tempfile.mkdtemp(prefix="leaf-terminal-diagrams."))
SOCKET = "leaf-diagrams-" + str(os.getpid())
CONFIG = ARTIFACTS / "config"
CLIPBOARD = ARTIFACTS / "clipboard.txt"
RESULTS = []
SCREEN_NUMBER = 0


def tmux(*args, check=True):
    return subprocess.run(["tmux", "-L", SOCKET, "-f", "/dev/null", *map(str, args)],
                          check=check, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def screen():
    return tmux("capture-pane", "-p", "-t", "leaf:0")


def save(label, text=None):
    global SCREEN_NUMBER
    SCREEN_NUMBER += 1
    text = screen() if text is None else text
    (ARTIFACTS / f"{SCREEN_NUMBER:03d}-{label}.txt").write_text(text)
    (ARTIFACTS / f"{SCREEN_NUMBER:03d}-{label}.ansi").write_text(
        tmux("capture-pane", "-p", "-e", "-t", "leaf:0"))
    return text


def wait_for(predicate, label, timeout=45):
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        last = screen()
        if predicate(last):
            return save(label, last)
        time.sleep(0.08)
    save("FAILED-" + label, last)
    raise AssertionError(f"Timeout: {label}\n{last}")


def ready(label, mode=None):
    def matches(text):
        title = text.splitlines()[0] if text else ""
        return "· ready ·" in title and (mode is None or mode in title)
    return wait_for(matches, label)


def key(*keys):
    tmux("send-keys", "-t", "leaf:0", *keys)
    time.sleep(0.15)


def search(query, label, exact=None):
    key("/")
    tmux("send-keys", "-l", "-t", "leaf:0", query)
    wait_for(lambda s: "/" + query in s and "[0/0]" not in s, label + "-query")
    if exact is not None:
        for _ in range(50):
            if re.search(r"\]\s+" + re.escape(exact) + r":", screen()):
                break
            key("Down")
        else:
            raise AssertionError("Could not select exact ID " + exact)
        save(label + "-exact-query")
    key("Enter")
    return ready(label + "-selected")


def extent(text):
    match = re.search(r"(\d+)×(\d+) cells x=(\d+) y=(\d+)", text)
    assert match, text
    return tuple(map(int, match.groups()))


def open_fixture(name, width, height, expected="ready", spacing=(8, 15, 8), initial_auto=False):
    fixture = ROOT / "tests/fixtures/diagrams" / (name + ".md")
    tmux("new-session", "-d", "-s", "leaf", "-x", width, "-y", height, "/bin/sh")
    tmux("set-option", "-t", "leaf", "status", "off")
    tmux("set-window-option", "-t", "leaf:0", "remain-on-exit", "on")
    command = ["env", "-u", "NO_COLOR", "-u", "CLICOLOR", "-u", "CLICOLOR_FORCE",
               "XDG_CONFIG_HOME=" + str(CONFIG),
               "PATH=" + str(ROOT / "tests/helpers/clipboard") + ":" + os.environ.get("PATH", ""),
               "LEAF_TEST_CLIPBOARD=" + str(CLIPBOARD), "TERM=xterm-256color",
               str(BINARY), "--theme", "ocean", "--node-spacing", str(spacing[0]),
               "--rank-spacing", str(spacing[1]), "--edge-spacing", str(spacing[2]), str(fixture)]
    tmux("respawn-pane", "-k", "-t", "leaf:0", shlex.join(command))
    document = wait_for(lambda s: "DOCUMENT-RESTORE-SENTINEL" in s
                        and "Loading diagram" not in s, f"{name}-{width}-document")
    assert "\x1b[" in tmux("capture-pane", "-p", "-e", "-t", "leaf:0"), "visual verification requires color output"
    if name in ["flowchart_large", "dense-cyclic-200"]:
        rows = document.splitlines()
        frame = next(i for i, row in enumerate(rows) if "mermaid" in row)
        label = r"Segment \d+" if name == "flowchart_large" else r"\bN\d+\b"
        first_node = next(i for i, row in enumerate(rows) if re.search(label, row))
        assert first_node - frame <= 5, "preview must show nodes immediately: " + document
        if name == "flowchart_large":
            assert "overview" in document and "200 nodes collapsed" in document, document
        else:
            assert len(set(re.findall(label, document))) >= 3, document
    key("v")
    result = wait_for(lambda s: f"· {expected} ·" in s.splitlines()[0], f"{name}-{width}-open")
    if expected == "ready":
        assert "cells" in result and any(c in result for c in "┌╭│─█"), result
        # Navigation scenarios below explicitly exercise full-detail geometry.
        # The default UI now chooses overview when the full canvas cannot fit.
        if not initial_auto and "overview" in result.splitlines()[0]:
            key("o")
            result = ready(f"{name}-{width}-full", "full")
    return fixture, result


def close_fixture(label):
    key("Escape")
    wait_for(lambda s: "DOCUMENT-RESTORE-SENTINEL" in s and "Mermaid #" not in s, label + "-restored")
    key("q")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        status = tmux("display-message", "-p", "-t", "leaf:0", "#{pane_dead}:#{pane_dead_status}").strip()
        if status == "1:0":
            break
        time.sleep(0.08)
    else:
        raise AssertionError("Leaf did not exit cleanly: " + status)
    tmux("kill-session", "-t", "leaf")


def run_architecture(width, height):
    fixture, initial = open_fixture("rookie-architecture", width, height)
    key("Right", "Right", "Down", "Down")
    panned = ready(f"architecture-{width}-pan")
    assert extent(panned)[2] > 0 and extent(panned)[3] > 0, panned
    key("NPage")
    paged = ready(f"architecture-{width}-page")
    assert extent(paged)[3] > extent(panned)[3], paged
    key("PPage", "Home")
    assert extent(ready(f"architecture-{width}-home"))[2:] == (0, 0)
    key("Tab", "BTab")
    ready(f"architecture-{width}-selection")
    key("c")
    ready(f"architecture-{width}-compact", "compact 8/15/8")
    key("c")
    ready(f"architecture-{width}-configured", "configured")
    key("d")
    ready(f"architecture-{width}-alternate", "alternate")
    key("d")
    ready(f"architecture-{width}-authored", "authored")
    key("o")
    overview = ready(f"architecture-{width}-overview", "overview")
    assert extent(overview)[0] < extent(initial)[0], overview
    search("Core", f"architecture-{width}-group")
    key("Space")
    expanded = ready(f"architecture-{width}-expand")
    assert extent(expanded)[:2] != extent(overview)[:2], expanded
    key("Space")
    ready(f"architecture-{width}-collapse")
    # Search includes m/M as text and sees hidden original nodes, not just preview.
    search("CHROMIUM", f"architecture-{width}-hidden-search")
    key("f")
    focus = ready(f"architecture-{width}-focus", "focus")
    assert re.search(r"hidden [1-9]", focus) and re.search(r"boundary [1-9]", focus), focus
    key("f")
    ready(f"architecture-{width}-unfocus")
    key("s")
    source_screen = ready(f"architecture-{width}-source", "source")
    assert "flowchart TB" in source_screen, source_screen
    key("y")
    wait_for(lambda s: "Original Mermaid copied" in s, f"architecture-{width}-copy")
    expected_source = fixture.read_text().split("```mermaid\n", 1)[1].split("```", 1)[0]
    assert CLIPBOARD.read_text() == expected_source
    key("s", "?")
    wait_for(lambda s: "Diagram controls" in s, f"architecture-{width}-help")
    key("Escape")
    tmux("resize-window", "-t", "leaf:0", "-x", max(40, width - 20), "-y", height - 5)
    time.sleep(0.4)
    resized = ready(f"architecture-{width}-resize")
    w, h, x, y = extent(resized)
    assert x <= max(0, w - (width - 20) // 2) and y <= max(0, h - (height - 9) // 2)
    close_fixture(f"architecture-{width}")
    RESULTS.append({"fixture": "architecture", "terminal": [width, height], "initial_extent": extent(initial)[:2], "all_controls": "pass"})
    print(f"PASS architecture {width}x{height}", flush=True)


def run_classes(width, height):
    _, initial = open_fixture("rookie-classes", width, height)
    search("Cookie", f"classes-{width}-search", exact="Cookie")
    key("a")
    hidden = ready(f"classes-{width}-members-hidden")
    assert extent(hidden)[:2] != extent(initial)[:2], hidden
    assert re.search(r"│\s+Cookie\s+│", hidden), "selected class moved outside viewport: " + hidden
    key("a")
    ready(f"classes-{width}-members-shown")
    close_fixture(f"classes-{width}")
    RESULTS.append({"fixture": "classes", "terminal": [width, height], "members": "pass"})
    print(f"PASS classes {width}x{height}", flush=True)


def run_large(width, height):
    _, initial = open_fixture("flowchart_large", width, height)
    assert extent(initial)[0] > width
    # Expanding an automatic overview preserves its selected group. Explicitly
    # select the first node before checking node-centered full-view navigation.
    initial = search("L1", f"large-{width}-first-node", exact="L1")
    node_row = next(i for i, row in enumerate(initial.splitlines()) if "Node 1 " in row)
    assert abs(node_row - (1 + (height - 4) // 2)) <= 1, initial
    far = search("L200", f"large-{width}-far-node")
    assert extent(far)[2] > 1000 and "Node 200" in far, far
    close_fixture(f"large-{width}")
    RESULTS.append({"fixture": "200-node", "terminal": [width, height], "far_node_pan": extent(far)[2]})
    print(f"PASS 200-node {width}x{height}", flush=True)


def run_spacing():
    for name, changed in [("spacing-nodes", (50, 15, 8)),
                          ("spacing-ranks", (8, 50, 8)),
                          ("spacing-edges", (8, 15, 50))]:
        _, compact = open_fixture(name, 120, 40)
        assert "configured 8/15/8" in compact.splitlines()[0]
        close_fixture(name + "-compact")
        _, configured = open_fixture(name, 120, 40, spacing=changed)
        label = "/".join(map(str, changed))
        assert "configured " + label in configured.splitlines()[0]
        assert compact.splitlines()[1:-3] != configured.splitlines()[1:-3], name + " only changed status text"
        key("c")
        toggled = ready(name + "-toggle-compact", "compact 8/15/8")
        assert extent(toggled)[:2] == extent(compact)[:2]
        key("c")
        restored = ready(name + "-toggle-configured", "configured " + label)
        assert extent(restored)[:2] == extent(configured)[:2]
        close_fixture(name)
        RESULTS.append({"fixture": name, "terminal": [120, 40], "configured": changed,
                        "compact_extent": extent(compact)[:2], "changed_extent": extent(configured)[:2],
                        "individual-spacing-and-toggle": "pass"})
    for name in ["flowchart_large", "dense-cyclic-200"]:
        extents = []
        overviews = []
        for spacing in [(8, 15, 8), (8, 50, 8), (50, 50, 20), (50, 15, 8), (8, 15, 20)]:
            label = name + "-spacing-" + "-".join(map(str, spacing))
            _, full = open_fixture(name, 120, 40, spacing=spacing)
            assert "configured " + "/".join(map(str, spacing)) in full.splitlines()[0]
            assert "nodes 200/200" in full
            assert "could not be emitted" not in full and "reserved node" not in full
            extents.append(extent(full)[:2])
            save(label + "-full")
            if name == "flowchart_large":
                key("o")
                overview = ready(label + "-overview", "overview")
                overviews.append(extent(overview)[:2])
            close_fixture(label)
        assert extents[1][0] > extents[0][0], (name, extents)
        if overviews:
            assert overviews[1][0] > overviews[0][0], overviews
        RESULTS.append({"fixture": name, "terminal": [120, 40], "spacing_extents": extents,
                        "overview_extents": overviews, "large-spacing": "pass"})
    print("PASS individual spacing, compact toggle, large full/overview spacing", flush=True)


def run_class_layout_consistency():
    for width, height in [(80, 30), (120, 40), (301, 72)]:
        for spacing in [(4, 8, 4), (8, 15, 8), (50, 50, 20)]:
            label = "classes-consistent-" + str(width) + "-" + "-".join(map(str, spacing))
            _, initial = open_fixture("rookie-classes", width, height, spacing=spacing)
            document = (ARTIFACTS / f"{SCREEN_NUMBER-1:03d}-rookie-classes-{width}-document.txt").read_text()
            document_extent = re.search(r"clipped (\d+)×(\d+)", document)
            assert document_extent, document
            assert tuple(map(int, document_extent.groups())) == extent(initial)[:2], "v changed the preview layout"
            assert "configured " + "/".join(map(str, spacing)) in initial.splitlines()[0]
            assert "nodes 31/31" in initial
            if width == 301 and spacing == (4, 8, 4):
                body = "\n".join(initial.splitlines()[1:-3])
                assert "CookieRecord" in body and "SourceExtraction" in body, "v opened a sparse unrelated component"
            assert "could not be emitted" not in initial and "reserved node" not in initial
            if spacing == (4, 8, 4):
                assert extent(initial)[0] <= 400 and extent(initial)[1] <= 210, initial
            key("Home")
            ready(label + "-home")
            # Changing only the viewport cannot trigger automatic label wrapping.
            tmux("resize-window", "-t", "leaf:0", "-x", max(60, width - 20), "-y", height)
            time.sleep(0.4)
            resized = ready(label + "-resized")
            assert extent(resized)[:2] == extent(initial)[:2]
            close_fixture(label)
            RESULTS.append({"fixture": "class-layout-consistency", "terminal": [width, height],
                            "spacing": spacing, "extent": extent(initial)[:2],
                            "preview-viewer-resize": "pass"})
    print("PASS class preview/viewer/resize consistency and compact extents", flush=True)


def run_tall_document():
    open_fixture("state_medium", 80, 40, spacing=(50, 50, 20), initial_auto=True)
    key("Escape")
    key("End")
    bottom = wait_for(lambda s: "State10" in s and "END-SENTINEL" in s,
                      "state-document-bottom")
    assert "clipped" not in bottom, bottom
    key("Home")
    wait_for(lambda s: "DOCUMENT-RESTORE-SENTINEL" in s, "state-document-top")
    key("v")
    ready("state-document-reopen", "full")
    close_fixture("state-document")
    RESULTS.append({"fixture": "state_medium", "full-document-scroll": "pass"})


def run_initial_fit():
    for spacing, columns, rows, overview in [((8, 20, 8), 140, 70, False),
                                            ((20, 50, 20), 140, 70, False),
                                            ((20, 50, 20), 80, 70, True)]:
        _, initial = open_fixture("flowchart_backedges_subgraphs", columns, rows,
                                  spacing=spacing, initial_auto=True)
        assert ("overview" in initial.splitlines()[0]) == overview, initial
        if not overview:
            width, height, x, y = extent(initial)
            assert width <= columns and (x, y) == (0, 0), initial
            if height > rows - 4:
                key("Down", "Down")
                scrolled = ready("initial-fit-vertical-scroll", "full")
                assert extent(scrolled)[3] > 0, scrolled
        else:
            key("o")
            full = ready("initial-fit-manual-full", "full")
            assert "nodes 12/12" in full, full
        close_fixture("initial-fit")
        RESULTS.append({"fixture": "initial-fit", "spacing": spacing,
                        "terminal": [columns, rows], "overview": overview})
    print("PASS viewport-based initial full/overview selection", flush=True)


def main():
    print("Artifacts: " + str(ARTIFACTS), flush=True)
    assert BINARY.is_file(), "Build the release executable first"
    binary_digest = hashlib.sha256(BINARY.read_bytes()).hexdigest()
    CONFIG.mkdir()
    (CONFIG / "leaf").mkdir()
    (CONFIG / "leaf/config.toml").write_text("theme='ocean'\nfile-history-length=0\n")
    try:
        run_tall_document()
        run_initial_fit()
        run_spacing()
        run_class_layout_consistency()
        for width, height in [(80, 30), (120, 40), (200, 50)]:
            run_architecture(width, height)
            run_classes(width, height)
            run_large(width, height)
        for name in ["sequence_medium", "state_medium", "flowchart_backedges_subgraphs", "pie", "unicode"]:
            _, initial = open_fixture(name, 120, 40)
            if name == "unicode":
                assert all(label in initial for label in ["東京", "é", "👩‍💻"]), initial
            if name in ["sequence_medium", "pie"]:
                key("o")
                wait_for(lambda s: "unavailable" in s, name + "-capability-feedback")
            close_fixture(name)
            RESULTS.append({"fixture": name, "terminal": [120, 40], "status": "pass"})
        fixture, _ = open_fixture("invalid", 80, 30, expected="error")
        key("s")
        wait_for(lambda s: "gantt" in s and "· source ·" in s, "invalid-source-access")
        key("y")
        expected_source = fixture.read_text().split("```mermaid\n", 1)[1].split("```", 1)[0]
        wait_for(lambda s: CLIPBOARD.exists() and CLIPBOARD.read_text() == expected_source, "invalid-copy")
        close_fixture("invalid")
        RESULTS.append({"fixture": "invalid", "source-copy": "pass"})
        run_large(270, 40)
        for width, height in [(80, 30), (120, 40), (200, 50), (270, 40)]:
            _, dense = open_fixture("dense-cyclic-200", width, height)
            assert "nodes 200/200" in dense, dense
            assert len(set(re.findall(r"\bN\d+\b", "\n".join(dense.splitlines()[1:-3])))) >= 5, dense
            first_node = next(i for i, row in enumerate(dense.splitlines()[1:-3]) if re.search(r"\bN\d+\b", row))
            assert first_node <= 4, "initial full view must not start with an empty band: " + dense
            key("Right", "Down", "/")
            tmux("send-keys", "-l", "-t", "leaf:0", "N199")
            wait_for(lambda s: "/N199" in s and "[0/0]" not in s, f"dense-{width}-node-search")
            key("Enter")
            selected = ready(f"dense-{width}-node-selected")
            assert "N199" in "\n".join(selected.splitlines()[1:-3]), selected
            close_fixture(f"dense-cyclic-{width}")
            RESULTS.append({"fixture": "dense-cyclic-200", "terminal": [width, height],
                            "nodes": 200, "edges": 269, "status": "pass"})
        assert hashlib.sha256(BINARY.read_bytes()).hexdigest() == binary_digest, "executable changed during verification"
        (ARTIFACTS / "binary.json").write_text(json.dumps({"path": str(BINARY), "sha256": binary_digest}, indent=2) + "\n")
        (ARTIFACTS / "results.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
        print(f"PASS: {len(RESULTS)} terminal cases, {SCREEN_NUMBER} captured screens", flush=True)
    finally:
        tmux("kill-server", check=False)


if __name__ == "__main__":
    main()
