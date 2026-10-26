// Frame timings of common interactions on a real graph (Rust core over FFI).
// Run in profile mode on a COPY of a graph (opening it may write today's journal):
// flutter drive --profile -d linux --driver=test_driver/integration_test.dart \
//   --target=integration_test/perf_test.dart --dart-define=GRAPH=/path/to/copy
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:synapse/src/app.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/rust/frb_generated.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/views/quick_switcher.dart';
import 'package:synapse/src/views/shell.dart';

const graph = String.fromEnvironment('GRAPH');

/// Lines and errors, returned to the driver (`build/integration_response_data.json`):
/// `flutter drive` does not show the app's prints.
final report = <String>[];

Future<void> wait(WidgetTester tester, Duration d) async {
  final end = DateTime.now().add(d);
  while (DateTime.now().isBefore(end)) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

/// Runs [action], keeps pumping for [after] and prints wall time plus frame stats.
Future<void> measure(
  WidgetTester tester,
  String name,
  Future<void> Function() action, {
  Duration after = const Duration(milliseconds: 800),
}) async {
  final timings = <FrameTiming>[];
  void collect(List<FrameTiming> t) => timings.addAll(t);
  SchedulerBinding.instance.addTimingsCallback(collect);
  final watch = Stopwatch()..start();
  await action();
  final acted = watch.elapsedMilliseconds;
  await wait(tester, after);
  SchedulerBinding.instance.removeTimingsCallback(collect);
  int us(Duration d) => d.inMicroseconds;
  final build = timings.map((t) => us(t.buildDuration)).toList()..sort();
  final raster = timings.map((t) => us(t.rasterDuration)).toList()..sort();
  String p(List<int> l, double q) => l.isEmpty
      ? '-'
      : (l[((l.length - 1) * q).round()] / 1000).toStringAsFixed(1);
  final slow = timings
      .where(
        (t) =>
            t.buildDuration + t.rasterDuration >
            const Duration(milliseconds: 16),
      )
      .length;
  report.add(
    'PERF ${name.padRight(26)} action ${acted.toString().padLeft(5)} ms | '
    'frames ${timings.length.toString().padLeft(3)} slow ${slow.toString().padLeft(3)} | '
    'build p50 ${p(build, .5)} p90 ${p(build, .9)} max ${p(build, 1)} | '
    'raster p50 ${p(raster, .5)} p90 ${p(raster, .9)} max ${p(raster, 1)} | '
    'rss ${ProcessInfo.currentRss ~/ (1 << 20)} MB',
  );
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmarkLive;

  testWidgets('interaction timings on a real graph', (tester) async {
    if (graph.isEmpty) {
      markTestSkipped('pass --dart-define=GRAPH=<dir>');
      return;
    }
    final errors = <String>[];
    final previous = FlutterError.onError;
    FlutterError.onError = (d) => errors.add(d.toString());
    try {
      await _run(tester);
    } catch (e, st) {
      errors.add('$e\n$st');
    } finally {
      FlutterError.onError = previous;
      binding.reportData = {'perf': report, 'errors': errors};
    }
  });
}

Future<void> _run(WidgetTester tester) async {
  {
    await RustLib.init();
    final backend = await FfiGraphBackend.open(graph);
    late AppState app;
    await measure(tester, 'startup', () async {
      await tester.pumpWidget(SynapseApp(backend: backend));
      while (find.byType(Shell).evaluate().isEmpty) {
        await tester.pump(const Duration(milliseconds: 16));
      }
      app = AppScope.read(tester.element(find.byType(Shell)));
    }, after: const Duration(seconds: 2));

    final scroll = find.byType(Scrollable).first;
    await measure(tester, 'journals scroll', () async {
      for (var i = 0; i < 40; i++) {
        await tester.drag(scroll, const Offset(0, -120), warnIfMissed: false);
        await tester.pump(const Duration(milliseconds: 16));
      }
    });

    final pages = await backend.listPages(
      includeJournals: false,
      limit: 1000,
      offset: 0,
    );
    final hub = pages.firstWhere((p) => p.isHub);
    final notes = pages.where((p) => !p.isHub).take(8).toList();
    await measure(tester, 'open hub', () async {
      app.go(PageTarget(id: hub.id));
      await tester.pump();
    });
    final hubs = pages.where((p) => p.isHub).toList();
    await measure(tester, 'open all hubs', () async {
      for (final h in hubs) {
        app.go(PageTarget(id: h.id));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 100));
      }
    });
    await measure(tester, 'open first hub again', () async {
      app.go(PageTarget(id: hub.id));
      await tester.pump();
    });
    await measure(tester, 'open 8 notes', () async {
      for (final n in notes) {
        app.go(PageTarget(id: n.id));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 16));
      }
    });

    final field = find.byType(TextField);
    if (field.evaluate().isNotEmpty) {
      await tester.tap(field.first);
      await tester.pump();
      final c = tester.widget<TextField>(field.first).controller!;
      final base = c.text;
      await measure(tester, 'type 40 chars', () async {
        for (var i = 1; i <= 40; i++) {
          final t = '$base ${'x' * i}';
          tester.testTextInput.updateEditingValue(
            TextEditingValue(
              text: t,
              selection: TextSelection.collapsed(offset: t.length),
            ),
          );
          await tester.pump(const Duration(milliseconds: 60));
        }
      }, after: const Duration(seconds: 2));
      tester.testTextInput.updateEditingValue(
        TextEditingValue(
          text: base,
          selection: TextSelection.collapsed(offset: base.length),
        ),
      );
      await wait(tester, const Duration(seconds: 1));
      FocusManager.instance.primaryFocus?.unfocus();
    }

    await measure(tester, 'back x5', () async {
      for (var i = 0; i < 5; i++) {
        app.back();
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 16));
      }
    });
    await measure(tester, 'all pages', () async {
      app.go(const AllPagesRoute());
      await tester.pump();
    });
    await measure(tester, 'all pages scroll', () async {
      for (var i = 0; i < 30; i++) {
        await tester.drag(
          find.byType(Scrollable).last,
          const Offset(0, -150),
          warnIfMissed: false,
        );
        await tester.pump(const Duration(milliseconds: 16));
      }
    });
    await measure(tester, 'quick switcher typing', () async {
      showQuickSwitcher(tester.element(find.byType(Shell)));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      var q = '';
      for (final ch in 'proyecto'.split('')) {
        q += ch;
        await tester.enterText(find.byType(TextField).last, q);
        await tester.pump(const Duration(milliseconds: 80));
      }
    });
    Navigator.of(tester.element(find.byType(TextField).last)).pop();
    await wait(tester, const Duration(milliseconds: 300));
    await measure(tester, 'new tab + switch x6', () async {
      app.newTab();
      await tester.pump();
      for (var i = 0; i < 6; i++) {
        app.nextTab();
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 16));
      }
    });
    await measure(tester, 'graph tree', () async {
      app.go(const GraphRoute(GraphMode.tree));
      await tester.pump();
    }, after: const Duration(seconds: 4));
    await measure(tester, 'graph network', () async {
      app.go(const GraphRoute(GraphMode.network));
      await tester.pump();
    }, after: const Duration(seconds: 6));
    await measure(
      tester,
      'network idle',
      () async {},
      after: const Duration(seconds: 2),
    );
    await measure(tester, 'journals after graph', () async {
      app.go(const JournalsRoute());
      await tester.pump();
    }, after: const Duration(seconds: 2));
    await measure(
      tester,
      'journals idle',
      () async {},
      after: const Duration(seconds: 2),
    );
  }
}
