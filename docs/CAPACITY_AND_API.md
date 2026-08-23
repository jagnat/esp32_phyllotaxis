# Sketch capacity and API readiness

Measured on the 16 MiB ESP32-S3 board.

## Capacity

The gallery provides 100 fixed-size slots:

- flash after the 4 MiB factory app partition: 12,517,376 bytes (11.9375 MiB)
- maximum module payload per slot: 32,704 bytes (32 KiB minus 64-byte header)
- 100 slots: 3,276,800 bytes (3.125 MiB)
- space remaining after 100 slots: 9,240,576 bytes (8.8125 MiB)
- absolute fixed-slot maximum if all remaining flash were consumed: 382 slots

The reference Rust WASM sketch is 6,342 bytes in release mode, so 100 copies
contain 634,200 bytes of module data but occupy 3.125 MiB in the fixed-slot
layout. Its debug build is 2,870,425 bytes and cannot be uploaded; contributor
instructions must require an optimized release build.

One hundred slots leave enough unallocated flash for two future 4 MiB OTA app
partitions plus roughly 832 KiB. Consuming the theoretical 382-slot maximum
would leave no room for OTA, crash data, or future persistent state.

The firmware now uses exactly 100 slots rather than 128. `SLOTS` is 100 and the
`sketches` partition is `0x320000`, retaining the `0x410000` base address.

## Runtime cost

Stored sketches do not consume RAM. The host reads and instantiates only the
selected module, and retains only one `SketchInstance`.

For the 6,347-byte reference module on hardware:

- load: about 15.6 ms
- render: about 6.3 ms and 52.3K fuel per frame
- external heap retained by the instance: about 94 KiB
- internal heap remained at 98,304 bytes free

Guests have a 1 MiB linear-memory limit and 150K fuel per frame. Gallery size
therefore does not change steady-state render memory, although a larger list
does make enumeration linearly slower and increases its temporary result by a
few KiB.

## API hardening status

The WASM ABI document and reference project are suitable for trusted sketch
authors. Modules have no WASI access, receive only the documented `phyllo`
imports, run with fuel metering, and are isolated from the native firmware.

The management API now addresses the main local-network risks identified in
the initial audit:

- upload, delete, run, and native selection require a bearer token of at least
  16 bytes; mutations are disabled when no token is compiled in;
- request size and content type are checked before allocating the upload body;
- upload performs full production-limit instantiation and validates imports,
  ABI, capabilities, exported types, pointers, memory, tables, and start code;
- load/start fuel is capped at 500K, frame fuel at 120K, guest memory at 1 MiB,
  and table elements at 1,024;
- the API is under `/api`, returns JSON, and uses meaningful status codes;
- slot metadata is committed last after the CRC-protected payload;
- the partition and implementation provide 100 slots.

Remaining boundaries: HTTP has no TLS, so bearer access is appropriate only on
a trusted WPA network; WebSocket/DDP streaming is outside the authenticated
WASM management API; and uploaded gallery entries are not yet individual
choices in the physical menu. Upload and rewrite stress testing with Wi-Fi
enabled is still required before unattended deployment.

## Decision

There is ample flash for 100 sketches, and storing that many does not threaten
runtime RAM. The ABI and token-protected API are suitable for trusted authors
on the installation's WPA network. They are not a public-Internet service, and
the token should not be distributed beyond people allowed to change the
installation.
