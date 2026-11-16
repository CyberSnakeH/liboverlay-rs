"""Mask selected registry globals on a runner-owned Wayland connection."""

import contextlib
import os
from pathlib import Path
import socket
import struct
import tempfile
import threading
from unix_transport import read_exact, send_with_fds


@contextlib.contextmanager
def mask_globals(upstream_path, hidden, block_frames=False, input_probe=None):
    with tempfile.TemporaryDirectory(prefix="liboverlay-wayland-proxy-") as directory:
        path = Path(directory) / "wayland.sock"
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path))
        listener.listen()
        listener.settimeout(0.2)
        stopped = threading.Event()
        peers, workers, failures = [], [], []
        lock = threading.Lock()
        dropped_frames = []

        def connect(client):
            upstream = socket.socket(socket.AF_UNIX)
            registries = set()
            compositors, surfaces, callbacks = set(), set(), set()
            registry_lock = threading.Lock()
            client_write = threading.Lock()

            def send_client(message, descriptors):
                with client_write:
                    send_with_fds(client, message, descriptors)

            probe = (
                input_probe.attach(lambda message: send_client(message, []))
                if input_probe
                else None
            )

            def forward(source, destination, requests):
                descriptors = []
                try:
                    while not stopped.is_set():
                        header = read_exact(source, 8, descriptors)
                        object_id, word = struct.unpack("=II", header)
                        size, opcode = word >> 16, word & 0xFFFF
                        if size < 8 or size % 4:
                            raise ValueError("invalid Wayland fixture frame")
                        message = header + read_exact(source, size - 8, descriptors)
                        if requests and probe:
                            message = probe.request(message)
                            if message is None:
                                continue  # Keep unrelated ancillary FDs for the next forwarded request.
                            object_id, word = struct.unpack_from("=II", message)
                            size, opcode = word >> 16, word & 0xFFFF
                        if requests and object_id == 1 and opcode == 1:
                            with registry_lock:
                                registries.add(struct.unpack_from("=I", message, 8)[0])
                        if block_frames and requests:
                            with registry_lock:
                                if object_id in registries and opcode == 0:
                                    length = struct.unpack_from("=I", message, 12)[0]
                                    if (
                                        message[16 : 16 + length - 1]
                                        == b"wl_compositor"
                                    ):
                                        compositors.add(
                                            struct.unpack_from(
                                                "=I", message, len(message) - 4
                                            )[0]
                                        )
                                elif object_id in compositors and opcode == 0:
                                    surfaces.add(
                                        struct.unpack_from("=I", message, 8)[0]
                                    )
                                elif object_id in surfaces and opcode == 3:
                                    callbacks.add(
                                        struct.unpack_from("=I", message, 8)[0]
                                    )
                        if not requests:
                            with registry_lock:
                                registry = object_id in registries
                                frame_done = (
                                    block_frames
                                    and object_id in callbacks
                                    and opcode == 0
                                )
                                if object_id == 1 and opcode == 1:
                                    callbacks.discard(
                                        struct.unpack_from("=I", message, 8)[0]
                                    )
                            if frame_done:
                                dropped_frames.append(object_id)
                                continue
                            if registry and opcode == 0:
                                length = struct.unpack_from("=I", message, 12)[0]
                                interface = message[16 : 16 + length - 1].decode()
                                if interface in hidden:
                                    continue
                        if requests:
                            send_with_fds(destination, message, descriptors)
                        else:
                            messages = probe.events(message) if probe else [message]
                            for outgoing in messages:
                                send_client(outgoing, descriptors)
                except (EOFError, OSError):
                    pass
                except Exception as error:
                    failures.append(error)
                finally:
                    for descriptor in descriptors:
                        os.close(descriptor)
                    for peer in (source, destination):
                        with contextlib.suppress(OSError):
                            peer.shutdown(socket.SHUT_RDWR)

            try:
                upstream.connect(str(upstream_path))
                with lock:
                    peers.append(upstream)
                sender = threading.Thread(
                    target=forward, args=(client, upstream, True), daemon=True
                )
                sender.start()
                forward(upstream, client, False)
                sender.join(timeout=2)
            except OSError as error:
                if not stopped.is_set():
                    failures.append(error)
            finally:
                upstream.close()
                client.close()

        def accept():
            while not stopped.is_set():
                try:
                    client, _ = listener.accept()
                except socket.timeout:
                    continue
                except OSError:
                    return
                with lock:
                    peers.append(client)
                worker = threading.Thread(target=connect, args=(client,), daemon=True)
                workers.append(worker)
                worker.start()

        acceptor = threading.Thread(target=accept, daemon=True)
        acceptor.start()
        try:
            yield str(path)
        finally:
            stopped.set()
            listener.close()
            acceptor.join(timeout=2)
            with lock:
                for peer in peers:
                    with contextlib.suppress(OSError):
                        peer.shutdown(socket.SHUT_RDWR)
            for worker in workers:
                worker.join(timeout=2)
            if failures:
                raise RuntimeError(f"Wayland fixture proxy failed: {failures}")
        if block_frames and not dropped_frames:
            raise RuntimeError("fixture did not observe any frame callback to withhold")
