# Synapse

A privacy-first, personal knowledge management platform.

Synapse is a personal fork of [Logseq](https://github.com/logseq/logseq) (DB-graph version),
maintained by Darío Sevilla (dario@dariosevilla.es). It keeps Logseq's outliner, DB-graph
model, and plugin API, reworked around a self-hosted server-client architecture: a single
daemon (`db-worker-node`) holds the authoritative graph and serves thin web, desktop, and
mobile clients over HTTP/WebSocket, with no cloud sync/E2EE layer.

This is not affiliated with, endorsed by, or published on behalf of the Logseq project or
Logseq Inc. All credit for the original design and the vast majority of the codebase goes to
the [Logseq](https://logseq.com) team and its contributors.

## Development

See [docs/develop-logseq.md](docs/develop-logseq.md) (macOS/Linux) and
[docs/develop-logseq-on-windows.md](docs/develop-logseq-on-windows.md) (Windows) for setting
up a development environment. The `docs/` folder also has guides for mobile development,
translations, and the CLI.

## License

AGPL-3.0, inherited from upstream Logseq. See [LICENSE.md](LICENSE.md).
