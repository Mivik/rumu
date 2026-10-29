#!/usr/bin/env python3
"""rumu stdio demo plugin (Python 3 stdlib only): a token-saving command rewriter.

Intercepts `tool.pre` for the `bash` tool and rewrites bare `git status`
commands into the compact `git status --short --branch` form, demonstrating
the `rewrite` decision (JSON merge patch on the event payload)."""

import json
import sys


def send(msg):
    sys.stdout.buffer.write((json.dumps(msg) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()


def log(text):
    print(f"[compact] {text}", file=sys.stderr, flush=True)


MANIFEST = {
    "name": "compact",
    "instructions": "Compacts `git status` shell commands to the short form "
    "to save tokens.",
    "tools": [],
    "hooks": [
        {
            "id": "rewrite-git-status",
            "match": {
                "event": "tool.pre",
                "tool": "bash",
                "cmd": {"re": "^git status( |$)"},
            },
            "priority": -10,
        }
    ],
}


def rewrite_patch(cmd):
    return {"effects": [], "decision": {"type": "rewrite", "patch": {"args": {"command": cmd}}}}


def handle(req):
    method = req.get("method")
    rid = req.get("id")

    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})

    elif method == "hook":
        params = req.get("params", {})
        event = params.get("event", {})
        cmd = (event.get("fields") or {}).get("cmd") or ""
        result = {"effects": [], "decision": {"type": "pass"}}
        if cmd == "git status" or cmd.startswith("git status "):
            result = rewrite_patch("git status --short --branch")
            log(f"rewriting {cmd!r} -> 'git status --short --branch'")
        if rid is not None:
            send({"jsonrpc": "2.0", "id": rid, "result": result})

    elif method == "shutdown":
        pass

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
        except Exception as exc:
            log(f"error handling message: {exc}")


if __name__ == "__main__":
    main()
