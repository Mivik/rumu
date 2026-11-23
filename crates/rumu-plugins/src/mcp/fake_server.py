"""A minimal stdio MCP server for the mcp tests (standard library only)."""
import json
import os
import sys

# Each start is logged, so tests can count reconnects.
with open(os.path.join(os.path.dirname(__file__), "starts"), "a") as f:
    f.write("start\n")

TOOLS = [
    {
        "name": "echo",
        "description": "Echo the given text back.",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    {
        "name": "list_issues",
        "title": "List issues",
        "description": "List the open issues of a GitHub repository.\nPaged by 30.",
        "inputSchema": {
            "type": "object",
            "properties": {"repo": {"type": "string"}},
        },
    },
    {
        "name": "fail",
        "description": "Always fails.",
        "inputSchema": {"type": "object"},
    },
]

RESOURCES = [
    {
        "uri": "mem://notes",
        "name": "notes",
        "description": "Meeting notes.\nWeekly.",
        "mimeType": "text/markdown",
    },
    {"uri": "mem://both", "name": "both"},
]

CONTENTS = {
    "mem://notes": [{"uri": "mem://notes", "mimeType": "text/markdown", "text": "# Notes"}],
    "mem://both": [
        {"uri": "mem://both/a", "text": "A"},
        # "hi\n" as a blob of a type rumu does not know.
        {"uri": "mem://both/b", "mimeType": "application/x-thing", "blob": "aGkK"},
    ],
}


def reply(id, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    msg = json.loads(line)
    method, id = msg.get("method"), msg.get("id")
    if id is None:
        continue
    if method == "initialize":
        reply(id, {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {"listChanged": True}, "resources": {}},
            "serverInfo": {"name": "fake", "version": "1"},
            "instructions": "Echo things.",
        })
    elif method == "tools/list":
        reply(id, {"tools": TOOLS})
    elif method == "tools/call":
        name = msg["params"]["name"]
        args = msg["params"].get("arguments") or {}
        if name == "echo":
            reply(id, {"content": [{"type": "text", "text": args.get("text", "")}]})
        elif name == "crash":
            # Unlisted: logs that it ran, then dies without answering.
            with open(args["log"], "a") as f:
                f.write("ran\n")
            os._exit(1)
        elif name == "list_issues":
            reply(id, {"content": [], "structuredContent": {"issues": [1, 2]}})
        else:
            reply(id, {"content": [{"type": "text", "text": "boom"}], "isError": True})
    elif method == "resources/list":
        reply(id, {"resources": RESOURCES})
    elif method == "resources/templates/list":
        reply(id, {"resourceTemplates": [{
            "uriTemplate": "mem://issue/{n}",
            "name": "issue",
            "description": "One issue.",
        }]})
    elif method == "resources/read":
        uri = msg["params"]["uri"]
        if uri in CONTENTS:
            reply(id, {"contents": CONTENTS[uri]})
        else:
            reply(id, error={"code": -32002, "message": "no resource " + uri})
    elif method == "ping":
        reply(id, {})
    else:
        reply(id, error={"code": -32601, "message": "unknown method " + str(method)})
