"""Controlled wl_touch peer on an owned proxy; never acquires host input devices."""

import struct
import threading
import time


def message(object_id, opcode, *arguments):
    payload = struct.pack(
        "=" + "I" * len(arguments), *(value & 0xFFFFFFFF for value in arguments)
    )
    return struct.pack("=II", object_id, (len(payload) + 8) << 16 | opcode) + payload


def string(value):
    value = value.encode() + b"\0"
    return struct.pack("=I", len(value)) + value + b"\0" * (-len(value) % 4)


class TouchProbe:
    """Adds one synthetic seat and touch capability to a runner-owned connection."""

    EXTRA_SEAT = 0x7FFF0001

    def __init__(self):
        self.lock = threading.RLock()
        self.connection = None

    def attach(self, send):
        with self.lock:
            if self.connection is not None:
                raise RuntimeError(
                    "touch fixture expects exactly one owned overlay connection"
                )
            self.connection = _Connection(send)
            return self.connection

    def action(self, stage):
        deadline = time.monotonic() + 5
        while True:
            with self.lock:
                peer = self.connection
                if peer:
                    with peer.lock:
                        if len(peer.touch) == 2 and peer.surface is not None:
                            return peer.action(stage)
            if time.monotonic() > deadline:
                raise RuntimeError("owned client did not bind both fixture touch seats")
            time.sleep(0.01)


class _Connection:
    def __init__(self, send):
        self.send = send
        self.lock = threading.RLock()
        self.registries, self.compositors = set(), set()
        self.seats, self.touch = {}, {}
        self.announced = set()
        self.surface = None

    def request(self, data):
        object_id, word = struct.unpack_from("=II", data)
        opcode = word & 0xFFFF
        with self.lock:
            if object_id == 1 and opcode == 1:
                self.registries.add(struct.unpack_from("=I", data, 8)[0])
            if object_id in self.registries and opcode == 0:
                name, length = struct.unpack_from("=II", data, 8)
                interface = data[16 : 16 + length - 1]
                new_id = struct.unpack_from("=I", data, len(data) - 4)[0]
                if interface == b"wl_compositor":
                    self.compositors.add(new_id)
                if interface == b"wl_seat":
                    self.seats[new_id] = name
                    if name == TouchProbe.EXTRA_SEAT:
                        self.send(message(new_id, 0, 4))  # capabilities: Touch
                        # Reserve the same client object ID upstream with an
                        # inert region, preserving libwayland's ID allocation.
                        return message(next(iter(self.compositors)), 1, new_id)
            if object_id in self.compositors and opcode == 0:
                self.surface = struct.unpack_from("=I", data, 8)[0]
            if object_id in self.seats:
                if opcode == 2:  # get_touch; the fixture owns this object, not Sway.
                    new_id = struct.unpack_from("=I", data, 8)[0]
                    self.touch[new_id] = self.seats[object_id]
                    return message(next(iter(self.compositors)), 1, new_id)
                if opcode == 3 and self.seats[object_id] == TouchProbe.EXTRA_SEAT:
                    del self.seats[object_id]
                    return message(object_id, 0)  # destroy the upstream inert region
            if object_id in self.touch and opcode == 0:
                del self.touch[object_id]
                return message(object_id, 0)
        return data

    def events(self, data):
        object_id, word = struct.unpack_from("=II", data)
        opcode = word & 0xFFFF
        with self.lock:
            if object_id in self.seats and opcode == 0:
                flags = struct.unpack_from("=I", data, 8)[0] | 4
                return [message(object_id, 0, flags)]
            if (
                object_id in self.registries
                and opcode == 0
                and object_id not in self.announced
            ):
                length = struct.unpack_from("=I", data, 12)[0]
                if data[16 : 16 + length - 1] == b"wl_seat":
                    self.announced.add(object_id)
                    payload = (
                        struct.pack("=I", TouchProbe.EXTRA_SEAT)
                        + string("wl_seat")
                        + struct.pack("=I", 5)
                    )
                    extra = (
                        struct.pack("=II", object_id, (len(payload) + 8) << 16)
                        + payload
                    )
                    return [data, extra]
        return [data]

    def action(self, stage):
        native = next(
            obj for obj, seat in self.touch.items() if seat != TouchProbe.EXTRA_SEAT
        )
        extra = next(
            obj for obj, seat in self.touch.items() if seat == TouchProbe.EXTRA_SEAT
        )
        if stage == "touch-start":
            for obj in (native, extra):
                self.send(
                    message(
                        obj, 0, 1, 1, self.surface, 7, 12 * 256 + 128, 20 * 256 + 64
                    )
                )
                self.send(message(obj, 3))
        elif stage == "touch-move":
            self.send(message(native, 2, 2, 7, -3 * 256 - 128, 22 * 256 + 64))
            self.send(message(native, 3))
        elif stage == "touch-cancel":
            self.send(message(native, 4))
            self.send(message(native, 3))
        elif stage == "touch-orphan":
            self.send(message(native, 2, 3, 7, 1, 1))
            self.send(message(native, 3))
        elif stage == "touch-remove":
            for registry in self.registries:
                self.send(message(registry, 1, TouchProbe.EXTRA_SEAT))
        else:
            raise RuntimeError(f"unknown touch fixture stage: {stage}")
        return "sent"
