import 'dart:async';
import 'dart:collection';

import 'package:flutter/foundation.dart';

/// Runs graph writes one at a time, in submission order. FFI calls are async and run on a
/// Rust thread pool, so without this two quick edits could reach the core out of order.
class WriteQueue {
  WriteQueue({this.onError});

  /// Called with any error a task throws; the task's own future also completes with it.
  void Function(Object error, StackTrace stack)? onError;

  final _tasks = Queue<Future<void> Function()>();
  bool _running = false;
  Completer<void>? _idle;

  bool get isIdle => !_running && _tasks.isEmpty;

  /// Completes when every queued task has run.
  Future<void> get idle {
    if (isIdle) return Future.value();
    return (_idle ??= Completer<void>()).future;
  }

  /// Queues `task`. Reads pass `report: false` so their errors do not reach [onError].
  Future<T> run<T>(Future<T> Function() task, {bool report = true}) {
    final done = Completer<T>();
    _tasks.add(() async {
      try {
        done.complete(await task());
      } catch (e, st) {
        if (report) onError?.call(e, st);
        done.completeError(e, st);
      }
    });
    _pump();
    return done.future;
  }

  Future<void> _pump() async {
    if (_running) return;
    _running = true;
    while (_tasks.isNotEmpty) {
      await _tasks.removeFirst()();
    }
    _running = false;
    final idle = _idle;
    _idle = null;
    idle?.complete();
  }

  @visibleForTesting
  int get pending => _tasks.length;
}
