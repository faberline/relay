# Log on-disk format break (2026-11-11)

## What changed

relay's durable log stores each segment as a core `storage-durable` framed-log
file. A frame holds one JSON entry behind a 16-byte header (seq, length, CRC32).
Segment files are named `<subject>__shard<N>__<base>.frames`.

Before this change, each segment was newline-delimited JSON in a
`.ndjson` file.

## What this means for existing data

- Old data is not migrated. There is no migration tool.
- A data directory that still holds `.ndjson` segments for a subject and
  shard fails to open with an `InvalidData` error that names the legacy file
  (`legacy NDJSON log file ... found`).
- relay does not read or delete the old file.

## What an operator does

1. Stop relay.
2. Move the old data directory out of the way, or point relay at an empty
   data directory. Do not delete it until it is no longer needed.
3. Start relay. The log starts empty for that subject and shard.
