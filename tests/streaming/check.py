#!/usr/bin/env python3
"""Exercise the real server and client contract against temporary Git commits."""
import http.client
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
ENV = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
           GIT_AUTHOR_NAME="Test", GIT_AUTHOR_EMAIL="test@example.invalid",
           GIT_COMMITTER_NAME="Test", GIT_COMMITTER_EMAIL="test@example.invalid")

def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], env=ENV).decode().strip()

def commit(repo, message):
    git(repo, "add", ".")
    git(repo, "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD")

def request(port, body):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    connection.request("POST", "/diff", json.dumps(body), {"Content-Type": "application/json"})
    response = connection.getresponse()
    if response.status != 200:
        data = json.loads(response.read())
        connection.close()
        return response.status, data
    assert response.getheader("Content-Type") == "application/x-ndjson"
    first = json.loads(response.readline())  # Parse before consuming the response.
    assert first["type"] == "start"
    events = [first]
    while line := response.readline():
        events.append(json.loads(line))
    connection.close()
    assert events[-1]["type"] == "complete"
    return 200, events

def serve(repo, config=None):
    args = [str(ROOT / "target/debug/difft"), "serve", "--repo", str(repo), "--listen", "127.0.0.1:0"]
    if config:
        args += ["--config", str(config)]
    process = subprocess.Popen(args, stderr=subprocess.PIPE, stdout=subprocess.DEVNULL, env=ENV, text=True)
    line = process.stderr.readline().strip()
    assert line.startswith("Listening on http://127.0.0.1:"), line
    return process, int(line.rsplit(":", 1)[1])

with tempfile.TemporaryDirectory(prefix="diffr-stream-test-") as temp:
    repo = Path(temp)
    git(repo, "init", "-q")
    (repo / "a.rs").write_text("fn run() { old(); }\n")
    (repo / "remove.py").write_text("print('remove')\n")
    (repo / "rename.py").write_text("print('same content')\n")
    base = commit(repo, "base")
    (repo / "a.rs").write_text("fn run() { old(); new(); }\n")
    (repo / "remove.py").unlink()
    (repo / "rename.py").rename(repo / "renamed.py")
    (repo / "binary.bin").write_bytes(b"a\0b")
    (repo / "z.py").write_text("print('new')\n")
    head = commit(repo, "head")
    # Uncommitted workspace attributes and config must apply to these pinned refs.
    (repo / ".gitattributes").write_text("*.rs diffr-classify=source\n*.py diffr-classify=test\n*.bin diffr-classify=generated\n")
    (repo / "diffr.toml").write_text('[files]\norder = ["test", "source", "generated"]\n[languages.rust]\nfolds = ""\n')
    process, port = serve(repo)
    try:
        status, events = request(port, dict(base=base, head=head, include_layout=True))
        assert status == 200
        assert events[0]["base"] == base and events[0]["head"] == head
        files = [e for e in events if e["type"] in ("file", "file_error")]
        assert len(files) == events[0]["total"] == 5
        assert [e["file"]["class"] for e in files] == ["test", "test", "test", "source", "generated"]
        assert events[-1] == dict(type="complete", succeeded=4, failed=1)
        assert files[-1]["type"] == "file_error"
        renamed = next(e["file"] for e in files if e["file"]["status"] == "renamed")
        assert renamed["old_path"] == "rename.py" and renamed["new_path"] == "renamed.py"
        rust = next(e for e in files if e["file"]["new_path"] == "a.rs")
        assert rust["diff"]["rhs_folds"] == [] and "layout" in rust
        _, events = request(port, dict(base=base, head=head, order=["source"], paths=["a.rs", "z.py"]))
        assert events[1]["file"]["new_path"] == "a.rs"
        assert "layout" not in events[1]
        _, events = request(port, dict(base=base, head=head, order=["generated"]))
        assert events[1]["type"] == "file_error" and events[-1]["succeeded"] == 4
        _, events = request(port, dict(base=head, head=head))
        assert [e["type"] for e in events] == ["start", "complete"]
        _, events = request(port, dict(base=head, head=head, paths=["a.rs"]))
        assert events[1]["file"]["status"] == "unchanged"
        assert request(port, dict(base="missing-ref", head=head))[0] == 400
        assert request(port, dict(base=base, head=head, paths=["missing.rs"]))[0] == 400
        # The server keeps its compiled config even if the file changes.
        (repo / "diffr.toml").write_text("invalid toml")
        assert request(port, dict(base=base, head=head, paths=["a.rs"]))[0] == 200
    finally:
        process.terminate()
        process.wait(timeout=10)
    explicit = repo / "custom.toml"
    explicit.write_text('[files]\norder = ["source"]\n')
    process, port = serve(repo, explicit)
    try:
        _, events = request(port, dict(base=base, head=head, paths=["a.rs"]))
        assert events[1]["diff"]["rhs_folds"], "explicit config replaces repo config; default Rust folds survive"
    finally:
        process.terminate()
        process.wait(timeout=10)
print("Streaming integration checks passed")
