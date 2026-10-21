// Runs the real app (Rust core over FFI) on a graph and exercises every graph mode.
// flutter test integration_test/graph_smoke_test.dart -d linux \
//   --dart-define=GRAPH=/path/to/copy/of/graph [--dart-define=SHOTS=/dir]
// Use a copy: opening a graph may write to it (today's journal).
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:synapse/src/app.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/graph/network_canvas.dart';
import 'package:synapse/src/rust/frb_generated.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/views/shell.dart';

const graph = String.fromEnvironment('GRAPH');
const shots = String.fromEnvironment('SHOTS');

Future<void> wait(WidgetTester tester, Duration d) async {
  final end = DateTime.now().add(d);
  while (DateTime.now().isBefore(end)) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

Future<void> shot(WidgetTester tester, String name) async {
  if (shots.isEmpty) return;
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find
        .ancestor(
          of: find.byType(Shell),
          matching: find.byType(RepaintBoundary),
        )
        .first,
  );
  final image = await boundary.toImage();
  final png = await image.toByteData(format: ui.ImageByteFormat.png);
  File('$shots/$name.png').writeAsBytesSync(png!.buffer.asUint8List());
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('graph modes on a real graph', (tester) async {
    if (graph.isEmpty) {
      markTestSkipped('pass --dart-define=GRAPH=<dir>');
      return;
    }
    await RustLib.init();
    final backend = await FfiGraphBackend.open(graph);
    await tester.pumpWidget(SynapseApp(backend: backend));
    await wait(tester, const Duration(seconds: 1));
    final app = AppScope.read(tester.element(find.byType(Shell)));

    final timings = <FrameTiming>[];
    void collect(List<FrameTiming> t) => timings.addAll(t);
    SchedulerBinding.instance.addTimingsCallback(collect);
    final watch = Stopwatch()..start();
    app.go(const GraphRoute(GraphMode.network));
    // Wait for the Rust layout to settle.
    while (watch.elapsed < const Duration(seconds: 20)) {
      await wait(tester, const Duration(milliseconds: 200));
      final c = find.byType(NetworkCanvas);
      if (c.evaluate().isNotEmpty &&
          tester.state<NetworkCanvasState>(c).settled) {
        break;
      }
    }
    final canvas = tester.state<NetworkCanvasState>(find.byType(NetworkCanvas));
    expect(canvas.settled, isTrue, reason: 'layout settles');
    expect(canvas.nodeCount, greaterThan(0));
    debugPrint(
      'network: ${canvas.nodeCount} nodes settled in ${watch.elapsed}',
    );
    // Pan for a second to measure steady-state frames.
    timings.clear();
    final centre = tester.getCenter(find.byType(NetworkCanvas));
    final gesture = await tester.startGesture(centre + const Offset(0, 200));
    for (var i = 0; i < 60; i++) {
      await gesture.moveBy(const Offset(3, 1));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await gesture.up();
    await wait(tester, const Duration(milliseconds: 300));
    SchedulerBinding.instance.removeTimingsCallback(collect);
    if (timings.isNotEmpty) {
      int us(Duration d) => d.inMicroseconds;
      final build = timings.map((t) => us(t.buildDuration)).toList()..sort();
      final raster = timings.map((t) => us(t.rasterDuration)).toList()..sort();
      String p(List<int> l, double q) =>
          '${(l[((l.length - 1) * q).round()] / 1000).toStringAsFixed(1)} ms';
      debugPrint(
        'frames ${timings.length}: build p50 ${p(build, .5)} p90 ${p(build, .9)}, '
        'raster p50 ${p(raster, .5)} p90 ${p(raster, .9)}',
      );
    }
    await shot(tester, 'app-network');

    for (final mode in [
      'Local',
      'Synapses',
      'Clusters',
      'Namespaces',
      'Timeline',
    ]) {
      await tester.tap(find.text(mode).last);
      await wait(tester, const Duration(seconds: 3));
      expect(tester.takeException(), isNull);
      await shot(tester, 'app-${mode.toLowerCase()}');
    }

    // Page view with the local graph panel.
    app.openTitle(app.recent.isEmpty ? 'contents' : app.recent.first.title);
    await wait(tester, const Duration(seconds: 3));
    await shot(tester, 'app-page');
  });
}
