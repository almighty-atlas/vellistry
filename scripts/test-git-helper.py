#!/usr/bin/env python3
"""Offline desktop credential-helper regression test (Python stdlib, Unix).

Usage: python3 scripts/test-git-helper.py target/debug/mdwell-desktop \
           target/release/mdwell-desktop
Arguments are literal executable paths, not shell commands. No build is performed.
Only a loopback mock receives requests; no real tokens, Git config, or keychain
are used. Captured credential output is checked in memory and never printed.
Exit 1 means a regression; exit 2 means invalid invocation/setup.
"""

import argparse
import hashlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading


TOKEN = b"mdwell-fixture-token-NOT-A-REAL-CREDENTIAL"
CAPABILITY = b"ab" * 32
RESPONSE = b"username=x-access-token\npassword=" + TOKEN + b"\n\n"
IPC_MARKER = b"mdwell: credential helper protocol or IPC failed"
START_MARKER = b"mdwell: credential helper execution failed"
LIMIT = 16 * 1024


class MockIPC:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(4)
        self.listener.settimeout(0.05)
        self.endpoint = "127.0.0.1:%d" % self.listener.getsockname()[1]
        self.stop = threading.Event()
        self.frames = []
        self.errors = []
        self.thread = threading.Thread(target=self.serve, daemon=True)

    def serve(self):
        while not self.stop.is_set():
            try:
                connection, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                self.errors.append("accept failed")
                return
            with connection:
                connection.settimeout(2)
                frame = bytearray()
                try:
                    while not frame.endswith(b"\n\n"):
                        part = connection.recv(1024)
                        if not part:
                            break
                        frame.extend(part)
                        if len(frame) > LIMIT:
                            break
                    self.frames.append(bytes(frame))
                    connection.sendall(RESPONSE)
                except OSError:
                    self.errors.append("IPC read/write failed")

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stop.set()
        self.thread.join(3)
        if self.thread.is_alive():
            self.errors.append("mock did not stop")
        self.listener.close()


def isolated_env(root):
    # Deliberately do not inherit GIT_*, credential, askpass, desktop/session,
    # keychain, preload, or broker environment variables from the caller.
    return {
        "PATH": os.defpath,
        "HOME": str(root),
        "XDG_CONFIG_HOME": str(root),
        "LC_ALL": "C",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_SYSTEM": os.devnull,
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_ASKPASS": "/bin/false",
        "SSH_ASKPASS": "/bin/false",
        "GIT_ALLOW_PROTOCOL": "",  # credential plumbing needs no transport
        "MDWELL_GIT_CREDENTIAL_CAPABILITY": CAPABILITY.decode(),
    }


def helper_shell(executable):
    # Keep identical to CredentialBroker::attach_executable in core/src/git.rs.
    quoted = str(executable).replace("'", "'\\''")
    return ("!f() { '" + quoted + "' --mdwell-git-credential \"$@\" || "
            "{ printf '%s\\n' 'mdwell: credential helper execution failed' >&2; "
            "return 1; }; }; f")


def invoke(executable, git, env, root, request, operation="get"):
    if git:
        command = [git, "-c", "credential.helper=", "-c",
                   "credential.helper=" + helper_shell(executable),
                   "-c", "credential.username=x-access-token",
                   "credential", "fill"]
    else:
        command = [str(executable), "--mdwell-git-credential", operation]
    try:
        return subprocess.run(command, input=request, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, cwd=root, env=env,
                              timeout=8)
    except subprocess.TimeoutExpired:
        return None


def run_binary(executable, git, root):
    digest = hashlib.file_digest(executable.open("rb"), "sha256").hexdigest()
    print(f"Executable: {executable}")
    print(f"  sha256={digest} bytes={executable.stat().st_size}")
    failures = 0

    def case(name, request, *, via_git=False, expected="success", operation="get"):
        nonlocal failures
        env = isolated_env(root)
        # For unreachable IPC, reserve a loopback port without listening. Keeping
        # it bound avoids a race in which another process becomes the endpoint.
        reserved = socket.socket()
        reserved.bind(("127.0.0.1", 0))
        with MockIPC() as server:
            env["MDWELL_GIT_CREDENTIAL_ENDPOINT"] = (
                "127.0.0.1:%d" % reserved.getsockname()[1]
                if expected == "unreachable" else server.endpoint)
            result = invoke(executable, git if via_git else None, env, root,
                            request, operation)
        reserved.close()
        reasons = []
        if result is None:
            reasons.append("timeout")
        else:
            # Never include raw stdout/stderr, input, capability or fake token
            # in diagnostics, including failed assertions.
            if TOKEN in result.stderr or CAPABILITY in result.stderr:
                reasons.append("sensitive fixture data in stderr")
            if CAPABILITY in result.stdout:
                reasons.append("capability in stdout")
            if expected == "success":
                fields = result.stdout.splitlines()
                if result.returncode != 0:
                    reasons.append("nonzero exit")
                if fields.count(b"password=" + TOKEN) != 1:
                    reasons.append("expected fake credential absent/duplicated")
                if result.stderr:
                    reasons.append("unexpected stderr")
                expected_frame = CAPABILITY + b"\n" + request.rstrip(b"\n") + b"\n\n"
                if via_git:
                    # Real Git injects the configured username before challenges.
                    lines = request.rstrip(b"\n").split(b"\n")
                    lines.insert(2, b"username=x-access-token")
                    expected_frame = CAPABILITY + b"\n" + b"\n".join(lines) + b"\n\n"
                if server.frames != [expected_frame]:
                    reasons.append("IPC frame absent/incorrect (EOF normalization/challenges)")
            else:
                if result.stdout:
                    reasons.append("unexpected stdout (credential leak)")
                if server.frames:
                    reasons.append("unexpected IPC request")
                want_success = expected == "noop" or (expected == "mismatch" and not via_git)
                if (result.returncode == 0) != want_success:
                    reasons.append("unexpected exit status")
                if want_success and result.stderr:
                    reasons.append("unexpected stderr")
        if server.errors:
            reasons.extend(server.errors)
        status = "FAIL" if reasons else "PASS"
        detail = "; ".join(reasons)
        if result is not None:
            detail += ("; " if detail else "") + (
                f"exit={result.returncode} ipc={len(server.frames)} "
                f"stdout_bytes={len(result.stdout)} stderr_bytes={len(result.stderr)} "
                f"ipc_marker={IPC_MARKER in result.stderr} "
                f"shell_marker={START_MARKER in result.stderr}")
        print(f"  {status} {name}: {detail}")
        failures += bool(reasons)

    basic = b"protocol=https\nhost=github.com\n"
    challenge = basic + b"wwwauth[]=Basic realm=GitHub\nwwwauth[]=Bearer realm=GitHub\n"
    # Direct probes separate EOF framing from challenge handling. Real Git's
    # helper invocation closes stdin at EOF; it does not send a blank line.
    for name, request in (("basic", basic), ("challenges", challenge)):
        case("direct/" + name + "/blank", request + b"\n")
        case("direct/" + name + "/EOF", request)
        case("git-fill/" + name, request + b"\n", via_git=True)
    # Git's HTTPS transport advertises repeated informational capabilities;
    # credential fill alone does not reproduce these fields.
    capabilities = (b"capability[]=authtype\ncapability[]=state\n" + challenge)
    case("direct/capabilities/blank", capabilities + b"\n")
    case("direct/capabilities/EOF", capabilities)
    mismatch = basic.replace(b"github.com", b"not-github.invalid")
    for via_git in (False, True):
        prefix = "git-fill/" if via_git else "direct/"
        case(prefix + "host-mismatch", mismatch + b"\n", via_git=via_git, expected="mismatch")
        case(prefix + "unreachable", basic + b"\n", via_git=via_git, expected="unreachable")
    for operation in ("store", "erase"):
        case("direct/" + operation, basic + RESPONSE, operation=operation, expected="noop")
    print(f"  Result: {14 - failures}/14 passed")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executables", nargs="+", help="literal executable paths")
    args = parser.parse_args()
    git = shutil.which("git", path=os.defpath)
    if git is None:
        parser.error("git unavailable on default system PATH")
    executables = [Path(arg).absolute() for arg in args.executables]
    for executable in executables:
        if not executable.is_file() or not os.access(executable, os.X_OK):
            parser.error("each argument must name an existing executable file")
    failures = 0
    with tempfile.TemporaryDirectory(prefix="mdwell-helper-test-") as directory:
        for executable in executables:
            failures += run_binary(executable, git, Path(directory))
    print(f"Total failed cases: {failures}; no captured credential output printed")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
