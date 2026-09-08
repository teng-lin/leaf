#!/usr/bin/env python3
"""Black-box checks against the actual release binary, using private config."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "target/release/leaf"
ARTIFACTS = Path(tempfile.mkdtemp(prefix="leaf-diagram-cli."))
ENV = {key: value for key, value in os.environ.items() if not key.startswith(("LEAF_", "MMDFLUX_"))}
ENV["XDG_CONFIG_HOME"] = str(ARTIFACTS / "config")
CONFIG = ARTIFACTS / "config/leaf/config.toml"
CONFIG.parent.mkdir(parents=True)
CONFIG.write_text("[mermaid]\nnode-spacing=8\nrank-spacing=15\nedge-spacing=8\n")
RESULTS = []
SOURCE = (ROOT / "tests/fixtures/diagrams/rookie-architecture.md").read_text()


def run(label, args, source=SOURCE, extra_env=None, success=True):
    result = subprocess.run([str(BINARY), *args], input=source, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            env={**ENV, **(extra_env or {})}, timeout=30)
    (ARTIFACTS / (label + ".stdout")).write_text(result.stdout)
    (ARTIFACTS / (label + ".stderr")).write_text(result.stderr)
    assert (result.returncode == 0) == success, (label, result.returncode, result.stderr)
    RESULTS.append({"case": label, "exit": result.returncode, "stdout_bytes": len(result.stdout.encode())})
    return result.stdout, result.stderr


def main():
    print("Artifacts: " + str(ARTIFACTS), flush=True)
    binary_digest = hashlib.sha256(BINARY.read_bytes()).hexdigest()
    help_text, _ = run("help", ["--help"])
    assert all(flag in help_text for flag in ["--node-spacing", "--rank-spacing", "--edge-spacing", "--mermaid-full"])
    for invalid in ["NaN", "inf", "-1", "501", "invalid"]:
        _, error = run("invalid-" + invalid, ["--node-spacing", invalid, "--inline"], success=False)
        assert "finite number" in error
    run("full-requires-inline", ["--mermaid-full"], success=False)
    compact, _ = run("config-compact", ["--inline", "plain:80", "--mermaid-full"])
    assert max(map(len, compact.splitlines())) > 80
    assert "direct Rust consumers" in compact and "browser::chromium" in compact
    baseline, _ = run("cli-override", ["--inline", "plain:80", "--mermaid-full", "--node-spacing", "50", "--rank-spacing", "50", "--edge-spacing", "20"])
    assert baseline != compact
    from_env, _ = run("env-override", ["--inline", "plain:80", "--mermaid-full"], extra_env={
        "LEAF_MERMAID_NODE_SPACING": "50", "LEAF_MERMAID_RANK_SPACING": "50", "LEAF_MERMAID_EDGE_SPACING": "20"})
    assert from_env == baseline
    for index in range(5):
        repeated, _ = run(f"repeat-default-{index}", ["--inline", "plain:80", "--mermaid-full", "--node-spacing", "50", "--rank-spacing", "50", "--edge-spacing", "20"])
        assert repeated == baseline, "fresh-process rendering must be deterministic"
        repeated_compact, _ = run(f"repeat-compact-{index}", ["--inline", "plain:80", "--mermaid-full"])
        assert repeated_compact == compact, "fresh-process compact rendering must be deterministic"
    invalid_env, _ = run("invalid-env-falls-through", ["--inline", "plain:80", "--mermaid-full"], extra_env={
        "LEAF_MERMAID_NODE_SPACING": "NaN", "LEAF_MERMAID_RANK_SPACING": "-1", "LEAF_MERMAID_EDGE_SPACING": "no"})
    assert invalid_env == compact
    cli_wins, _ = run("cli-over-env", ["--inline", "plain:80", "--mermaid-full", "--node-spacing=8", "--rank-spacing=15", "--edge-spacing=8"], extra_env={
        "LEAF_MERMAID_NODE_SPACING": "50", "LEAF_MERMAID_RANK_SPACING": "50", "LEAF_MERMAID_EDGE_SPACING": "20"})
    assert cli_wins == compact
    # Compare canvas geometry, not status text containing the option values.
    for name, graph, flag in [
        ("node", "flowchart TD\nA[Alpha]-->B[Bravo]\nA-->C[Gamma]", "--node-spacing"),
        ("rank", "flowchart LR\nA[Alpha]-->B[Bravo]-->C[Gamma]", "--rank-spacing"),
        ("edge", "flowchart TD\nA-->B-->C-->D\nA-->D\nB-->D", "--edge-spacing"),
    ]:
        source = "```mermaid\n" + graph + "\n```\n"
        base, _ = run("spacing-" + name + "-compact", ["--inline", "plain:120", "--mermaid-full"], source=source)
        changed, _ = run("spacing-" + name + "-changed", ["--inline", "plain:120", "--mermaid-full", flag, "50"], source=source)
        canvas = lambda text: "\n".join(row for row in text.splitlines() if not any(hint in row for hint in ["v open", "spacing", "mermaid"]))
        assert "warning" not in base and "warning" not in changed
        assert canvas(base) != canvas(changed), name + " changed only metadata, not diagram geometry"
    grouped = (ROOT / "tests/fixtures/diagrams/flowchart_large.md").read_text()
    small, _ = run("spacing-overview-compact", ["--inline", "plain:120"], source=grouped)
    large, _ = run("spacing-overview-rank-50", ["--inline", "plain:120", "--rank-spacing", "50"], source=grouped)
    group_gap = lambda text: next(row.index("Segment 2") - row.index("Segment 1") for row in text.splitlines() if "Segment 1" in row and "Segment 2" in row)
    assert group_gap(large) > group_gap(small), "overview still masks rank spacing"
    assert "spacing 8/15/8" in small and "spacing 8/50/8" in large
    preview, _ = run("clipped-preview", ["--inline", "plain:80"])
    assert "clipped" in preview and "--mermaid-full" in preview
    assert max(map(len, preview.splitlines())) <= 80
    ansi, _ = run("ansi-full", ["--inline", "ansi:80", "--mermaid-full"])
    assert "\x1b[" in ansi
    assert re.sub(r"\x1b\[[0-9;]*m", "", ansi) == compact
    for name in ["flowchart_large", "dense-cyclic-200"]:
        source = (ROOT / "tests/fixtures/diagrams" / (name + ".md")).read_text()
        for width in [80, 120, 200, 270]:
            preview, _ = run(f"{name}-preview-{width}", ["--inline", f"plain:{width}"], source=source)
            rows = preview.splitlines()
            frame = next(i for i, row in enumerate(rows) if "mermaid" in row)
            label = r"Segment \d+" if name == "flowchart_large" else r"\bN\d+\b"
            first_node = next(i for i, row in enumerate(rows) if re.search(label, row))
            assert first_node - frame <= 5, preview
            assert max(map(len, rows)) <= width
            assert "v open" in preview and "--mermaid-full" in preview
            if name == "flowchart_large":
                assert "overview" in preview and "200 nodes collapsed" in preview
            else:
                assert len(set(re.findall(label, preview))) >= 3
        full, _ = run(f"{name}-full", ["--inline", "plain:80", "--mermaid-full"], source=source)
        label = r"Node \d+" if name == "flowchart_large" else r"\bN\d+\b"
        assert len(set(re.findall(label, full))) == 200, "full export must retain every node"
    raw = "x" * 50_000 + "\n" + "short\n" * 1000
    fallback, _ = run("bounded-rejected-source", ["--inline", "plain:80", "--mermaid-full"], source="```mermaid\n" + raw + "```\n")
    assert raw in fallback and len(fallback) < len(raw) + 1000
    over_limit = "x" * (1_048_576 + 1)
    rejected, _ = run("source-admission-limit", ["--inline", "plain:80"], source="```mermaid\n" + over_limit + "\n```\n")
    assert "warning" in rejected and len(rejected) < 2000
    empty, _ = run("empty-mermaid", ["--inline", "plain:80"], source="```mermaid\n```\n")
    assert "warning" in empty
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest() == binary_digest, "executable changed during verification"
    (ARTIFACTS / "binary.json").write_text(json.dumps({"path": str(BINARY), "sha256": binary_digest}, indent=2) + "\n")
    (ARTIFACTS / "results.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
    print(f"PASS: {len(RESULTS)} CLI cases", flush=True)


if __name__ == "__main__":
    main()
