"""Test-only forwarding shim for files the existing scripted peer cannot list.

The actual mock ACP peer and authenticated host MCP connection remain unchanged.
No registry/event writes and no interpretation of engine execution state.
"""
import json
import pathlib
import signal
import subprocess
import sys

script = pathlib.Path(sys.argv[1])
session = sys.argv[2]
turns = json.loads(script.read_text())
child = subprocess.Popen(sys.argv[3:], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         text=True, bufsize=1)


def settle(*_):
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=2)


signal.signal(signal.SIGTERM, lambda *_: (settle(), sys.exit(0)))
try:
    for line in sys.stdin:
        frame = json.loads(line)
        params = frame.get("params", {})
        if frame.get("method") == "tools/call" and params.get("name") == "report_stage_outcome":
            args = params["arguments"]
            call = args["call_id"]
            assert call.startswith("script-")
            index = int(call.removeprefix("script-"))
            assert call == f"script-{index}"
            assert int(script.with_suffix(".cursor").read_text()) == index + 1
            turn = turns[index]
            assert args["outcome"] == turn["outcome"]
            files = turn.get("fixture_files", {})
            assert set(files) <= {"spec.toml", "spec.md", "tracked-sentinel.txt", "untracked-sentinel.txt"}
            artifacts = turn.get("fixture_artifacts", [])
            assert set(artifacts) <= {"spec.toml", "spec.md"}
            assert all(isinstance(content, str) for content in files.values())
            for name, content in files.items():
                pathlib.Path(name).write_text(content)
            args["artifacts_produced"].extend(artifacts)
            with script.with_name("artifact-proxy.jsonl").open("a") as log:
                log.write(json.dumps({"session": session, "call_id": call,
                                      "node": turn.get("fixture_node"), "task": turn.get("fixture_task"),
                                      "files": list(files), "artifacts": args["artifacts_produced"]}) + "\n")
        child.stdin.write(json.dumps(frame) + "\n")
        child.stdin.flush()
        if "id" not in frame:
            continue
        while True:
            reply = child.stdout.readline()
            if not reply:
                raise RuntimeError("actual MCP helper EOF before response")
            sys.stdout.write(reply)
            sys.stdout.flush()
            if json.loads(reply).get("id") == frame["id"]:
                break
finally:
    settle()
