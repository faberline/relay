# Log format break: NDJSON to framed segments (2026-11-11)

What changed: relay's durable log (`relay-storage`) no longer writes
newline-delimited JSON segments (`<subject>__shard<N>__<base>.ndjson`). Each
segment is now a core `storage-durable` framed log file
(`<subject>__shard<N>__<base>.frames`): one CRC-checked frame per entry, with
the entry seq as the frame seq and the entry JSON as the payload.

Old data is not migrated. Relay does not read old `.ndjson` files and does not
delete them. This was accepted by the control desk (relay#129, 2026-11-11).

What an operator sees: if a data directory still holds a `.ndjson` segment for
a subject and shard, the log fails to open with an `InvalidData` error that
starts with `legacy NDJSON log file` and names the file.

To recover, move the old data directory away, or start with an empty one. The
old files are left as they are until the operator removes them.
