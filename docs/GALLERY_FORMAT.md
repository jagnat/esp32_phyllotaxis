# Gallery storage format v2

The `sketches` partition begins at `0x410000`, occupies `0x320000` bytes, and
contains 100 fixed 32 KiB slots. Each slot contains a 64-byte little-endian
header followed by at most 32,704 bytes of WASM payload.

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | magic `PHYW` |
| 4 | 2 | header format version (`2`) |
| 6 | 2 | header length (`64`) |
| 8 | 4 | payload length |
| 12 | 4 | payload CRC-32/ISO-HDLC |
| 16 | 4 | WASM ABI version (`2`) |
| 20 | 4 | capability bits |
| 24 | 3 | RGB menu color |
| 27 | 1 | reserved |
| 28 | 1 | UTF-8 name length |
| 29 | 3 | reserved |
| 32 | 32 | zero-padded display name |

Names are restricted to ASCII letters, digits, `.`, `_`, and `-`. Reserved
bytes must be written as zero and ignored when reading.

Writes are transactional at slot granularity:

1. Invalidate the header.
2. Write the payload.
3. Write the complete valid header as the commit record.

A reset at any point before step 3 leaves an invalid slot. A torn final commit
is rejected by header validation or the payload CRC. Rewriting or deleting one
slot does not affect another slot.

Legacy v1 headers (`PHYW`, payload length, CRC, 24-byte name) are recognized as
read-only compatibility records. The next successful upload to that slot
replaces it with v2.
