#!/usr/bin/env python3
"""Stream a test pattern to the board over DDP and print telemetry replies.

Usage: ./ddp_demo.py <board-ip>
Select the stream slot first (write API token required):
  curl -X POST -H "Authorization: Bearer $PHYLLO_TOKEN" \
       http://<board-ip>/api/v2/select/5
"""
import math
import select
import socket
import struct
import sys
import time

NUM_LEDS = 90
PORT = 4048
FPS = 60

def frame(t, encoder_pos):
    data = bytearray()
    for i in range(NUM_LEDS):
        h = (i / NUM_LEDS + t * 0.1 + encoder_pos * 0.02) % 1.0
        r, g, b = hsv(h, 1.0, 0.6)
        data += bytes((r, g, b))
    return bytes(data)

def hsv(h, s, v):
    i = int(h * 6)
    f = h * 6 - i
    p, q, t_ = v * (1 - s), v * (1 - f * s), v * (1 - (1 - f) * s)
    rgb = [(v, t_, p), (q, v, p), (p, v, t_), (p, q, v), (t_, p, v), (v, p, q)][i % 6]
    return tuple(int(c * 255) for c in rgb)

def main():
    addr = (sys.argv[1], PORT)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setblocking(False)
    seq = 0
    encoder_pos = 0
    t0 = time.time()
    while True:
        payload = frame(time.time() - t0, encoder_pos)
        header = struct.pack(">BBBBIH", 0x41, seq & 0x0F, 0x01, 0x01, 0, len(payload))
        sock.sendto(header + payload, addr)
        seq += 1
        while select.select([sock], [], [], 0)[0]:
            pkt, _ = sock.recvfrom(256)
            if pkt[:4] == b"PHY1":
                (encoder_pos,) = struct.unpack_from("<i", pkt, 8)
                pressed = pkt[12]
                bands = struct.unpack_from("<6f", pkt, 16)
                bar = "".join("#" if b > (j + 1) / 8 else "." for j, b in enumerate(bands))
                print(f"\rencoder={encoder_pos:6d} btn={pressed} audio[{bar}]", end="")
        time.sleep(1 / FPS)

if __name__ == "__main__":
    main()
