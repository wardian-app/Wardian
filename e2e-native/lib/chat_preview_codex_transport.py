"""Local fixture transport only. No TCP, provider calls, or transcript writes.

Windows selects winsock-af-unix explicitly; python-af-unix never falls back.
The protocol itself is implemented by chatPreviewCodexPeer.mjs.
"""

import base64
import ctypes
import json
import os
import socket
import struct
import sys
import threading

MAX_LINE = 2 * 1024 * 1024


class WindowsUnixSocket:
    """Small explicit AF_UNIX adapter for Python builds without socket.AF_UNIX."""

    def __init__(self, handle=None):
        if sys.platform != "win32":
            raise RuntimeError("winsock_transport_requires_windows")
        # Importing socket initializes Winsock for this interpreter.
        self.api = ctypes.WinDLL("Ws2_32", use_last_error=True)
        word = ctypes.c_size_t
        for name, arguments, result in [
            ("socket", [ctypes.c_int] * 3, word),
            ("bind", [word, ctypes.c_void_p, ctypes.c_int], ctypes.c_int),
            ("connect", [word, ctypes.c_void_p, ctypes.c_int], ctypes.c_int),
            ("listen", [word, ctypes.c_int], ctypes.c_int),
            ("accept", [word, ctypes.c_void_p, ctypes.c_void_p], word),
            ("send", [word, ctypes.c_void_p, ctypes.c_int, ctypes.c_int], ctypes.c_int),
            ("recv", [word, ctypes.c_void_p, ctypes.c_int, ctypes.c_int], ctypes.c_int),
            ("shutdown", [word, ctypes.c_int], ctypes.c_int),
            ("closesocket", [word], ctypes.c_int),
            ("WSAGetLastError", [], ctypes.c_int),
        ]:
            function = getattr(self.api, name)
            function.argtypes, function.restype = arguments, result
        self.handle = self.api.socket(1, socket.SOCK_STREAM, 0) if handle is None else handle
        if self.handle == ctypes.c_size_t(-1).value:
            self.fail()

    def fail(self):
        raise OSError(self.api.WSAGetLastError(), "fixture_AF_UNIX_failed")

    def address(self, operation, path):
        encoded = os.fsencode(path)
        if len(encoded) >= 108 or b"\0" in encoded:
            raise RuntimeError("fixture_socket_path_bound")
        address = ctypes.create_string_buffer(struct.pack("<H", 1) + encoded + b"\0")
        if getattr(self.api, operation)(self.handle, address, len(address) - 1) == -1:
            self.fail()

    def bind(self, path):
        self.address("bind", path)

    def connect(self, path):
        self.address("connect", path)

    def listen(self, count):
        if self.api.listen(self.handle, count) == -1:
            self.fail()

    def accept(self):
        handle = self.api.accept(self.handle, None, None)
        if handle == ctypes.c_size_t(-1).value:
            self.fail()
        return WindowsUnixSocket(handle)

    def recv(self, count):
        buffer = ctypes.create_string_buffer(count)
        size = self.api.recv(self.handle, buffer, count, 0)
        if size == -1:
            self.fail()
        return buffer.raw[:size]

    def sendall(self, data):
        offset = 0
        while offset < len(data):
            part = data[offset:]
            buffer = ctypes.create_string_buffer(part)
            size = self.api.send(self.handle, buffer, len(part), 0)
            if size <= 0:
                self.fail()
            offset += size

    def shutdown(self, direction):
        if self.api.shutdown(self.handle, direction) == -1:
            self.fail()

    def close(self):
        if self.handle is not None:
            self.api.closesocket(self.handle)
            self.handle = None


def unix_socket(transport):
    if transport == "winsock-af-unix":
        return WindowsUnixSocket()
    if transport == "python-af-unix" and hasattr(socket, "AF_UNIX"):
        return socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    raise RuntimeError("explicit_AF_UNIX_transport_unavailable")


def proxy(path, transport):
    peer = unix_socket(transport)
    peer.connect(path)

    def upload():
        try:
            while True:
                data = os.read(0, 65536)
                if not data:
                    break
                peer.sendall(data)
        except OSError:
            pass
        finally:
            try:
                peer.shutdown(socket.SHUT_WR)
            except OSError:
                pass

    threading.Thread(target=upload, daemon=True).start()
    try:
        while True:
            data = peer.recv(65536)
            if not data:
                break
            sys.stdout.buffer.write(data)
            sys.stdout.buffer.flush()
    finally:
        peer.close()


def serve(path, transport):
    if not os.path.isabs(path) or os.path.lexists(path):
        raise RuntimeError("socket_target_not_new_absolute_path")
    listener = unix_socket(transport)
    listener.bind(path)
    identity = os.lstat(path)
    listener.listen(4)
    sockets = {}
    output = threading.Lock()
    stopped = threading.Event()

    def emit(value):
        if not stopped.is_set():
            with output:
                sys.stdout.write(json.dumps(value) + "\n")
                sys.stdout.flush()

    def read(peer, channel):
        try:
            while not stopped.is_set():
                data = peer.recv(65536)
                if not data:
                    break
                emit({"id": channel, "data": base64.b64encode(data).decode("ascii")})
        except OSError:
            pass
        finally:
            sockets.pop(channel, None)
            peer.close()
            emit({"id": channel, "end": True})

    def accept():
        sequence = 0
        try:
            while not stopped.is_set():
                accepted = listener.accept()
                peer = accepted[0] if isinstance(accepted, tuple) else accepted
                if len(sockets) >= 4:
                    peer.close()
                    continue
                sequence += 1
                sockets[sequence] = peer
                emit({"id": sequence, "open": True})
                threading.Thread(target=read, args=(peer, sequence), daemon=True).start()
        except OSError:
            pass

    threading.Thread(target=accept, daemon=True).start()
    emit({"ready": True})
    try:
        while True:
            line = sys.stdin.buffer.readline(MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE:
                raise RuntimeError("transport_line_bound")
            value = json.loads(line)
            peer = sockets.get(value["id"])
            if peer:
                try:
                    if value.get("end"):
                        peer.shutdown(socket.SHUT_RDWR)
                    else:
                        peer.sendall(base64.b64decode(value["data"], validate=True))
                except OSError:
                    pass
    finally:
        stopped.set()
        listener.close()
        for peer in list(sockets.values()):
            peer.close()
        if os.path.lexists(path):
            current = os.lstat(path)
            if (current.st_dev, current.st_ino) == (identity.st_dev, identity.st_ino):
                os.unlink(path)


if __name__ == "__main__":
    try:
        if sys.argv[1] == "probe":
            selected = unix_socket(sys.argv[2])
            selected.close()
            print(json.dumps({"executable": sys.executable, "version": list(sys.version_info[:3]),
                              "transport": sys.argv[2], "stdlib_af_unix": hasattr(socket, "AF_UNIX")}))
        elif sys.argv[1] == "check-socket":
            # IO_REPARSE_TAG_AF_UNIX names a socket, not a path-redirection link.
            # Node cannot lstat it; no connection or filesystem mutation occurs.
            if sys.platform != "win32" or os.lstat(sys.argv[2]).st_reparse_tag != 0x80000023:
                raise RuntimeError("not_windows_AF_UNIX_socket")
        elif sys.argv[1] == "proxy":
            proxy(sys.argv[2], sys.argv[3])
        elif sys.argv[1] == "server":
            serve(sys.argv[2], sys.argv[3])
        else:
            raise RuntimeError("unsupported_transport_role")
    except Exception as error:
        print("fixture_transport_error:" + type(error).__name__ + ":" + str(getattr(error, "errno", "none")), file=sys.stderr)
        sys.exit(1)
