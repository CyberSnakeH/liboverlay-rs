"""Bounded Unix-socket framing and descriptor forwarding for test fixtures."""

import array
import os
import socket


def read_exact(peer, size, descriptors=None):
    if not 0 <= size <= 128 * 1024 * 1024:
        raise ValueError("invalid fixture message size")
    result = bytearray()
    while len(result) < size:
        if descriptors is None:
            part = peer.recv(size - len(result))
        else:
            part, ancillary, flags, _ = peer.recvmsg(
                size - len(result), socket.CMSG_SPACE(256 * 4)
            )
            for level, kind, data in ancillary:
                if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                    received = array.array("i")
                    received.frombytes(data)
                    descriptors.extend(received)
            if flags & socket.MSG_CTRUNC:
                raise ValueError("truncated fixture file descriptors")
        if not part:
            raise EOFError
        result.extend(part)
    return bytes(result)


def send_with_fds(peer, payload, descriptors):
    try:
        if descriptors:
            sent = peer.sendmsg(
                [payload],
                [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", descriptors))],
            )
            peer.sendall(payload[sent:])
        else:
            peer.sendall(payload)
    finally:
        for descriptor in descriptors:
            os.close(descriptor)
        descriptors.clear()
