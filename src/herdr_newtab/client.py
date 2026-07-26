"""Talk to the running Herdr server.

Two transports, same surface:

* the Unix socket at ``HERDR_SOCKET_PATH`` -- one request per connection,
  newline-delimited JSON, ~15ms round trip. Used for the snapshot the picker
  reads when it opens.
* the ``herdr`` CLI at ``HERDR_BIN_PATH`` -- portable, works when the socket
  is missing or is a Windows named pipe. Used as the automatic fallback for
  everything else.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
from typing import Any, Dict, List, Optional


class HerdrError(RuntimeError):
    """A Herdr request failed, or no server could be reached."""


def _env_path(name: str) -> Optional[str]:
    value = os.environ.get(name)
    return value if value else None


def resolve_bin() -> str:
    """The Herdr binary to shell out to.

    HERDR_BIN_PATH can outlive the binary it names -- upgrading Herdr under a
    running server leaves the old path dangling -- so an unusable value falls
    back to whatever `herdr` resolves to on PATH.
    """
    candidate = _env_path("HERDR_BIN_PATH")
    if candidate and os.path.isfile(candidate) and os.access(candidate, os.X_OK):
        return candidate
    return "herdr"


class HerdrClient:
    def __init__(
        self,
        socket_path: Optional[str] = None,
        bin_path: Optional[str] = None,
        timeout: float = 4.0,
    ) -> None:
        if socket_path is None:
            socket_path = _env_path("HERDR_SOCKET_PATH")
        self.socket_path = socket_path
        self.bin_path = bin_path or resolve_bin()
        self.timeout = timeout
        self._socket_ok = self._socket_usable()

    @property
    def socket_ok(self) -> bool:
        return self._socket_ok

    # -- transports ---------------------------------------------------------

    def _socket_usable(self) -> bool:
        if not self.socket_path or not hasattr(socket, "AF_UNIX"):
            return False
        try:
            return os.path.exists(self.socket_path)
        except OSError:
            return False

    def _socket_call(self, method: str, params: Dict[str, Any]) -> Dict[str, Any]:
        payload = json.dumps({"id": "newtab", "method": method, "params": params}) + "\n"
        conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        conn.settimeout(self.timeout)
        try:
            conn.connect(self.socket_path)  # type: ignore[arg-type]
            conn.sendall(payload.encode("utf-8"))
            chunks: List[bytes] = []
            buffered = b""
            while b"\n" not in buffered:
                chunk = conn.recv(65536)
                if not chunk:
                    break
                chunks.append(chunk)
                buffered = b"".join(chunks)
        finally:
            try:
                conn.close()
            except OSError:
                pass
        line = buffered.split(b"\n", 1)[0]
        if not line:
            raise HerdrError("herdr closed the connection without a response")
        return json.loads(line.decode("utf-8", "replace"))

    def _cli(self, args: List[str]) -> Dict[str, Any]:
        try:
            proc = subprocess.run(
                [self.bin_path] + args,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=self.timeout + 4.0,
                check=False,  # a non-zero exit still carries a usable message
            )
        except FileNotFoundError:
            raise HerdrError("herdr binary not found (set HERDR_BIN_PATH)")
        except subprocess.TimeoutExpired:
            raise HerdrError("herdr %s timed out" % " ".join(args))
        out = proc.stdout.decode("utf-8", "replace").strip()
        if not out:
            err = proc.stderr.decode("utf-8", "replace").strip()
            raise HerdrError(err or "herdr %s produced no output" % " ".join(args))
        try:
            return json.loads(out.splitlines()[-1])
        except ValueError:
            raise HerdrError("herdr %s returned unparseable output" % " ".join(args))

    @staticmethod
    def _unwrap(response: Dict[str, Any]) -> Dict[str, Any]:
        error = response.get("error")
        if error:
            message = error.get("message") if isinstance(error, dict) else str(error)
            raise HerdrError(message or "herdr request failed")
        result = response.get("result")
        if not isinstance(result, dict):
            raise HerdrError("herdr returned an unexpected response shape")
        return result

    def call(self, method: str, params: Dict[str, Any], cli_args: List[str]) -> Dict[str, Any]:
        """Run a request over the socket, falling back to the CLI."""
        if self._socket_ok:
            try:
                return self._unwrap(self._socket_call(method, params))
            except HerdrError:
                raise
            except (OSError, ValueError):
                # Transport-level failure only: the server may have restarted
                # or handed off. Stop trusting the socket and use the CLI.
                self._socket_ok = False
        return self._unwrap(self._cli(cli_args))

    # -- operations ---------------------------------------------------------

    def snapshot(self) -> Dict[str, Any]:
        result = self.call("session.snapshot", {}, ["api", "snapshot"])
        snapshot = result.get("snapshot")
        if not isinstance(snapshot, dict):
            raise HerdrError("session snapshot missing from response")
        return snapshot

    def create_tab(
        self, workspace_id: str, cwd: str, label: Optional[str] = None
    ) -> Dict[str, Any]:
        params: Dict[str, Any] = {"workspace_id": workspace_id, "cwd": cwd, "focus": True}
        args = ["tab", "create", "--workspace", workspace_id, "--cwd", cwd, "--focus"]
        if label:
            params["label"] = label
            args += ["--label", label]
        return self.call("tab.create", params, args)

    def create_workspace(self, cwd: str, label: Optional[str] = None) -> Dict[str, Any]:
        params: Dict[str, Any] = {"cwd": cwd, "focus": True}
        args = ["workspace", "create", "--cwd", cwd, "--focus"]
        if label:
            params["label"] = label
            args += ["--label", label]
        return self.call("workspace.create", params, args)

    def start_agent(self, name: str, kind: str, pane_id: str, timeout_ms: int = 60000) -> None:
        """Launch an agent into a pane that is still an idle shell.

        Herdr only returns once the agent owns the terminal and is ready for
        input, so this can sit for many seconds. Callers that want the popup
        gone first should run it in a detached child.
        """
        self.call(
            "agent.start",
            {"name": name, "kind": kind, "pane_id": pane_id, "timeout_ms": timeout_ms},
            [
                "agent",
                "start",
                name,
                "--kind",
                kind,
                "--pane",
                pane_id,
                "--timeout",
                str(timeout_ms),
            ],
        )
