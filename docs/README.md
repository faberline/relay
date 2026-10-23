# relay docs

Project-level documentation for relay. Usage starts at the root
[`README.md`](../README.md), and contributor rules live in
[`CONTRIBUTING.md`](../CONTRIBUTING.md).

| Document | What it answers |
|----------|-----------------|
| [architecture.md](architecture.md) | How the source is cut into contexts and what may depend on what. |
| [glossary.md](glossary.md) | The terms relay's code, API and docs use. |
| [domain/](domain/README.md) | What each part of the model owns. |
| [operations/](operations/README.md) | How to run, configure, and test relay. |
| [adr/](adr/) | Architecture decisions and the reasons behind them. |

## Architecture decisions

| ADR | Decision |
|-----|----------|
| [0001](adr/0001-single-crate-layered-modules.md) | Superseded by 0003. One crate on faberline/core; each context was one top-level module. |
| [0002](adr/0002-pull-not-push.md) | relay owns worker-leased pull; defer owns scheduled push and tape owns broadcast replay. |
| [0003](adr/0003-multi-crate-workspace.md) | Virtual workspace on faberline/core; one `relay-<context>` crate per context, checked by `ddd.toml`. |
