#!/usr/bin/env python3
"""rumu stdio demo plugin (Python 3 stdlib only): a `clock.now` tool plus a
`log-events` hook that logs fired events to stderr.

Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout."""

import datetime
import json
import sys


def send(msg):
    sys.stdout.buffer.write((json.dumps(msg) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()


def log(text):
    print(f"[clock] {text}", file=sys.stderr, flush=True)


MANIFEST = {
    "name": "clock",
    "instructions": "Provides `now`, the current local time; use it whenever "
    "'now' or 'today' matters instead of guessing.",
    "tools": [
        {
            "name": "now",
            "description": "Current local time as an ISO-8601 string "
            "(e.g. 2026-01-01T00:00:00+00:00).",
            "schema": {
                "type": "object",
                "properties": {},
                "additionalProperties": False,
            },
        }
    ],
    "hooks": [
        {
            "id": "log-events",
            "match": {"event": "prompt"},
        }
    ],
}


def handle(req):
    method = req.get("method")
    rid = req.get("id")

    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})

    elif method == "hook":
        params = req.get("params", {})
        event = params.get("event", {})
        text = (event.get("fields") or {}).get("text") or ""
        log(f"hook {params.get('hook_id')} fired: {text[:60]!r}")
        if rid is not None:
            send({"jsonrpc": "2.0", "id": rid, "result": {"effects": [], "decision": {"type": "pass"}}})

    elif method == "tool/call":
        name = req.get("params", {}).get("name")
        if name == "now":
            now = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
            send({
                "jsonrpc": "2.0", "id": rid,
                "result": {"content": {"text": now}, "is_error": False, "effects": []},
            })
        elif rid is not None:
            send({
                "jsonrpc": "2.0", "id": rid,
                "error": {"code": -32601, "message": f"no tool `{name}`"},
            })

    elif method == "shutdown":
        log("shutting down")

    elif rid is not None:
        send({
            "jsonrpc": "2.0", "id": rid,
            "error": {"code": -32601, "message": f"unknown method {method!r}"},
        })


def main():
    for line in sys.stdin.buffer:
        line = line.strip()
        if not line:
            continue
        try:
            handle(json.loads(line))
        except Exception as exc:  # never die on one bad message
            log(f"error handling message: {exc}")


if __name__ == "__main__":
    main()
