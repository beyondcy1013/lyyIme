#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""mock_ai_server.py —— AI 端到端测试用 mock 服务(OpenAI /chat/completions 兼容)。

监听 127.0.0.1 随机/指定端口;对任意 POST 返回固定回复「AI回复OK」;
每个请求的 path/Authorization/body 追加写入 MOCK_AI_DUMP 文件供断言。

用法::

    MOCK_AI_DUMP=/tmp/dump.jsonl python3 mock_ai_server.py --port-file /tmp/port
"""

import argparse
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPLY = 'AI回复OK'


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get('Content-Length', 0))
        raw = self.rfile.read(length)
        dump = os.environ.get('MOCK_AI_DUMP')
        if dump:
            try:
                body = json.loads(raw.decode('utf-8'))
            except ValueError:
                body = {'raw': raw.decode('utf-8', 'replace')}
            with open(dump, 'a', encoding='utf-8') as fh:
                fh.write(json.dumps({
                    'path': self.path,
                    'auth': self.headers.get('Authorization'),
                    'body': body,
                }, ensure_ascii=False) + '\n')
        payload = json.dumps(
            {'choices': [{'message': {'content': REPLY}}]},
            ensure_ascii=False).encode('utf-8')
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt, *args):  # 静默:不污染 e2e 输出
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--port', type=int, default=0,
                        help='监听端口;0=自动分配')
    parser.add_argument('--port-file', help='把实际端口写到这里(供 e2e 读取)')
    parser.add_argument('--idle-exit', type=float, default=0,
                        help='秒;>0 时收到首个请求后延时自杀(e2e 收尾用)')
    args = parser.parse_args()

    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    port = server.server_address[1]
    if args.port_file:
        with open(args.port_file, 'w') as fh:
            fh.write(str(port))
    sys.stdout.write('MOCK-AI-READY %d\n' % port)
    sys.stdout.flush()
    if args.idle_exit > 0:
        server.timeout = args.idle_exit
        while True:
            if not server.handle_request():
                break
    else:
        server.serve_forever()


if __name__ == '__main__':
    main()
