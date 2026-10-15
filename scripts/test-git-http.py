#!/usr/bin/env python3
"""Offline real-Git smart HTTPS regression; needs Python, OpenSSL, Git and Cargo.

Run from anywhere: python3 scripts/test-git-http.py [path/to/oauth-executable]
Only a fixed fake credential is used. No request headers, bodies, credential
frames, subprocess output, or secrets are logged. TLS verification stays enabled.
"""

import argparse
import base64
import collections
import hmac
import http.server
import json
import os
from pathlib import Path
import shutil
import socketserver
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.parse


FAKE_TOKEN = "fixture-oauth-secret-never-persist"
PROJECT = Path(__file__).resolve().parent.parent
DEADLINE = time.monotonic() + 110


def run(args, *, env, cwd=PROJECT, input=None):
    remaining = DEADLINE - time.monotonic()
    if remaining <= 0:
        raise RuntimeError("fixture deadline exceeded")
    result = subprocess.run(
        [str(arg) for arg in args], cwd=cwd, env=env, input=input,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=min(60, remaining),
    )
    if result.returncode:
        # Even a crashing helper must not cause raw diagnostic output to escape.
        category = Path(args[0]).name
        if category not in {"cargo", "openssl", "git"}:
            category = "OAuth harness"
        codes = [code for code in (
            "GitCredentialHelperStart", "GitCredentialIpc", "GitAuthentication",
            "GitPermissionDenied", "GitNetwork", "unsafe repository-local Git configuration",
        ) if code.encode("ascii") in result.stderr]
        detail = ", ".join(codes) or "no recognized core error code"
        raise RuntimeError(f"{category} subprocess failed (exit {result.returncode}; {detail}; output withheld)")
    return result.stdout


def isolated_env():
    env = dict(os.environ)
    for key in list(env):
        if (key.startswith("GIT_") or key.startswith("MDWELL_GIT_CREDENTIAL_")
                or key.lower() in {"http_proxy", "https_proxy", "all_proxy", "no_proxy"}):
            del env[key]
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_TERMINAL_PROMPT="0", LC_ALL="C")
    return env


class Proxy(socketserver.ThreadingTCPServer):
    daemon_threads = True
    block_on_close = False

    def __init__(self, root, git, env, context):
        self.root, self.git, self.env, self.context = root, git, env, context
        self.mode = "success"
        self.counts = collections.Counter()
        self.lock = threading.Lock()
        super().__init__(("127.0.0.1", 0), Tunnel)

    def count(self, event):
        with self.lock:
            self.counts[(self.mode, event)] += 1

    def handle_error(self, request, client_address):
        # socketserver's default traceback could include request data.
        self.count("server_error")


class Tunnel(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(10)
        # Read without buffering beyond the CONNECT header into TLS data.
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            byte = self.request.recv(1)
            if not byte or len(header) >= 16 * 1024:
                self.server.count("invalid_connect")
                return
            header.extend(byte)
        if header.split(b"\r\n", 1)[0] not in (
            b"CONNECT github.com:443 HTTP/1.1", b"CONNECT github.com:443 HTTP/1.0",
        ):
            self.server.count("invalid_connect")
            self.request.sendall(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            return
        # This is a TLS terminator, never a forwarding proxy: no outbound sockets.
        self.request.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        with self.server.context.wrap_socket(self.request, server_side=True) as tls:
            self.server.count("tls")
            SmartHTTP(tls, self.client_address, self.server)


class SmartHTTP(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def respond(self, status, body=b"", headers=()):
        self.send_response(status)
        for key, value in headers:
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def do_GET(self):
        self.serve_git()

    def do_POST(self):
        self.serve_git()

    def serve_git(self):
        target = urllib.parse.urlsplit(self.path)
        allowed = (
            self.command == "GET"
            and target.path == "/owner/repo.git/info/refs"
            and target.query == "service=git-upload-pack"
        ) or (
            self.command == "POST"
            and target.path == "/owner/repo.git/git-upload-pack"
            and not target.query
        )
        if self.headers.get("Host") != "github.com" or not allowed or target.netloc:
            self.server.count("invalid_http")
            self.respond(403)
            return
        auth = self.headers.get("Authorization")
        expected = "Basic " + base64.b64encode(
            ("x-access-token:" + FAKE_TOKEN).encode("ascii")
        ).decode("ascii")
        if auth is None:
            self.server.count("challenge")
            self.respond(401, headers=(("WWW-Authenticate", 'Basic realm="GitHub"'),))
            return
        if not hmac.compare_digest(auth.encode(), expected.encode()):
            self.server.count("wrong_credential")
            self.respond(401, headers=(("WWW-Authenticate", 'Basic realm="GitHub"'),))
            return
        self.server.count("authenticated")
        if self.server.mode == "authentication":
            self.respond(401, headers=(("WWW-Authenticate", 'Basic realm="GitHub"'),))
            return
        if self.server.mode == "permission":
            self.respond(403)
            return
        if self.headers.get("Transfer-Encoding"):
            self.server.count("unsupported_transfer")
            self.respond(400)
            return
        length = int(self.headers.get("Content-Length", "0"))
        if not 0 <= length <= 2 * 1024 * 1024:
            self.respond(413)
            return
        body = self.rfile.read(length)
        if len(body) != length:
            self.respond(400)
            return
        env = dict(self.server.env)
        env.update(
            GIT_PROJECT_ROOT=str(self.server.root / "repositories"),
            GIT_HTTP_EXPORT_ALL="1", PATH_INFO=target.path,
            QUERY_STRING=target.query, REQUEST_METHOD=self.command,
            CONTENT_TYPE=self.headers.get("Content-Type", ""),
            CONTENT_LENGTH=str(length), REMOTE_USER="x-access-token",
            REMOTE_ADDR="127.0.0.1", SERVER_PROTOCOL="HTTP/1.1",
            HTTP_GIT_PROTOCOL=self.headers.get("Git-Protocol", ""),
        )
        response = run([self.server.git, "http-backend"], env=env,
                       cwd=self.server.root, input=body)
        headers, separator, payload = response.partition(b"\r\n\r\n")
        if not separator:
            self.server.count("invalid_cgi")
            self.respond(500)
            return
        status, outgoing = 200, []
        for line in headers.decode("ascii").split("\r\n"):
            key, value = line.split(":", 1)
            if key.lower() == "status":
                status = int(value.strip().split()[0])
            elif key.lower() not in {"content-length", "connection", "transfer-encoding"}:
                outgoing.append((key, value.strip()))
        self.server.count(self.command)
        self.respond(status, payload, outgoing)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", nargs="?", type=Path)
    args = parser.parse_args()
    git, openssl = shutil.which("git"), shutil.which("openssl")
    if not git or not openssl:
        raise RuntimeError("Git and OpenSSL are required")
    env = isolated_env()
    with tempfile.TemporaryDirectory(prefix="vellistry-git-http-") as directory:
        root = Path(directory)
        config = root / "global.config"
        config.write_text("")
        env["GIT_CONFIG_GLOBAL"] = str(config)
        if args.executable:
            executable = args.executable.resolve(strict=True)
        else:
            artifacts = run([
                "cargo", "build", "--offline", "-p", "mdwell-core", "--test", "oauth",
                "--message-format=json",
            ], env=env)
            candidates = [
                item["executable"] for line in artifacts.splitlines()
                if (item := json.loads(line)).get("reason") == "compiler-artifact"
                and item.get("target", {}).get("name") == "oauth"
                and item.get("executable")
            ]
            if len(candidates) != 1:
                raise RuntimeError("could not discover the OAuth test executable")
            executable = Path(candidates[0]).resolve(strict=True)
        cert, key = root / "certificate.pem", root / "key.pem"
        run([
            openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=github.com",
            "-addext", "subjectAltName=DNS:github.com",
        ], env=env)
        source = root / "source"
        source.mkdir()
        bare = root / "repositories/owner/repo.git"
        bare.parent.mkdir(parents=True)
        run([git, "init", "--initial-branch=main", source], env=env)
        (source / "README.md").write_text("# HTTPS fixture\n\nInitial quokka.\n")
        run([git, "add", "README.md"], env=env, cwd=source)
        run([git, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
             "commit", "-m", "fixture"], env=env, cwd=source)
        run([git, "clone", "--bare", "--", source, bare], env=env)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        with Proxy(root, git, env, context) as proxy:
            worker = threading.Thread(target=proxy.serve_forever, daemon=True)
            worker.start()
            try:
                run([git, "config", "--file", config, "http.proxy",
                     f"http://127.0.0.1:{proxy.server_address[1]}"], env=env)
                run([git, "config", "--file", config, "http.sslCAInfo", cert], env=env)
                run([git, "config", "--file", config, "http.sslVerify", "true"], env=env)
                env["MDWELL_FIXTURE_REAL_GIT"] = git
                for mode in ("success", "permission", "authentication"):
                    proxy.mode = mode
                    try:
                        output = run([executable, "--http-fixture", root, mode], env=env)
                    except RuntimeError:
                        # Event names/counts are fixed metadata, not wire contents.
                        print(f"Fixture mode {mode}: {dict(proxy.counts)}")
                        raise
                    if FAKE_TOKEN.encode() in output:
                        raise RuntimeError("fixture credential escaped to output")
                    if proxy.counts[(mode, "challenge")] < 1 or proxy.counts[(mode, "authenticated")] < 1:
                        raise RuntimeError("fixture did not exercise challenged authentication")
                if (proxy.counts[("success", "GET")] < 2
                        or proxy.counts[("success", "POST")] < 2
                        or proxy.counts[("success", "challenge")] < 2):
                    raise RuntimeError("clone and fetch did not both exercise challenged smart HTTPS")
                failures = {"server_error", "invalid_connect", "invalid_http", "wrong_credential",
                            "unsupported_transfer", "invalid_cgi"}
                if any(value for (_, event), value in proxy.counts.items() if event in failures):
                    raise RuntimeError("proxy/backend invariant failed (details withheld)")
            finally:
                proxy.shutdown()
                worker.join(timeout=5)
    print("PASS: real Git smart HTTPS clone/reopen/fetch/read/search; 401 and 403 classified")
    print("TLS verification enabled; exact github.com:443 CONNECT target; no outbound forwarding")


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        # RuntimeError messages above are fixed diagnostics, never subprocess data.
        raise SystemExit(f"FAIL: {error}") from None
    except (OSError, ValueError, subprocess.TimeoutExpired):
        # No traceback or exception text: neither is guaranteed credential-free.
        raise SystemExit("FAIL: smart HTTPS fixture failed; raw diagnostics withheld") from None
