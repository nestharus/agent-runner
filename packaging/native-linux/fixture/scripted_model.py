"""A scripted loopback stand-in for an OpenAI-compatible chat model (not a
model). The turn's user text decides the reply:

* `RUN <command>`: one `bash` tool call with that command; after its
  result, the text `ANSWER:` followed by the tool result.
* anything else: the text `NO-SCRIPT`.

Every request is kept (path and body) for the fixture to inspect.
"""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def _text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content if isinstance(part, dict))
    return ""


def reply(body):
    """The SSE chunks answering one chat request."""
    messages = body.get("messages") or []
    users = [i for i, m in enumerate(messages) if m.get("role") == "user"]
    at = users[-1] if users else None
    text = _text(messages[at]) if at is not None else ""
    results = [m for m in messages[at:] if m.get("role") == "tool"] if at is not None else []

    def chunk(delta, finish):
        return {"id": "scripted", "object": "chat.completion.chunk", "created": 0, "model": "scripted",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

    if text.startswith("RUN ") and not results:
        call = {"index": 0, "id": "call_scripted_1", "type": "function",
                "function": {"name": "bash", "arguments": json.dumps({"command": text[4:].strip()})}}
        return [chunk({"role": "assistant", "tool_calls": [call]}, None), chunk({}, "tool_calls")]
    answer = "ANSWER:\n" + _text(results[-1]) if results else "NO-SCRIPT"
    return [chunk({"role": "assistant", "content": answer}, None), chunk({}, "stop")]


class ScriptedModel:
    def __init__(self):
        self.requests = []
        model = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                length = int(self.headers.get("content-length") or 0)
                raw = self.rfile.read(length)
                try:
                    body = json.loads(raw)
                except ValueError:
                    body = None
                model.requests.append({"path": self.path, "body": body, "authorization": self.headers.get("authorization")})
                if not self.path.endswith("/chat/completions") or not isinstance(body, dict):
                    self.send_response(404)
                    self.send_header("content-length", "0")
                    self.end_headers()
                    return
                out = "".join(f"data: {json.dumps(c)}\n\n" for c in reply(body)) + "data: [DONE]\n\n"
                data = out.encode()
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.send_header("content-length", str(len(data)))
                self.send_header("connection", "close")
                self.end_headers()
                self.wfile.write(data)

            do_GET = do_POST

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base_url = f"http://127.0.0.1:{self.server.server_address[1]}/v1"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
