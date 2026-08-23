# Sketch capacity and API readiness

Measured on the 16 MiB ESP32-S3 board with firmware `7ba75f5`.

## Capacity

The current gallery is limited to 16 sketches by configuration, not by the
hardware:

- flash after the 4 MiB factory app partition: 12,517,376 bytes (11.9375 MiB)
- current gallery: 16 x 32 KiB = 524,288 bytes
- maximum module payload per slot: 32,704 bytes (32 KiB minus 64-byte header)
- 100 slots: 3,276,800 bytes (3.125 MiB)
- space remaining after 100 slots: 9,240,576 bytes (8.8125 MiB)
- absolute fixed-slot maximum if all remaining flash were consumed: 382 slots

The reference Rust WASM sketch is 6,347 bytes in release mode, so 100 copies
contain 634,700 bytes of module data but occupy 3.125 MiB in the fixed-slot
layout. Its debug build is 2,870,425 bytes and cannot be uploaded; contributor
instructions must require an optimized release build.

One hundred slots leave enough unallocated flash for two future 4 MiB OTA app
partitions plus roughly 832 KiB. Consuming the theoretical 382-slot maximum
would leave no room for OTA, crash data, or future persistent state.

Recommendation: use exactly 100 slots rather than 128. Change `SLOTS` in
`src/gallery.rs` to 100 and the `sketches` partition in `partitions.csv` to
`0x320000`. The base address can remain `0x410000`.

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

## What is safe to publish now

The WASM ABI v2 document and reference project are suitable for trusted sketch
authors. Modules have no WASI access, receive only the documented `phyllo`
imports, run with fuel metering, and are isolated from the native firmware.

The current HTTP management API is not suitable for an untrusted or public
network yet:

- there is no authentication or authorization for upload, delete, run,
  selection, or streaming endpoints;
- the upload extractor allocates the complete declared request body before the
  handler applies the 32 KiB size check;
- upload validation checks module shape, but ABI version, pointer layout,
  imports, and instantiation are not fully checked until the sketch is run;
- guest linear memory is capped, but table elements are not explicitly capped;
- module start code gets a 100M-fuel load allowance on the render task, so a
  hostile module can cause a long stall and watchdog reboot;
- API errors are plain-text 200 responses, and the endpoints themselves are
  not versioned;
- slot writes are CRC-protected but not transactional: interrupted writes fail
  closed at run time, yet can still appear in a listing;
- uploaded gallery entries are not yet individual choices in the physical
  menu, whose metadata currently knows only one generic WASM entry.

Before broad access, add an upload body limit before allocation, authentication
or a physical maintenance-mode gate, full preflight instantiation with tighter
load and table limits, structured status codes, and a versioned gallery
manifest containing at least ABI version, display name, and menu color. Then
stress-test uploads and rewrites with Wi-Fi enabled while rendering.

## Decision

There is ample flash for 100 sketches, and storing that many does not threaten
runtime RAM. Publish the ABI to a trusted group now if desired. Do not expose
the current write API to untrusted users until the safeguards above are in
place.
