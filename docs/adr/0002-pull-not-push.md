# ADR 0002 — Pull, not push

## Status

Accepted.

## Context

faberline has three message services. A task queue can either let workers pull
work, or push work to an HTTP target on a schedule. A topic log can keep
entries for replay by many readers.

## Decision

- relay is the pull side: workers lease entries, ack them, and relay deletes
  them on ack. Concurrency is driven by the number of workers.
- defer is the push side: scheduled HTTP dispatch, long-horizon ETA, and rate
  limits.
- tape owns broadcast and durable replay.
- relay and defer share only the priority byte: `0` lowest, `255` highest,
  default `10`.

## Consequences

- relay has no server-side rate governor and no HTTP target executor.
- relay keeps storage proportional to the backlog, not to total throughput.
- Callers that need replay or fan-out use tape, not relay.
