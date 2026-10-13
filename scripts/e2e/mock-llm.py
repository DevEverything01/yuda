#!/usr/bin/env python3
"""端到端测试用的 OpenAI 兼容 chat/completions 端点：对任何请求返回固定文本。

用法：mock-llm.py <端口> <返回文本>
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, reply = int(sys.argv[1]), sys.argv[2]


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        body = json.dumps({"choices": [{"message": {"role": "assistant", "content": reply}}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", port), Handler).serve_forever()
