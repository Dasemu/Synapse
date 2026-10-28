# Lessons

Recurring mistakes and how to avoid them. Add an entry when a mistake repeats.

- `flutter analyze` crashes ("FormatException: Unexpected end of input" from the analysis
  server) when the project path has non-ASCII characters (`~/Hacking/Darío/...`). Use
  `dart analyze` locally; CI paths are ASCII so `flutter analyze` is fine there.
- Generated FFI types share a namespace with Flutter widgets in Dart: avoid DTO names such as
  `PageView`, `Page`, `Route` in `synapse-ffi`.
- wasmi 2's default tail-call dispatch overflowed the stack in debug test builds (an
  infinite-loop module crashed the process instead of running out of fuel). `synapse-modules`
  uses `portable-dispatch`; keep it, and keep the runaway-module test.
- Fedora's packaged `cargo`/`rust` (in `/usr/bin`) shadows rustup's toolchain in some shells;
  mixing them breaks `target/` with E0514 ("compiled by an incompatible version of rustc").
  Prefix commands with `PATH="$HOME/.cargo/bin:$PATH"` (the Stop hook already does) and check
  `cargo --version` before building.
