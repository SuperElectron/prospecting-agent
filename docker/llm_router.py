import json
import os
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CHAT_UPSTREAM = os.environ.get("LLM_CHAT_UPSTREAM", "http://host.docker.internal:8000").rstrip("/")
EMBED_UPSTREAM = os.environ.get("LLM_EMBED_UPSTREAM", "http://host.docker.internal:8002").rstrip("/")
HOP_HEADERS = {"host", "content-length", "connection", "transfer-encoding", "accept-encoding"}


class Router(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        self.forward(None)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        self.forward(self.rfile.read(length))

    def forward(self, body):
        upstream = EMBED_UPSTREAM if self.path.startswith("/v1/embeddings") else CHAT_UPSTREAM
        if body and self.path.startswith("/v1/embeddings"):
            try:
                payload = json.loads(body)
                payload.pop("dimensions", None)
                body = json.dumps(payload).encode()
            except ValueError:
                pass
        request = urllib.request.Request(upstream + self.path, data=body, method=self.command)
        for name, value in self.headers.items():
            if name.lower() not in HOP_HEADERS:
                request.add_header(name, value)
        try:
            with urllib.request.urlopen(request, timeout=300) as resp:
                data = resp.read()
                self.respond(resp.status, resp.headers.get("Content-Type", "application/json"), data)
        except urllib.error.HTTPError as e:
            self.respond(e.code, e.headers.get("Content-Type", "application/json"), e.read())
        except (urllib.error.URLError, TimeoutError) as e:
            detail = json.dumps({"error": {"message": f"upstream unreachable: {e}"}}).encode()
            self.respond(502, "application/json", detail)

    def respond(self, status, content_type, data):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8080), Router).serve_forever()
