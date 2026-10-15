"""Offline only: loopback HTTP servers and a patched GitHub connection."""
import contextlib
import http.client
import io
import json
import socket
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs

import broker

CLIENT = "test_client"
SECRET = "server-secret-do-not-return"
CALLBACK_PORT = 54321
CODE = {"grant_type": "authorization_code", "client_id": CLIENT, "code": "one-time-code",
        "code_verifier": "a" * 43, "redirect_uri": "http://127.0.0.1:54321/callback"}
TOKEN = {"access_token": "mock-access", "token_type": "bearer", "refresh_token": "mock-refresh",
         "expires_in": 28800, "refresh_token_expires_in": 15897600}


@contextlib.contextmanager
def running(server):
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
    thread.start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


class MockGithub(broker.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.server.calls.append((self.path, dict(self.headers), parse_qs(body.decode())))
        status, payload, headers = self.server.response
        data = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        self.send_response(status)
        for name, value in headers.items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class BrokerTests(unittest.TestCase):
    def setUp(self):
        self.upstream = broker.ThreadingHTTPServer(("127.0.0.1", 0), MockGithub)
        self.upstream.calls = []
        self.upstream.response = (200, TOKEN, {})
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        upstream_port = self.stack.enter_context(running(self.upstream))
        # The production URL remains fixed HTTPS; only tests replace the transport.
        self.stack.enter_context(patch.object(broker.http.client, "HTTPSConnection",
            side_effect=lambda host, timeout: http.client.HTTPConnection("127.0.0.1", upstream_port, timeout=timeout)))
        self.server = broker.BrokerServer(("127.0.0.1", 0), CLIENT, CALLBACK_PORT, broker.GithubExchange(SECRET))
        self.port = self.stack.enter_context(running(self.server))

    def request(self, data=CODE, path="/token", method="POST", headers=None, raw=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=2)
        try:
            conn.request(method, path, json.dumps(data).encode() if raw is None else raw,
                         headers or {"Content-Type": "application/json"})
            response = conn.getresponse()
            body = response.read()
            self.assertEqual(response.getheader("Cache-Control"), "no-store")
            self.assertIsNone(response.getheader("Access-Control-Allow-Origin"))
            return response.status, body
        finally:
            conn.close()

    def test_code_exchange_form_and_filtered_response(self):
        self.upstream.response = (200, {**TOKEN, "scope": "", "debug": SECRET}, {})
        status, body = self.request()
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body), TOKEN)
        path, headers, fields = self.upstream.calls[0]
        self.assertEqual(path, "/login/oauth/access_token")
        self.assertEqual(headers["Accept"], "application/json")
        self.assertEqual(headers["Content-Type"], "application/x-www-form-urlencoded")
        self.assertNotIn("grant_type", fields)
        self.assertEqual(fields["client_secret"], [SECRET])
        self.assertEqual(fields["redirect_uri"], [CODE["redirect_uri"]])
        self.assertEqual(fields["code_verifier"], [CODE["code_verifier"]])

    def test_refresh_exchange(self):
        self.assertEqual(self.request({"grant_type": "refresh_token", "client_id": CLIENT,
                                       "refresh_token": "old-refresh"})[0], 200)
        fields = self.upstream.calls[0][2]
        self.assertEqual(set(fields), {"grant_type", "client_id", "refresh_token", "client_secret"})
        self.assertEqual(fields["grant_type"], ["refresh_token"])

    def test_input_validation(self):
        bad = [{**CODE, "client_id": "wrong"}, {**CODE, "scope": "repo"},
               {**CODE, "client_secret": SECRET}, [], {**CODE, "code": "a" * 2049}]
        for code in ("", "x\n", "x\x7f", 42, "é"):
            bad.append({**CODE, "code": code})
        for verifier in ("a" * 42, "a" * 129, "a" * 42 + "+", None):
            bad.append({**CODE, "code_verifier": verifier})
        for uri in ("http://127.0.0.1/callback", "http://127.0.0.1:0/callback",
                    "http://127.0.0.1:65536/callback", "http://localhost:123/callback",
                    "http://user@127.0.0.1:123/callback", "https://127.0.0.1:123/callback",
                    "http://127.0.0.1:123/callback?", "http://127.0.0.1:123/callback#",
                    "http://127.0.0.1:123/callback/", "http://127.0.0.1:0123/callback"):
            bad.append({**CODE, "redirect_uri": uri})
        # Test validation directly so the production rate limit cannot mask results.
        for value in bad:
            with self.subTest(value=value), self.assertRaises(broker.Rejected):
                broker.validate(value, CLIENT, CALLBACK_PORT)
        self.assertEqual(self.request({**CODE, "client_id": "wrong"})[0], 400)
        self.assertFalse(self.upstream.calls)
        for n in (43, 128):
            broker.validate({**CODE, "code_verifier": "~" * n}, CLIENT, CALLBACK_PORT)
        for port in (1024, 65535):
            broker.validate({**CODE, "redirect_uri": f"http://127.0.0.1:{port}/callback"}, CLIENT, port)

    def test_callback_port_exact_policy(self):
        for port in (0, 1, 1023, 1024, CALLBACK_PORT - 1, CALLBACK_PORT + 1, 65535, 65536):
            with self.subTest(port=port):
                status, body = self.request({**CODE, "redirect_uri": f"http://127.0.0.1:{port}/callback"})
                self.assertEqual(status, 400)
                self.assertEqual(json.loads(body), {"error": "invalid_request"})
        self.assertFalse(self.upstream.calls)
        self.assertEqual(self.request()[0], 200)
        self.assertEqual(len(self.upstream.calls), 1)

    def test_callback_port_config(self):
        for value in (None, "", "0", "1", "1023", "65536", "99999", "54321\n",
                      " 54321", "54321 ", "+54321", "054321", "54321.0", "port", 54321):
            with self.subTest(value=value), self.assertRaises(ValueError):
                broker.parse_callback_port(value)
        for port in (1024, CALLBACK_PORT, 65535):
            self.assertEqual(broker.parse_callback_port(str(port)), port)
        # Invalid/missing configuration must exit before binding any listener.
        for value in (None, "1023", "65536", "not-a-port"):
            env = {"GITHUB_CLIENT_ID": CLIENT, "GITHUB_CLIENT_SECRET": SECRET}
            if value is not None:
                env["GITHUB_CALLBACK_PORT"] = value
            err = io.StringIO()
            with patch.dict(broker.os.environ, env, clear=True), patch("sys.argv", ["broker.py"]), \
                    patch.object(broker, "BrokerServer") as server, contextlib.redirect_stderr(err):
                with self.assertRaises(SystemExit) as exit_result:
                    broker.main()
                self.assertEqual(exit_result.exception.code, 2)
                server.assert_not_called()
            self.assertIn("GITHUB_CALLBACK_PORT", err.getvalue())
            self.assertNotIn(SECRET, err.getvalue())
        with patch.dict(broker.os.environ, {"GITHUB_CLIENT_ID": CLIENT, "GITHUB_CLIENT_SECRET": SECRET,
                                           "GITHUB_CALLBACK_PORT": str(CALLBACK_PORT)}, clear=True), \
                patch("sys.argv", ["broker.py"]), patch.object(broker, "BrokerServer") as server, \
                contextlib.redirect_stdout(io.StringIO()):
            broker.main()
            self.assertEqual(server.call_args.args[:3], (("127.0.0.1", 8787), CLIENT, CALLBACK_PORT))
            server.return_value.__enter__.return_value.serve_forever.assert_called_once()

    def test_refresh_whitelist(self):
        base = {"grant_type": "refresh_token", "client_id": CLIENT, "refresh_token": "refresh"}
        for data in ({**base, "code": "code"}, {**base, "redirect_uri": CODE["redirect_uri"]},
                     {**base, "refresh_token": ""}, {**base, "refresh_token": "bad\n"}):
            with self.assertRaises(broker.Rejected):
                broker.validate(data, CLIENT, CALLBACK_PORT)

    def test_malformed_and_bounded_requests(self):
        for raw in (b'{"client_id":"a","client_id":"b"}', b'NaN', b'\xff', b'{'):
            self.assertEqual(self.request(raw=raw)[0], 400)
        self.assertEqual(self.request(raw=b"x" * (broker.MAX_BODY + 1))[0], 413)
        self.assertEqual(self.request(headers={"Content-Type": "text/plain"})[0], 415)
        self.assertEqual(self.request(headers={"Content-Type": "application/json", "Transfer-Encoding": "chunked"})[0], 400)
        self.assertEqual(self.request(path="/token?secret=value")[0], 404)
        for method in ("GET", "OPTIONS"):
            self.assertEqual(self.request(method=method)[0], 501)
        self.assertFalse(self.upstream.calls)

    def test_duplicate_length_and_missing_length(self):
        for length in ("", "Content-Length: 1\r\nContent-Length: 1\r\n"):
            with socket.create_connection(("127.0.0.1", self.port), timeout=2) as conn:
                conn.sendall(("POST /token HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n" + length + "\r\nx").encode())
                self.assertIn(b"400", conn.recv(4096))
        self.assertFalse(self.upstream.calls)

    def test_upstream_failures_redacted_and_redirect_not_followed(self):
        for response in ((200, {"error": SECRET, "error_description": SECRET}, {}),
                         (500, SECRET.encode(), {}),
                         (302, SECRET.encode(), {"Location": f"http://127.0.0.1:{self.port}/token"}),
                         (200, b"not json " + SECRET.encode(), {}),
                         (200, b"x" * (broker.MAX_UPSTREAM + 1), {}),
                         (200, {"access_token": "x", "token_type": "bearer", "expires_in": True}, {})):
            with self.subTest(status=response[0]):
                self.upstream.response = response
                before = len(self.upstream.calls)
                out, err = io.StringIO(), io.StringIO()
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    status, body = self.request()
                self.assertIn(status, (401, 502))
                self.assertNotIn(SECRET.encode(), body)
                self.assertEqual(len(self.upstream.calls), before + 1)
                self.assertEqual(out.getvalue() + err.getvalue(), "")

    def test_body_and_upstream_timeouts(self):
        with patch.object(broker, "IO_TIMEOUT", 0.05):
            with socket.create_connection(("127.0.0.1", self.port), timeout=2) as conn:
                conn.sendall(b"POST /token HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\nx")
                self.assertIn(b"408", conn.recv(4096))
        with patch.object(broker.http.client, "HTTPSConnection", side_effect=TimeoutError(SECRET)):
            status, body = self.request()
            self.assertEqual(status, 502)
            self.assertNotIn(SECRET.encode(), body)

    def test_peer_limit_ignores_forwarded_headers(self):
        self.server.limiter = broker.RateLimit(burst=1, per_second=0)
        self.assertEqual(self.request(headers={"Content-Type": "application/json", "X-Forwarded-For": "1.2.3.4"})[0], 200)
        self.assertEqual(self.request(headers={"Content-Type": "application/json", "X-Forwarded-For": "5.6.7.8"})[0], 429)
        self.assertEqual(len(self.upstream.calls), 1)

    def test_concurrency_cap(self):
        for _ in range(broker.MAX_WORKERS):
            self.server.slots.acquire()
        try:
            with socket.create_connection(("127.0.0.1", self.port), timeout=2) as conn:
                self.assertEqual(conn.recv(1), b"")
        finally:
            for _ in range(broker.MAX_WORKERS):
                self.server.slots.release()
        self.assertEqual(self.request()[0], 200)

    def test_rate_table_bounded_and_refill(self):
        now = [0]
        limit = broker.RateLimit(burst=1, per_second=1, max_peers=1, clock=lambda: now[0])
        self.assertTrue(limit.allow("a"))
        self.assertFalse(limit.allow("a"))
        self.assertFalse(limit.allow("b"))
        now[0] = 1
        self.assertTrue(limit.allow("a"))
        now[0] = 602
        self.assertTrue(limit.allow("b"))
        self.assertEqual(len(limit.peers), 1)

    def test_aggregate_budget_exceeds_single_edge_client(self):
        now = [0.0]
        limit = broker.RateLimit(clock=lambda: now[0])
        # The documented edge allows one client 20 burst + 12/minute.
        for _ in range(20):
            self.assertTrue(limit.allow("127.0.0.1"))
        for second in range(5, 65, 5):
            now[0] = float(second)
            self.assertTrue(limit.allow("127.0.0.1"))
        # A different user still has capacity in the shared proxy-peer bucket.
        self.assertTrue(limit.allow("127.0.0.1"))

    def test_production_host_and_timeout(self):
        with patch.object(broker.http.client, "HTTPSConnection", side_effect=TimeoutError()) as connection:
            with self.assertRaises(broker.Rejected):
                broker.GithubExchange(SECRET)(broker.validate(CODE, CLIENT, CALLBACK_PORT))
            connection.assert_called_once_with("github.com", timeout=broker.IO_TIMEOUT)


if __name__ == "__main__":
    unittest.main()
