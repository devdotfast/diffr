#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""Reference diffr fold hook: rewrite large novel folds as Python-style pseudocode.

diffr writes one NDJSON request per file on stdin and reads one reply per request
on stdout, matched by id. Requests are answered concurrently, so replies arrive
in whatever order the model finishes; diffr routes them by id.

Environment:
  GOOGLE_API_KEY         required
  DIFFR_SUMMARY_MODEL    default gemini-3.8-flash
  DIFFR_SUMMARY_WORKERS  concurrent requests, default 16
"""
import json
import os
import sys
import threading
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

API_KEY = os.environ["GOOGLE_API_KEY"]
MODEL = os.environ.get("DIFFR_SUMMARY_MODEL", "gemini-3.8-flash")
WORKERS = int(os.environ.get("DIFFR_SUMMARY_WORKERS", "16"))
URL = f"https://generativelanguage.googleapis.com/v1beta/models/{MODEL}:generateContent"

SYSTEM = (
    "You rewrite regions of a source file as terse Python-style pseudocode for a diff "
    "viewer that shows the pseudocode in place of the collapsed region. The user supplies "
    "one numbered source file and a list of folds, each with an id and 1-based line range. "
    "For each fold, write pseudocode covering only that fold's lines: keep the control flow "
    "and the names that matter, drop types, error plumbing and boilerplate. Aim for about one "
    "pseudocode line per five source lines, between one and eight lines per fold. Reply with "
    "one {id, pseudocode} object per fold."
)
SCHEMA = {
    "type": "ARRAY",
    "items": {
        "type": "OBJECT",
        "properties": {"id": {"type": "INTEGER"}, "pseudocode": {"type": "STRING"}},
        "required": ["id", "pseudocode"],
    },
}

write_lock = threading.Lock()


def prompt(request):
    numbered = "\n".join(
        f"{number:5d} | {line}" for number, line in enumerate(request["src"].splitlines(), 1)
    )
    folds = "\n".join(
        f"- fold {fold['id']}: lines {fold['range']['start']['line'] + 1}-"
        f"{fold['range']['end']['line'] + 1}"
        for fold in request["folds"]
    )
    language = request["language"] or "unknown language"
    return f"File {request['path']} ({language}):\n\n{numbered}\n\nFolds:\n{folds}"


def complete(request):
    body = {
        "systemInstruction": {"parts": [{"text": SYSTEM}]},
        "contents": [{"role": "user", "parts": [{"text": prompt(request)}]}],
        "generationConfig": {
            "temperature": 0,
            "maxOutputTokens": 160 * len(request["folds"]) + 100,
            "thinkingConfig": {"thinkingBudget": 0},
            "responseMimeType": "application/json",
            "responseSchema": SCHEMA,
        },
    }
    http = urllib.request.Request(
        URL,
        data=json.dumps(body).encode(),
        headers={"x-goog-api-key": API_KEY, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(http, timeout=60) as response:
        data = json.load(response)
    content = data["candidates"][0]["content"]["parts"][-1]["text"]
    expected = {fold["id"] for fold in request["folds"]}
    texts = {}
    for item in json.loads(content):
        if item["id"] not in expected:
            raise ValueError(f"model answered for unknown fold {item['id']}: {content[:200]}")
        if item["pseudocode"].strip():
            texts[str(item["id"])] = item["pseudocode"].strip()
    return texts


def answer(line):
    request = json.loads(line)
    try:
        reply = {"id": request["id"], "texts": complete(request)}
    except urllib.error.HTTPError as error:
        reply = {"id": request["id"], "error": f"{MODEL}: HTTP {error.code} {error.read()[:200]!r}"}
    except (OSError, ValueError, KeyError) as error:
        reply = {"id": request["id"], "error": f"{MODEL}: {error}"}
    with write_lock:
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


def main():
    with ThreadPoolExecutor(WORKERS) as pool:
        for line in sys.stdin:
            if line.strip():
                pool.submit(answer, line)


if __name__ == "__main__":
    main()
