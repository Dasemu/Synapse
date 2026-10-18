# CLAUDE.md

## Overview
Synapse is a personal fork of Logseq (DB-graph version): a self-hosted outliner/PKM with a
single daemon (`db-worker-node`) holding the authoritative graph, served to thin web,
Electron desktop, and Capacitor mobile (iOS/Android) clients over HTTP/WebSocket. Core app is
ClojureScript (shadow-cljs); supporting tools (CLI, scripts, libs) are Node/TypeScript, plus
an OCaml CLI companion and a Python embedding sidecar.

## Commands
- Lint + unit tests: `bb dev:lint-and-test`
- Single test: `bb dev:test -v <namespace/testcase-name>`
- ClojureScript lint only: `pnpm cljs:lint` (clj-kondo)
- Unit tests only: `pnpm test` (compiles then runs cljs tests)
- App E2E: from `clj-e2e/`, `bb test`
- CLI E2E: `bb -f cli-e2e/bb.edn test --skip-build`
- Dev build (watch): `pnpm dev` / `pnpm watch` (web) / `pnpm electron-watch` / `pnpm mobile-watch`
- Release build: `pnpm release`, `pnpm release-electron`, `pnpm release-mobile`

## Architecture
See `AGENTS.md` for detailed engineering conventions and
`docs/agent-guide/implemented/architecture/2026-08-24-logseq-runtime-and-engineering-guide.md`
for runtime/sync/CLI/rendering/search architecture. Workspace packages: `deps/*` (common,
db, db-sync, outliner, graph-parser, publish(ing), shui), `cli/` (OCaml CLI), `libs/`
(published JS/TS SDK), `scripts/`, `sidecar/` (Python embedding server).

## Conventions
Full conventions live in `AGENTS.md` (coding style, i18n, testing, commit/PR rules) — read it
first; it takes precedence over this file for code changes. Directory-specific `AGENTS.md`
files override the root one.

## Workflow quirks
- Repo-local skills live under `.agents/skills/`; load the matching `SKILL.md` before editing
  (i18n changes mandatorily require `.agents/skills/logseq-i18n/SKILL.md`).
- No `development` branch here: work on a task branch off `main`, PR into `main`.
- No local formatter is wired for the main ClojureScript app; `libs/` uses Prettier for its
  own TS code.

## Deployment
Not a typical always-on web service: ships as Electron desktop, Capacitor mobile apps, and a
self-hosted `db-worker-node` daemon. See `deploy/logseq-daemon.service` (systemd unit) and
`deploy/Caddyfile` (reverse proxy) for the daemon-side deployment.

## Gotchas
- `cli/` (OCaml/dune CLI) is not built in CI yet (`LOGSEQ_SKIP_CLI_BUNDLE=1` in
  `.gitea/workflows/build.yml`); don't assume CLI release steps run there.
- This repo uses Gitea (git.zenvairo.es), not GitHub: CI workflows live in
  `.gitea/workflows/`, matching the existing `build.yml`.
