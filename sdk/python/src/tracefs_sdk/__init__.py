"""Client for the tracefs-daemon socket protocol: one JSON request line and one JSON
response line per Unix socket connection. The TypedDicts in `protocol` are generated
from schema/tracefs.schema.json; `Results` maps each op to its result type."""

import json
import os
import socket
from collections.abc import Mapping
from typing import Any

from . import protocol

DEFAULT_SOCKET = "/run/tracefs/tracefs-daemon.sock"


class TracefsError(Exception):
    """A request the daemon refused; `code` is listed in `protocol.ErrorBody`."""

    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


class Client:
    def __init__(self, socket_path: str | None = None):
        self.socket_path = socket_path or os.environ.get("TRACEFS_SOCKET", DEFAULT_SOCKET)

    def call(self, op: str, args: Mapping[str, Any] | None = None) -> Any:
        """Send one request and return its result (typed as `protocol.Results[op]`).
        Paths in arguments must be absolute: the daemon resolves them, not this process."""
        req: dict[str, Any] = {"op": op} if args is None else {"op": op, "args": args}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.connect(self.socket_path)
            s.sendall(json.dumps(req).encode() + b"\n")
            s.shutdown(socket.SHUT_WR)
            with s.makefile("rb") as f:
                line = f.readline()
        try:
            r: protocol.Response = json.loads(line)
        except ValueError as e:
            raise TracefsError("invalid", f"bad response: {e}") from None
        if not r.get("ok"):
            err = r.get("error")
            if err is None:
                raise TracefsError("internal", "unknown error")
            raise TracefsError(err["code"], err["message"])
        return r.get("result")


__all__ = ["DEFAULT_SOCKET", "TracefsError", "Client", "protocol"]
