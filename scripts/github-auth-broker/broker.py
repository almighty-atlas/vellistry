#!/usr/bin/env python3
"""Stateless GitHub App token broker. Put behind a TLS reverse proxy."""
import argparse
import http.client
import json
import os
import re
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlencode

MAX_BODY = 8192
MAX_UPSTREAM = 16384
IO_TIMEOUT = 10
MAX_WORKERS = 16


class Rejected(Exception):
    def __init__(self, status, error):
        self.status, self.error = status, error


def secret(value):
    return isinstance(value, str) and 1 <= len(value) <= 2048 and all(
        0x21 <= ord(c) <= 0x7e for c in value
    )


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate field")
        result[key] = value
    return result


def decode(data):
    return json.loads(data.decode("utf-8"), object_pairs_hook=unique_object,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))


def parse_callback_port(value):
    if not isinstance(value, str) or not re.fullmatch(r"[1-9][0-9]{3,4}", value):
        raise ValueError("Invalid callback port")
    port = int(value)
    if not 1024 <= port <= 65535:
        raise ValueError("Invalid callback port")
    return port


def validate(data, client_id, callback_port):
    if not isinstance(data, dict) or data.get("client_id") != client_id:
        raise Rejected(400, "invalid_request")
    grant = data.get("grant_type")
    if grant == "authorization_code":
        if set(data) != {"grant_type", "client_id", "code", "code_verifier", "redirect_uri"}:
            raise Rejected(400, "invalid_request")
        verifier, redirect = data["code_verifier"], data["redirect_uri"]
        if not isinstance(verifier, str) or not re.fullmatch(r"[A-Za-z0-9._~-]{43,128}", verifier):
            raise Rejected(400, "invalid_request")
        if redirect != f"http://127.0.0.1:{callback_port}/callback" or not secret(data["code"]):
            raise Rejected(400, "invalid_request")
        # GitHub's web flow does NOT accept authorization_code as grant_type.
        return {k: v for k, v in data.items() if k != "grant_type"}
    if grant == "refresh_token" and set(data) == {"grant_type", "client_id", "refresh_token"}:
        if secret(data["refresh_token"]):
            return dict(data)
    raise Rejected(400, "invalid_request")


def token_response(data):
    if not isinstance(data, dict):
        raise Rejected(502, "upstream_failure")
    if "error" in data:
        raise Rejected(401, "authentication_rejected")
    token_type = data.get("token_type")
    if not secret(data.get("access_token")) or not isinstance(token_type, str) or token_type.lower() != "bearer":
        raise Rejected(502, "upstream_failure")
    result = {"access_token": data["access_token"], "token_type": "bearer"}
    if "refresh_token" in data:
        if not secret(data["refresh_token"]):
            raise Rejected(502, "upstream_failure")
        result["refresh_token"] = data["refresh_token"]
    for key in ("expires_in", "refresh_token_expires_in"):
        if key in data:
            value = data[key]
            if type(value) is not int or not 0 < value <= 2**53 - 1:
                raise Rejected(502, "upstream_failure")
            result[key] = value
    if "expires_in" in result and not {"refresh_token", "refresh_token_expires_in"} <= result.keys():
        raise Rejected(502, "upstream_failure")
    return result


class GithubExchange:
    def __init__(self, client_secret):
        self.client_secret = client_secret

    def __call__(self, fields):
        body = urlencode({**fields, "client_secret": self.client_secret}).encode("ascii")
        # Fixed HTTPS origin, verified certificates, no redirects, no env proxies.
        conn = None
        try:
            conn = http.client.HTTPSConnection("github.com", timeout=IO_TIMEOUT)
            conn.request("POST", "/login/oauth/access_token", body, {
                "Accept": "application/json",
                "Content-Type": "application/x-www-form-urlencoded",
                "User-Agent": "mdwell-auth-broker",
            })
            response = conn.getresponse()
            if response.status != 200:
                raise Rejected(502, "upstream_failure")
            deadline = time.monotonic() + IO_TIMEOUT
            data = bytearray()
            while len(data) <= MAX_UPSTREAM:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError()
                # The response can detach the socket from the connection on EOF.
                if conn.sock is not None:
                    conn.sock.settimeout(remaining)
                chunk = response.read1(min(4096, MAX_UPSTREAM + 1 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
            if len(data) > MAX_UPSTREAM:
                raise Rejected(502, "upstream_failure")
            return token_response(decode(data))
        except Rejected:
            raise
        except Exception:
            raise Rejected(502, "upstream_failure") from None
        finally:
            if conn is not None:
                conn.close()


class RateLimit:
    """Bounded peer-IP token buckets; no untrusted forwarded-header parsing."""
    def __init__(self, burst=200, per_second=2.0, max_peers=4096, clock=time.monotonic):
        self.burst, self.rate, self.max_peers, self.clock = burst, per_second, max_peers, clock
        self.peers = {}
        self.lock = threading.Lock()

    def allow(self, peer):
        with self.lock:
            now = self.clock()
            if peer not in self.peers and len(self.peers) >= self.max_peers:
                self.peers = {k: v for k, v in self.peers.items() if now - v[1] < 600}
                if len(self.peers) >= self.max_peers:
                    return False
            tokens, previous = self.peers.get(peer, (self.burst, now))
            tokens = min(self.burst, tokens + (now - previous) * self.rate)
            allowed = tokens >= 1
            self.peers[peer] = (tokens - 1 if allowed else tokens, now)
            return allowed


class BrokerServer(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 32

    def __init__(self, address, client_id, callback_port, exchange, limiter=None):
        self.callback_port = parse_callback_port(str(callback_port))
        self.client_id, self.exchange = client_id, exchange
        self.limiter = limiter or RateLimit()
        self.slots = threading.BoundedSemaphore(MAX_WORKERS)
        super().__init__(address, Handler)

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            # Do not create a worker or block accept() for saturated connections.
            request.close()
            return
        try:
            super().process_request(request, address)
        except Exception:
            self.slots.release()
            self.shutdown_request(request)

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()

    def handle_error(self, request, client_address):
        pass  # Never print tracebacks containing request or token data.


class Handler(BaseHTTPRequestHandler):
    server_version = "AuthBroker"
    sys_version = ""

    def setup(self):
        self.request.settimeout(IO_TIMEOUT)
        super().setup()

    def log_message(self, *args):
        pass

    def send_error(self, code, message=None, explain=None):
        self.reply(code, {"error": "invalid_request"})

    def reply(self, status, payload):
        data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.close_connection = True
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Pragma", "no-cache")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Connection", "close")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        try:
            if not self.server.limiter.allow(self.client_address[0]):
                raise Rejected(429, "rate_limited")
            if self.path != "/token":
                raise Rejected(404, "not_found")
            if self.headers.get_all("Transfer-Encoding") or self.headers.get_all("Content-Encoding"):
                raise Rejected(400, "invalid_request")
            lengths = self.headers.get_all("Content-Length", [])
            if len(lengths) != 1 or not re.fullmatch(r"[0-9]{1,8}", lengths[0]):
                raise Rejected(400, "invalid_request")
            length = int(lengths[0])
            if length > MAX_BODY:
                raise Rejected(413, "request_too_large")
            types = self.headers.get_all("Content-Type", [])
            if len(types) != 1 or types[0].lower() not in ("application/json", "application/json; charset=utf-8"):
                raise Rejected(415, "unsupported_media_type")
            deadline, body = time.monotonic() + IO_TIMEOUT, bytearray()
            while len(body) < length:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError()
                self.connection.settimeout(remaining)
                chunk = self.rfile.read1(length - len(body))
                if not chunk:
                    raise Rejected(400, "invalid_request")
                body.extend(chunk)
            try:
                fields = validate(decode(body), self.server.client_id, self.server.callback_port)
            except (ValueError, UnicodeError, RecursionError):
                raise Rejected(400, "invalid_request") from None
            self.reply(200, self.server.exchange(fields))
        except Rejected as error:
            self.reply(error.status, {"error": error.error})
        except (TimeoutError, socket.timeout):
            self.reply(408, {"error": "request_timeout"})
        except Exception:
            self.reply(502, {"error": "upstream_failure"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8787)
    args = parser.parse_args()
    client_id, client_secret = os.environ.get("GITHUB_CLIENT_ID"), os.environ.get("GITHUB_CLIENT_SECRET")
    if not client_id or not re.fullmatch(r"[A-Za-z0-9_]{1,128}", client_id) or not secret(client_secret):
        parser.exit(2, "Set valid GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET on the server.\n")
    try:
        callback_port = parse_callback_port(os.environ.get("GITHUB_CALLBACK_PORT"))
    except ValueError:
        parser.exit(2, "Set GITHUB_CALLBACK_PORT to the registered desktop callback port (1024-65535).\n")
    if not 1 <= args.port <= 65535:
        parser.exit(2, "Invalid listen port.\n")
    with BrokerServer(("127.0.0.1", args.port), client_id, callback_port, GithubExchange(client_secret)) as server:
        print("GitHub auth broker listening on loopback; TLS proxy required.", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
