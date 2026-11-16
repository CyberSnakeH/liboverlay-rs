"""Local test transport that masks selected QueryExtension replies.

Only connections to the runner's private Xvfb are forwarded. No display contents
are recorded. Requests and replies otherwise remain byte-for-byte unchanged.
"""

import contextlib
import os
from pathlib import Path
import secrets
import socket
import struct
import threading
from unix_transport import read_exact, send_with_fds


@contextlib.contextmanager
def mask_extensions(display, names):
    """Expose an Xvfb through a new Unix display with selected extensions absent."""
    listener = socket.socket(socket.AF_UNIX)
    for _ in range(100):
        number = secrets.randbelow(40_000) + 10_000
        path = Path(f"/tmp/.X11-unix/X{number}")
        try:
            listener.bind(str(path))
            break
        except FileExistsError:
            continue
        except OSError as error:
            if error.errno == 98:  # EADDRINUSE
                continue
            raise
    else:
        listener.close()
        raise RuntimeError("could not allocate a private X11 proxy socket")
    listener.listen()
    listener.settimeout(0.2)
    path.chmod(0o600)
    stopped = threading.Event()
    peers = []
    workers = []
    failures = []
    lock = threading.Lock()
    hidden = {name.encode() for name in names}

    def connect(client):
        upstream = socket.socket(socket.AF_UNIX)
        try:
            upstream.connect(f"/tmp/.X11-unix/X{display}")
            with lock:
                peers.append(upstream)
            setup = read_exact(client, 12)
            endian = "<" if setup[:1] == b"l" else ">"
            auth_name, auth_data = struct.unpack_from(endian + "HH", setup, 6)
            auth_bytes = ((auth_name + 3) // 4 + (auth_data + 3) // 4) * 4
            upstream.sendall(setup + read_exact(client, auth_bytes))
            response = read_exact(upstream, 8)
            length = struct.unpack_from(endian + "H", response, 6)[0] * 4
            client.sendall(response + read_exact(upstream, length))
            masked_sequences = set()
            sequence_lock = threading.Lock()

            def requests():
                sequence = 0
                descriptors = []
                try:
                    while not stopped.is_set():
                        header = read_exact(client, 4, descriptors)
                        units = struct.unpack_from(endian + "H", header, 2)[0]
                        if units == 0:
                            header += read_exact(client, 4, descriptors)
                            units = struct.unpack_from(endian + "I", header, 4)[0]
                        length = units * 4
                        if length < len(header) or length > 128 * 1024 * 1024:
                            raise ValueError("invalid fixture request size")
                        sequence = (sequence + 1) & 0xFFFF
                        remaining = length - len(header)
                        if header[0] == 98:  # QueryExtension
                            request = header + read_exact(
                                client, remaining, descriptors
                            )
                            name_length = struct.unpack_from(endian + "H", request, 4)[
                                0
                            ]
                            if request[8 : 8 + name_length] in hidden:
                                with sequence_lock:
                                    masked_sequences.add(sequence)
                            send_with_fds(upstream, request, descriptors)
                        else:
                            # Large PutImage requests need no inspection. Stream
                            # them with bounded storage instead of buffering a frame.
                            send_with_fds(upstream, header, descriptors)
                            while remaining:
                                size = min(remaining, 64 * 1024)
                                chunk = read_exact(client, size, descriptors)
                                send_with_fds(upstream, chunk, descriptors)
                                remaining -= size
                except (EOFError, OSError):
                    pass
                except Exception as error:
                    failures.append(error)
                finally:
                    for descriptor in descriptors:
                        os.close(descriptor)
                    with contextlib.suppress(OSError):
                        upstream.shutdown(socket.SHUT_RDWR)

            sender = threading.Thread(target=requests, daemon=True)
            sender.start()
            try:
                while not stopped.is_set():
                    response = bytearray(read_exact(upstream, 32))
                    kind = response[0] & 0x7F
                    length = (
                        struct.unpack_from(endian + "I", response, 4)[0] * 4
                        if kind in (1, 35)
                        else 0
                    )
                    if length > 128 * 1024 * 1024:
                        raise ValueError("invalid fixture response size")
                    if kind == 1:
                        sequence = struct.unpack_from(endian + "H", response, 2)[0]
                        with sequence_lock:
                            if sequence in masked_sequences:
                                masked_sequences.remove(sequence)
                                response[8:12] = b"\0" * 4
                    client.sendall(response + read_exact(upstream, length))
            finally:
                with contextlib.suppress(OSError):
                    client.shutdown(socket.SHUT_RDWR)
                sender.join(timeout=2)
        except (EOFError, OSError):
            pass
        except Exception as error:
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
        yield f":{number}"
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
        path.unlink(missing_ok=True)
    if failures:
        raise RuntimeError(f"X11 fixture proxy failed: {failures}")
