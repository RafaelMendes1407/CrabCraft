# A tiny fake debug adapter for the session unit tests.
# Usage: fake_adapter.py <log file> <early|late|late-noset> [--interpreter=vscode]
# (late-noset: no setVariable, only setExpression.)
# Logs every request it receives (one JSON line each) and plays a scripted program:
# a loop that hits line 10 of the breakpoint file three times (x = 1, 2, 3), then exits with 3.
import base64
import json
import sys

LOG = open(sys.argv[1], "a")
MODE = sys.argv[2]
seq = 0
x = 1
bp_ids = []
stops = 0


def read():
    n = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.decode().strip()
        if not line:
            break
        if line.lower().startswith("content-length:"):
            n = int(line.split(":")[1])
    return json.loads(sys.stdin.buffer.read(n))


def send(obj):
    global seq
    seq += 1
    obj["seq"] = seq
    b = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
    sys.stdout.buffer.flush()


def respond(m, body=None, ok=True, message=None):
    r = {"type": "response", "request_seq": m["seq"], "command": m["command"], "success": ok}
    if body is not None:
        r["body"] = body
    if message:
        r["message"] = message
    send(r)


def event(name, body=None):
    send({"type": "event", "event": name, "body": body or {}})


def stop(reason, line=10):
    global stops, cur_line
    stops += 1
    cur_line = line
    event("stopped", {"reason": reason, "threadId": 1, "allThreadsStopped": True})


def run_on():
    global x
    x += 1
    if x <= 3:
        stop("breakpoint")
    else:
        event("output", {"category": "stdout", "output": "bye\n"})
        event("exited", {"exitCode": 3})
        event("terminated")


cur_line = 10
while True:
    m = read()
    if m.get("type") != "request":
        continue
    cmd = m["command"]
    a = m.get("arguments") or {}
    LOG.write(json.dumps({"command": cmd, "arguments": a}) + "\n")
    LOG.flush()
    if cmd == "initialize":
        respond(m, {
            "supportsConfigurationDoneRequest": True,
            "supportsConditionalBreakpoints": True,
            "supportsExceptionInfoRequest": True,
            "supportsSetVariable": "noset" not in MODE,
            "supportsSetExpression": True,
            "supportsReadMemoryRequest": True,
            "exceptionBreakpointFilters": [
                {"filter": "all", "label": "All"},
                {"filter": "user-unhandled", "label": "User-Unhandled", "default": True},
            ],
        })
    elif cmd == "launch":
        if MODE == "early":
            event("initialized")
            respond(m)
        else:
            respond(m)
            event("initialized")
    elif cmd == "setBreakpoints":
        bps = []
        for b in a.get("breakpoints", []):
            bps.append({"id": 100 + b["line"], "verified": False, "line": b["line"], "message": "pending"})
        bp_ids = [i for i in bp_ids if i not in [b["id"] for b in bps]] + [b["id"] for b in bps]
        respond(m, {"breakpoints": bps})
    elif cmd == "setExceptionBreakpoints":
        respond(m)
    elif cmd == "configurationDone":
        respond(m)
        event("process", {"name": "app", "systemProcessId": 4242})
        event("output", {"category": "stdout", "output": "hello\n"})
        event("output", {"category": "telemetry", "output": "ignored"})
        for i in bp_ids:
            # Bound one line lower, like a debugger moving a breakpoint to code.
            event("breakpoint", {"reason": "changed", "breakpoint": {"id": i, "verified": True, "line": i - 100 + 1 if i - 100 == 9 else i - 100}})
        stop("breakpoint")
    elif cmd == "threads":
        respond(m, {"threads": [{"id": 1, "name": "Main"}]})
    elif cmd == "stackTrace":
        frames = [
            {"id": stops * 10, "name": "Calc", "line": cur_line, "column": 1, "source": {"path": "/w/Calc.cs"}},
            {"id": stops * 10 + 1, "name": "Main", "line": 5, "column": 1, "source": {"path": "/w/Program.cs"}},
        ]
        start = a.get("startFrame", 0)
        levels = a.get("levels", 0) or len(frames)
        respond(m, {"stackFrames": frames[start:start + levels], "totalFrames": len(frames)})
    elif cmd == "scopes":
        respond(m, {"scopes": [{"name": "Locals", "variablesReference": 1000 + stops, "expensive": False}]})
    elif cmd == "variables":
        ref = a["variablesReference"]
        if ref == 1000 + stops:
            respond(m, {"variables": [
                {"name": "x", "value": str(x), "type": "int", "variablesReference": 0},
                {"name": "order", "value": "{Order}", "type": "Order", "variablesReference": 2000 + stops},
            ]})
        elif ref == 2000 + stops:
            respond(m, {"variables": [{"name": "Customer", "value": '"Ada"', "variablesReference": 0}]})
        else:
            respond(m, ok=False, message="invalid reference")
    elif cmd == "evaluate":
        e = a["expression"]
        if e == "x":
            respond(m, {"result": str(x), "type": "int", "variablesReference": 0})
        elif e == "x * 10":
            respond(m, {"result": str(x * 10), "type": "int", "variablesReference": 0})
        elif e == "order":
            respond(m, {"result": "{Order}", "variablesReference": 2000 + stops})
        else:
            respond(m, ok=False, message="error CS0103: The name '%s' does not exist" % e)
    elif cmd in ("setVariable", "setExpression"):
        target = a.get("name") if cmd == "setVariable" else a.get("expression")
        try:
            value = int(a["value"], 0)
        except ValueError:
            respond(m, ok=False, message="cannot assign '%s'" % a["value"])
            continue
        if target != "x":
            respond(m, ok=False, message="no variable named '%s'" % target)
            continue
        x = value
        respond(m, {"value": str(x), "type": "int"})
    elif cmd == "continue":
        respond(m, {"allThreadsContinued": True})
        event("continued", {"threadId": 1, "allThreadsContinued": True})
        run_on()
    elif cmd == "next":
        respond(m)
        event("continued", {"threadId": 1})
        stop("step", cur_line + 1)
    elif cmd in ("stepIn", "stepOut"):
        respond(m)
        stop("step", 20 if cmd == "stepIn" else 12)
    elif cmd == "pause":
        respond(m)
        stop("pause")
    elif cmd == "readMemory":
        # 8 readable bytes at 0x1000 (the last one is x); everything after is unreadable.
        start = int(a["memoryReference"], 0) + a.get("offset", 0)
        memory = bytes([0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, x])
        lo = max(start, 0x1000)
        hi = min(start + a["count"], 0x1000 + len(memory))
        if hi <= lo:
            respond(m, {"address": hex(start), "unreadableBytes": a["count"]})
        else:
            data = memory[lo - 0x1000:hi - 0x1000]
            respond(m, {"address": hex(lo), "data": base64.b64encode(data).decode(), "unreadableBytes": start + a["count"] - hi})
    elif cmd == "exceptionInfo":
        respond(m, {"exceptionId": "System.Exception", "breakMode": "always"})
    elif cmd == "disconnect":
        respond(m)
        sys.exit(0)
    else:
        respond(m, ok=False, message="unknown " + cmd)
