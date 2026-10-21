# Lessons

Recurring mistakes and how to avoid them. Add an entry when a mistake repeats.

- `flutter analyze` crashes ("FormatException: Unexpected end of input" from the analysis
  server) when the project path has non-ASCII characters (`~/Hacking/Darío/...`). Use
  `dart analyze` locally; CI paths are ASCII so `flutter analyze` is fine there.
- Generated FFI types share a namespace with Flutter widgets in Dart: avoid DTO names such as
  `PageView`, `Page`, `Route` in `synapse-ffi`.
