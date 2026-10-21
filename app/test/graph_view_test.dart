import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/graph/graph_model.dart';
import 'package:synapse/src/graph/network_canvas.dart';
import 'package:synapse/src/graph/tree_canvas.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/graph_view.dart';
import 'package:synapse/src/views/timeline_view.dart';

import 'support/memory_backend.dart';

MemoryBackend demo() {
  final b = MemoryBackend();
  final a = b.createPage('Alpha');
  b.seed(a.id, [(0, 'see [[Beta]] and [[proj/one]] #topic')]);
  final beta = b.createPage('Beta');
  b.seed(beta.id, [(0, 'back to [[Alpha]] #topic')]);
  b.createPage('Lonely');
  return b;
}

Future<AppState> pumpGraph(WidgetTester tester, MemoryBackend b) async {
  tester.view.physicalSize = const Size(1600, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final app = AppState(b);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(
        theme: buildTheme(Brightness.light),
        home: const Scaffold(body: GraphView()),
      ),
    ),
  );
  await settle(tester);
  return app;
}

Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 6; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

Future<void> mode(WidgetTester tester, String label) async {
  await tester.tap(find.text(label));
  await settle(tester);
}

NetworkCanvasState canvas(WidgetTester tester) =>
    tester.state<NetworkCanvasState>(find.byType(NetworkCanvas));

void main() {
  testWidgets('network mode shows pages and filters orphans', (tester) async {
    await pumpGraph(tester, demo());
    // Alpha, Beta, proj/one, topic, Lonely.
    expect(canvas(tester).nodeCount, 5);
    expect(find.textContaining('5 pages'), findsOneWidget);
    await tester.tap(find.text('Orphans'));
    await settle(tester);
    expect(canvas(tester).nodeCount, 4);
    await tester.tap(find.text('Orphans'));
    await settle(tester);
    expect(canvas(tester).nodeCount, 5);
  });

  testWidgets('a filter change while the layout runs starts the new layout', (
    tester,
  ) async {
    final b = demo()..layoutSteps = 6;
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      AppScope(
        state: AppState(b),
        child: MaterialApp(
          theme: buildTheme(Brightness.light),
          home: const Scaffold(body: GraphView()),
        ),
      ),
    );
    // Let the first layout start, but not settle.
    await tester.pump(const Duration(milliseconds: 20));
    await tester.pump(const Duration(milliseconds: 20));
    expect(b.layouts, hasLength(1));
    expect(b.layouts.first.steps, lessThan(6));
    await tester.tap(find.text('Orphans'));
    await settle(tester);
    await settle(tester);
    expect(b.layouts, hasLength(2));
    expect(b.layouts.first.disposed, isTrue);
    expect(b.layouts.last.steps, greaterThanOrEqualTo(6));
    expect(canvas(tester).settled, isTrue);
  });

  testWidgets('switches between every mode', (tester) async {
    await pumpGraph(tester, demo());

    await mode(tester, 'Local');
    expect(find.byKey(const ValueKey('depth')), findsOneWidget);
    expect(find.byType(NetworkCanvas), findsOneWidget);

    await mode(tester, 'Synapses');
    expect(find.text('Pulse'), findsOneWidget);
    expect(find.byType(NetworkCanvas), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));

    await mode(tester, 'Clusters');
    expect(find.byType(NetworkCanvas), findsOneWidget);

    await mode(tester, 'Namespaces');
    expect(find.byType(TreeCanvas), findsOneWidget);
    expect(find.byType(NetworkCanvas), findsNothing);

    await mode(tester, 'Timeline');
    expect(find.byType(TimelineView), findsOneWidget);

    await mode(tester, 'Network');
    expect(find.byType(NetworkCanvas), findsOneWidget);
    await tester.pump(const Duration(seconds: 1));
  });

  testWidgets('local mode follows the depth slider', (tester) async {
    final b = demo();
    // Chain Alpha – Beta – Gamma – Delta.
    final g = b.createPage('Gamma');
    b.seed(g.id, [(0, '[[Beta]] [[Delta]]')]);
    await pumpGraph(tester, b);
    await mode(tester, 'Local');
    // Focus is the highest-degree page until the user picks one; pick Delta.
    await tester.enterText(find.byType(TextField), 'Delta');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    expect(
      canvas(tester).nodeCount,
      3,
      reason: 'Delta, Gamma, Beta at depth 2',
    );
    final slider = tester.widget<Slider>(find.byKey(const ValueKey('depth')));
    slider.onChanged!(1);
    await settle(tester);
    expect(canvas(tester).nodeCount, 2);
    slider.onChanged!(3);
    await settle(tester);
    expect(canvas(tester).nodeCount, 5, reason: 'adds Alpha and topic');
  });

  test('namespace tree groups a/b titles and tags', () async {
    final model = GraphModel(await demo().graphData());
    final root = buildNamespaceTree(model);
    final labels = root.children.map((c) => c.label).toList();
    expect(labels, ['Namespaces', 'Tags']);
    expect(root.children[0].children.single.label, 'proj');
    expect(root.children[0].children.single.children.single.label, 'one');
    final topic = root.children[1].children.single;
    expect(topic.label, '#topic');
    expect(topic.children.map((c) => c.label).toSet(), {'Alpha', 'Beta'});
  });

  test('filter by tag, query and min degree', () async {
    final model = GraphModel(await demo().graphData());
    String t(int i) => model.node(i).title;
    final topic = model.indexOf(
      model.data.nodes.firstWhere((n) => n.title == 'topic').id,
    )!;
    expect(GraphFilter(tag: topic).apply(model).map(t).toSet(), {
      'Alpha',
      'Beta',
      'topic',
    });
    expect(const GraphFilter(query: 'alp').apply(model).map(t), ['Alpha']);
    expect(const GraphFilter(minDegree: 3).apply(model).map(t).toSet(), {
      'Alpha',
    });
  });

  test('orphans are judged among the pages that are shown', () async {
    final b = demo();
    final model = GraphModel(await b.graphData());
    String t(int i) => model.node(i).title;
    // Only Alpha matches the query; its neighbours are filtered out, so with orphans
    // hidden nothing is left.
    expect(
      const GraphFilter(query: 'alpha', orphans: false).apply(model).map(t),
      isEmpty,
    );
    expect(
      const GraphFilter(query: 'a', orphans: false).apply(model).map(t).toSet(),
      {'Alpha', 'Beta'},
    );
  });

  test('hull core drops stray members', () {
    final pts = [
      for (var i = 0; i < 10; i++) Offset(i.toDouble(), (i % 3).toDouble()),
      const Offset(500, 500),
    ];
    expect(corePoints(pts), hasLength(10));
  });

  test('convex hull keeps only the outline', () {
    final hull = convexHull(const [
      Offset(0, 0),
      Offset(10, 0),
      Offset(10, 10),
      Offset(0, 10),
      Offset(5, 5),
    ]);
    expect(hull.length, 4);
    expect(hull.contains(const Offset(5, 5)), isFalse);
  });
}
