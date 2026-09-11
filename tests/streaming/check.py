#!/usr/bin/env python3
"""Exercise the CLI stream against real Git repositories and partial staging."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "target/debug/diffr"
# An empty global configuration and no model API key, whatever the machine has.
CONFIG_HOME = tempfile.mkdtemp(prefix="diffr-config-home-")
ENV = dict(
    {
        k: v
        for k, v in os.environ.items()
        if k not in ("GEMINI_API_KEY", "GOOGLE_API_KEY")
    },
    XDG_CONFIG_HOME=CONFIG_HOME,
    GIT_CONFIG_GLOBAL="/dev/null",
    GIT_CONFIG_NOSYSTEM="1",
    GIT_AUTHOR_NAME="Test",
    GIT_AUTHOR_EMAIL="test@example.invalid",
    GIT_COMMITTER_NAME="Test",
    GIT_COMMITTER_EMAIL="test@example.invalid",
)


def git(repo, *args):
    return (
        subprocess.check_output(["git", "-C", str(repo), *args], env=ENV)
        .decode()
        .strip()
    )


def commit(repo, message):
    git(repo, "add", ".")
    git(repo, "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD")


def cli(repo, *args):
    return subprocess.run(
        [str(EXE), "--repo", str(repo), *args],
        capture_output=True,
        env=ENV,
        check=False,
    )


def path_of(record):
    file = record["file"]
    return (file.get("rhs") or file["lhs"])["path"]


def stream(repo, *args, code=0):
    with subprocess.Popen(
        [str(EXE), "--repo", str(repo), "--format", "ndjson", *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=ENV,
    ) as process:
        first = json.loads(process.stdout.readline())
        assert first["type"] == "start" and first["version"] == 2, first
        events = [first, *[json.loads(line) for line in process.stdout]]
        stderr = process.stderr.read()
        assert process.wait(timeout=30) == code, stderr
    assert events[-1]["type"] == "complete"
    files = events[1:-1]
    assert all(e["type"] == "file" for e in files)
    succeeded = sum("diff" in e for e in files)
    failed = sum("error" in e for e in files)
    assert all(("diff" in e) != ("error" in e) for e in files)
    complete = events[-1]
    assert complete["succeeded"] == succeeded, complete
    if "aborted" in complete:
        assert complete["failed"] >= failed and code == 2
    else:
        assert complete["failed"] == failed
        assert succeeded + failed == len(first["files"])
    manifest = {json.dumps(f["file"], sort_keys=True) for f in first["files"]}
    for record in files:
        assert json.dumps(record["file"], sort_keys=True) in manifest
    for line in json.dumps(events).split('"'):
        assert line != "null"
    return events


def leaves(regions):
    for region in regions:
        if region["kind"] == "leaf":
            yield region
        else:
            yield from leaves(region["children"])


def all_regions(regions):
    for region in regions:
        yield region
        if region["kind"] == "fold":
            yield from all_regions(region["children"])


def check_tiling(source):
    at = 0
    for leaf in leaves(source["regions"]):
        assert leaf["start"] == {"line": at, "column": 0}, (leaf, at)
        assert leaf["end"]["column"] == 0 and leaf["end"]["line"] > at
        at = leaf["end"]["line"]
    text = source["text"]
    lines = text.count("\n") + (1 if text and not text.endswith("\n") else 0)
    assert at == lines, (at, lines)


with tempfile.TemporaryDirectory(prefix="diffr-stream-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    (repo / "a.rs").write_text("fn run() {\n    old();\n}\n")
    (repo / "remove.py").write_text("print('remove')\n")
    (repo / "rename.py").write_text("print('same content')\n")
    base = commit(repo, "base")
    (repo / "a.rs").write_text("fn run() {\n    old();\n    new();\n}\n")
    (repo / "remove.py").unlink()
    (repo / "rename.py").rename(repo / "renamed.py")
    (repo / "binary.bin").write_bytes(b"a\0b")
    (repo / "z.py").write_text("print('new')\n")
    head = commit(repo, "head")
    (repo / ".gitattributes").write_text(
        "*.rs diffr-classify=source\n*.py diffr-classify=test\n*.bin diffr-classify=generated\n"
    )
    (repo / "diffr.toml").write_text('[languages.rust]\nfolds = ""\n')
    events = stream(repo, base, head, "--order", "test,source,generated", code=2)
    assert events[0]["lhs"] == {"type": "revision", "rev": base}
    assert events[0]["rhs"] == {"type": "revision", "rev": head}
    manifest = events[0]["files"]
    # Results arrive in completion order; --order governs computation priority only.
    assert sorted(f["category"] for f in manifest) == [
        "generated",
        "source",
        "test",
        "test",
        "test",
    ]
    renamed = next(f for f in manifest if f["status"] == "renamed")
    assert renamed["file"]["lhs"]["path"] == "rename.py"
    assert renamed["file"]["rhs"]["path"] == "renamed.py"
    assert renamed["file"]["lhs"]["mode"] == "100644"
    assert len(renamed["file"]["rhs"]["oid"]) == 40
    assert next(f for f in manifest if path_of(f) == "a.rs")["language"] == "Rust"
    assert next(f for f in manifest if path_of(f) == "remove.py")["status"] == "deleted"
    assert "rhs" not in next(f for f in manifest if path_of(f) == "remove.py")["file"]
    records = {path_of(e): e for e in events[1:-1]}
    binary = records["binary.bin"]["error"]
    assert binary["code"] == "binary" and binary["message"]
    rust = records["a.rs"]["diff"]
    assert rust["type"] == "text"
    assert not any(r["kind"] == "fold" for r in all_regions(rust["rhs"]["regions"]))
    assert "syntax" not in rust["rhs"]
    assert rust["stats"]["textual"] == {"added": 1, "removed": 0}
    assert rust["stats"]["structural"] == {"added": 1, "removed": 0}
    check_tiling(rust["lhs"])
    check_tiling(rust["rhs"])
    changed = [leaf for leaf in leaves(rust["rhs"]["regions"]) if "changed" in leaf]
    assert changed[0]["changed"] == [{"line": 2, "start_column": 0, "end_column": 10}]
    removed = records["remove.py"]["diff"]
    assert "rhs" not in removed and check_tiling(removed["lhs"]) is None
    assert removed["stats"]["textual"] == {"added": 0, "removed": 1}
    added = records["z.py"]["diff"]
    assert "lhs" not in added and next(leaves(added["rhs"]["regions"]))["changed"]
    same = records["renamed.py"]["diff"]
    only = list(leaves(same["lhs"]["regions"]))
    assert only[0]["visibility"] == {"collapsed": True, "label": "1 unchanged line"}
    assert only[0]["id"] == next(leaves(same["rhs"]["regions"]))["id"]
    # Syntax spans are opt-in and carry tree-sitter capture names.
    events = stream(repo, base, head, "--syntax", "--", "a.rs")
    syntax = events[1]["diff"]["rhs"]["syntax"]
    assert {span["capture"] for span in syntax} >= {"keyword", "function"}, syntax
    assert all(span["start_column"] < span["end_column"] for span in syntax)
    # An early file failure must not prevent the later successes.
    events = stream(repo, base, head, "--order", "generated", code=2)
    assert sum("error" in e for e in events[1:-1]) == 1 and events[-1]["succeeded"] == 4
    events = stream(repo, base, head, "--", "a.rs", "z.py")
    assert sorted(path_of(e) for e in events[1:-1]) == ["a.rs", "z.py"]
    events = stream(repo, base, head, "--jobs", "1", "--", "a.rs", "z.py")
    assert [path_of(e) for e in events[1:-1]] == ["a.rs", "z.py"]
    assert len(stream(repo, head, head)) == 2
    assert len(stream(repo, base, head, "--", "missing.rs")) == 2
    stream(repo, base, head, "--exit-code", "--", "a.rs", code=1)
    for args in (
        ["bad-ref", head],
        [base, head, "--quiet"],
        ["--no-index", "a", "b"],
        [base, head, "--stat"],
    ):
        result = cli(repo, "--format", "ndjson", *args)
        assert result.returncode == 2 and not result.stdout and result.stderr
    (repo / "diffr.toml").write_text("invalid toml")
    assert cli(repo, "--format", "ndjson", base, head).returncode == 2
    # --config replaces the global file; the repository file still layers on top.
    (repo / "diffr.toml").write_text("")
    custom = repo / "custom.toml"
    custom.write_text("[folds]\nmin_lines = 2\n")
    events = stream(repo, base, head, "--config", str(custom), "--", "a.rs")
    assert any(
        r["kind"] == "fold" for r in all_regions(events[1]["diff"]["rhs"]["regions"])
    )
    # The structural limits come from [diff]; exceeding one names the key.
    events = stream(repo, base, head, "--", "a.rs")
    assert "structural" in events[1]["diff"]["stats"], events[1]["diff"]["stats"]
    for args in (["--graph-limit", "1"], ["--set", "diff.graph_limit=1"]):
        events = stream(repo, base, head, *args, "--", "a.rs")
        fallback = events[1]["diff"]["stats"]["fallback"]
        assert fallback["code"] == "too_complex", fallback
        assert "diff.graph_limit (1)" in fallback["message"], fallback
        # The parse still stands in fallback: folds are present, paired
        # through the line alignment.
        assert any(
            r["kind"] == "fold"
            for r in all_regions(events[1]["diff"]["rhs"]["regions"])
        ), events[1]["diff"]["rhs"]["regions"]
    # So do highlight spans: the language is guessed from the path.
    events = stream(repo, base, head, "--graph-limit", "1", "--syntax", "--", "a.rs")
    for side in ("lhs", "rhs"):
        assert events[1]["diff"][side]["syntax"], side
    # The previous stream stays available while frontends migrate.
    v1 = cli(
        repo, "--format", "ndjson-v1", "--config", str(custom), base, head, "--", "a.rs"
    )
    assert json.loads(v1.stdout.splitlines()[0])["version"] == 1

with tempfile.TemporaryDirectory(prefix="diffr-operands-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    source = repo / "a.rs"
    initial = "fn run() { initial(); }\n"
    staged = "fn run() { staged(); }\n"
    working = "fn run() { working(); }\n"
    source.write_text(initial)
    base = commit(repo, "initial")
    source.write_text(staged)
    git(repo, "add", "a.rs")
    source.write_text(working)
    for selection, left, right in (
        ([], staged, working),
        (["--cached"], initial, staged),
        ([base], initial, working),
    ):
        for reverse in (False, True):
            args = [*selection, *(["-R"] if reverse else [])]
            for output in ("--name-only", "--numstat"):
                actual = cli(repo, *args, output)
                expected = subprocess.check_output(
                    ["git", "-C", str(repo), "diff", *args, output], env=ENV
                )
                assert actual.returncode == 0 and actual.stdout == expected
            diff = stream(repo, *args)[1]["diff"]
            assert diff["lhs"]["text"] == (right if reverse else left)
            assert diff["rhs"]["text"] == (left if reverse else right)
    assert cli(repo, "--quiet").returncode == 1
    assert cli(repo, base, "HEAD", "--exit-code").returncode == 0
    git(repo, "rm", "-f", "a.rs")
    source.write_text(working)
    assert stream(repo, base)[0]["files"][0]["status"] == "deleted"
    assert cli(repo, base, "--name-status").stdout == b"D\ta.rs\n"

with tempfile.TemporaryDirectory(prefix="diffr-unborn-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    (repo / "new.rs").write_text("fn new() {}\n")
    git(repo, "add", ".")
    events = stream(repo, "--cached")
    assert events[0]["lhs"] == {"type": "empty_tree"}
    assert events[0]["files"][0]["status"] == "added"

# A standalone comparison streams the same three records.
with tempfile.TemporaryDirectory(prefix="diffr-no-index-") as temp:
    before = Path(temp) / "before.py"
    after = Path(temp) / "after.py"
    before.write_text("def f():\n    return 1\n")
    after.write_text("def f():\n    return 2\n")
    result = subprocess.run(
        [str(EXE), "--no-index", "--format", "ndjson", "--", str(before), str(after)],
        capture_output=True,
        env=ENV,
        check=True,
    )
    events = [json.loads(line) for line in result.stdout.splitlines()]
    assert events[0]["lhs"] == {"type": "path", "path": str(before)}
    assert events[1]["file"]["lhs"]["path"] == str(before)
    assert events[1]["diff"]["stats"]["structural"] == {"added": 1, "removed": 1}
    assert events[2] == {"type": "complete", "succeeded": 1, "failed": 0}

# A configured fold hook fills fold labels before each file record; a hook
# failure ends the run.
with tempfile.TemporaryDirectory(prefix="diffr-hook-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    git(repo, "commit", "--allow-empty", "-qm", "empty")
    base = git(repo, "rev-parse", "HEAD")
    rpc_server = ROOT / "tests/hooks/rpc_server.py"
    large = "def f():\n    a()\n    b()\n    c()\n\ndef g():\n    d()\n"
    (repo / "good.py").write_text(large)
    (repo / "bad.py").write_text(large)
    (repo / "small.py").write_text("def h():\n    e()\n")
    head = commit(repo, "additions")

    def hook_config(mode, *extra):
        command = [sys.executable, str(rpc_server), mode, *extra]
        return f"[folds.hook]\ncommand = {json.dumps(command)}\ntags = ['body']\nmin_lines = 3\n"

    def fold_labels(record):
        return [
            r["visibility"]["label"]
            for r in all_regions(record["diff"]["rhs"]["regions"])
            if r["kind"] == "fold" and "body" in r["tags"]
        ]

    (repo / "diffr.toml").write_text(hook_config("echo"))
    events = {path_of(e): e for e in stream(repo, base, head)[1:-1]}
    assert fold_labels(events["good.py"]) == ["# pseudocode\npseudo Body"]
    assert fold_labels(events["small.py"]) == []
    (repo / "diffr.toml").write_text(hook_config("error"))
    events = stream(repo, base, head, "--jobs", "1", code=2)
    assert events[-1]["aborted"]["code"] == "hook_failed"
    assert "declined" in events[-1]["aborted"]["message"]
    (repo / "diffr.toml").write_text("[folds.hook]\ncommand = ['./missing-hook']\n")
    assert cli(repo, "--format", "ndjson", base, head).returncode == 2
    (repo / "diffr.toml").write_text(hook_config("exit"))
    assert cli(repo, "--format", "ndjson", base, head).returncode == 2
    # Relative hook paths resolve against the config file, not the repository.
    # The repository file still layers over --config, so clear it first.
    (repo / "diffr.toml").write_text("")
    with tempfile.TemporaryDirectory(prefix="diffr-hook-config-") as elsewhere:
        shutil.copy(rpc_server, Path(elsewhere) / "hook.py")
        (Path(elsewhere) / "hook.toml").write_text(
            f"[folds.hook]\ncommand = [{json.dumps(sys.executable)}, 'hook.py', 'cwd', {json.dumps(str(repo.resolve()) + os.sep)}]\ntags = ['body']\nmin_lines = 3\n"
        )
        events = stream(
            repo,
            base,
            head,
            "--config",
            str(Path(elsewhere) / "hook.toml"),
            "--",
            "good.py",
        )
        assert fold_labels(events[1])[0] == "# pseudocode\n" + str(
            Path(elsewhere).resolve()
        )

# Configuration: one layered file, the three commands frontends drive, and
# the default collapse rules on the wire.
with tempfile.TemporaryDirectory(prefix="diffr-config-") as temp:
    repo = Path(temp) / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    (repo / "src").mkdir()
    (repo / "src" / "lib.py").write_text(
        "def gone():\n    a()\n    b()\n    c()\n\ndef kept():\n    a()\n"
    )
    (repo / "Cargo.lock").write_text("[[package]]\nname = 'a'\n")
    base = commit(repo, "base")
    (repo / "src" / "lib.py").write_text("def kept():\n    a()\n")
    (repo / "Cargo.lock").write_text("[[package]]\nname = 'b'\n")
    (repo / "test_lib.py").write_text("def test_kept():\n    assert True\n")
    head = commit(repo, "head")
    env = dict(ENV, XDG_CONFIG_HOME=str(Path(temp) / "home"))
    config_file = Path(temp) / "home" / "diffr" / "config.toml"

    def config(*args, code=0):
        result = subprocess.run(
            [str(EXE), "--repo", str(repo), "config", *args],
            capture_output=True,
            env=env,
            check=False,
        )
        assert result.returncode == code, result.stderr
        return result.stdout.decode()

    schema = json.loads(config("schema"))
    assert schema["properties"]["folds"] and schema["properties"]["diff"]
    shown = json.loads(config("show", "--json"))
    assert shown["folds"]["min_lines"] == 12 and shown["summarize"]["api_key"] is None
    config("set", "folds.min_lines", "3")
    config("set", "summarize.api_key", "1234")
    config("set", "folds.typo", "1", code=2)
    text = config_file.read_text()
    assert "min_lines = 3" in text and 'api_key = "1234"' in text, text
    shown = json.loads(config("show", "--json"))
    assert shown["folds"]["min_lines"] == 3
    assert shown["summarize"]["api_key"] == "<redacted>"
    assert (
        json.loads(config("show", "--json", "--reveal"))["summarize"]["api_key"]
        == "1234"
    )
    result = subprocess.run(
        [
            str(EXE),
            "--repo",
            str(repo),
            "--set",
            "folds.min_lines=7",
            "config",
            "show",
            "--json",
        ],
        capture_output=True,
        env=env,
        check=True,
    )
    assert json.loads(result.stdout)["folds"]["min_lines"] == 7

    # No key in the environment and summarize disabled: the rules still run.
    config("set", "summarize.enabled", "false")
    with subprocess.Popen(
        [str(EXE), "--repo", str(repo), "--format", "ndjson", base, head],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    ) as process:
        events = [json.loads(line) for line in process.stdout]
        assert process.wait(timeout=30) == 0, process.stderr.read()
    manifest = {
        (f["file"].get("rhs") or f["file"]["lhs"])["path"]: f
        for f in events[0]["files"]
    }
    assert manifest["Cargo.lock"]["category"] == "generated"
    assert manifest["Cargo.lock"]["visibility"] == {
        "collapsed": True,
        "label": "Generated file · hidden by default",
    }
    assert (
        manifest["test_lib.py"]["visibility"]["label"]
        == "Test file · hidden by default"
    )
    assert "visibility" not in manifest["src/lib.py"]
    lib = next(e for e in events[1:-1] if path_of(e) == "src/lib.py")
    deleted = [
        r
        for r in all_regions(lib["diff"]["lhs"]["regions"])
        if r["kind"] == "fold" and r.get("visibility", {}).get("collapsed")
    ]
    assert [r["visibility"]["label"] for r in deleted] == ["3 lines removed"], deleted
    config("set", "folds.collapse_deleted", "false")
    config("set", "folds.collapse_generated", "false")
    with subprocess.Popen(
        [str(EXE), "--repo", str(repo), "--format", "ndjson", base, head],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    ) as process:
        events = [json.loads(line) for line in process.stdout]
        assert process.wait(timeout=30) == 0, process.stderr.read()
    assert "visibility" not in next(
        f for f in events[0]["files"] if path_of(f) == "Cargo.lock"
    )
    lib = next(e for e in events[1:-1] if path_of(e) == "src/lib.py")
    assert not any(
        r.get("visibility", {}).get("collapsed")
        for r in all_regions(lib["diff"]["lhs"]["regions"])
        if r["kind"] == "fold"
    )

# Closing the pipe while a multi-file producer is active must not leave it
# blocked forever on a full queue. Unix CLI output retains normal SIGPIPE behavior.
if os.name == "posix":
    with tempfile.TemporaryDirectory(prefix="diffr-cancel-") as temp:
        repo = Path(temp)
        git(repo, "init", "-q")
        git(repo, "commit", "--allow-empty", "-qm", "empty")
        base = git(repo, "rev-parse", "HEAD")
        for index in range(8):
            (repo / f"{index}.txt").write_text("some new text\n" * 4096)
        head = commit(repo, "large additions")
        with subprocess.Popen(
            [str(EXE), "--repo", str(repo), base, head, "--format", "ndjson"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=ENV,
        ) as process:
            assert json.loads(process.stdout.readline())["type"] == "start"
            process.stdout.close()
            assert process.wait(timeout=30) != 0
print("CLI streaming, cancellation and Git operand checks passed")
