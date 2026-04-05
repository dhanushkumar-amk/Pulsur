---
title: WAL Format
description: Byte-level description of the queue write-ahead log and recovery flow.
---

# WAL Format

The queue crate persists events with an append-only write-ahead log plus periodic snapshots.

## Files on disk

- `wal.log` for the active segment
- `wal.001.log`, `wal.002.log`, and so on for rotated segments
- `snapshot.bin` for the compacted queue snapshot

## Record format

Each WAL record is length-prefixed.

Byte layout:

```text
+-------------------+-----------------------------+
| 4 bytes           | N bytes                     |
+-------------------+-----------------------------+
| u32 little-endian | bincode(WalEvent) payload   |
+-------------------+-----------------------------+
```

The queue writes records in this order:

1. serialize `WalEvent` with `bincode`
2. convert payload length to little-endian `u32`
3. append the 4-byte length
4. append the serialized payload
5. `sync_all()` the file

## Event types

Current durable event variants:

- `Enqueue(Job)`
- `Dequeue(Uuid)`
- `Ack(Uuid)`
- `Nack(Uuid, String)`

## Recovery model

On startup:

1. read `snapshot.bin` if it exists
2. rebuild queue state from the snapshot
3. iterate all WAL segments in sorted order
4. replay each length-prefixed record into the in-memory queue
5. ignore trailing partial records caused by interrupted writes

That last behavior is important. The replay loop stops cleanly on `UnexpectedEof`, which means a torn final write does not poison the entire queue.

## Rotation and compaction

When the active WAL exceeds the configured max size:

1. rename `wal.log` to the next rotated segment
2. write a fresh `snapshot.bin`
3. delete old rotated segments
4. create a new empty `wal.log`

## Why this format works well here

Benefits:

- appends are simple and durable
- crash recovery is deterministic
- snapshots prevent unbounded replay times
- length-prefixing makes replay parsing cheap

Tradeoff:

- `bincode` is compact, but not designed as a public cross-language format
- WAL evolution needs versioning discipline if the `WalEvent` schema changes
