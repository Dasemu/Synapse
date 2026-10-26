import 'package:flutter/material.dart';

import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
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

    await mode(tester, 'Synapses');
    expect(find.text('Pulse'), findsOneWidget);
    expect(find.byType(NetworkCanvas), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));

    await mode(tester, 'Clusters');
    expect(find.byType(NetworkCanvas), findsOneWidget);

    await mode(tester, 'Tree');
    expect(find.byType(TreeCanvas), findsOneWidget);
    expect(find.byType(NetworkCanvas), findsNothing);

    await mode(tester, 'Timeline');
    expect(find.byType(TimelineView), findsOneWidget);

    await mode(tester, 'Network');
    expect(find.byType(NetworkCanvas), findsOneWidget);
    await tester.pump(const Duration(seconds: 1));
  });

  test('tree groups a/b titles and tags', () async {
    final model = GraphModel(await demo().graphData());
    final root = buildNamespaceTree(model);
    TreeItem branch(String l) => root.children.firstWhere((c) => c.label == l);
    // Hubs are covered by the hubs group below; notes in no hub hang off the centre.
    // Namespaces have no node of their own: `proj` hangs off the centre.
    expect(root.children.map((c) => c.label), [
      'Alpha',
      'Beta',
      'Lonely',
      'proj',
      'Tags',
      'topic',
    ]);
    expect(branch('proj').children.single.label, 'one');
    final topic = branch('Tags').children.single;
    expect(topic.label, '#topic');
    expect(topic.children.map((c) => c.label).toSet(), {'Alpha', 'Beta'});
  });

  test(
    'tree centre carries the store name; sibling branches get distinct hues',
    () async {
      final model = GraphModel(await demo().graphData());
      final root = buildNamespaceTree(model, name: 'Mi almacén');
      expect(root.label, 'Mi almacén');
      final arms = root.children;
      expect(arms.length, greaterThan(1));
      // Arms are evenly spread over the hue circle.
      final gap = arms[1].hue - arms[0].hue;
      expect(gap, closeTo(360 / arms.length, 1e-9));
      // Children split their parent's sector evenly and stay inside it.
      for (final a in arms) {
        final k = a.children.length;
        for (var j = 1; j < k; j++) {
          expect(
            a.children[j].hue - a.children[j - 1].hue,
            closeTo(a.span / k, 1e-9),
          );
        }
        for (final c in a.children) {
          expect((c.hue - a.hue).abs(), lessThanOrEqualTo(a.span / 2));
        }
      }
    },
  );

  test('tag grouping uses each page\'s most used tag', () {
    GraphNode n(String t, List<String> tags) => GraphNode(
      id: t,
      title: t,
      createdAt: 0,
      isTag: false,
      degree: 1,
      backlinks: 0,
      cluster: 0,
      isHub: false,
      tags: tags,
    );
    final m = GraphModel(
      GraphData(
        nodes: [
          n('a', ['Tecnología', 'idea']),
          n('b', ['tecnología']),
          n('c', ['idea', 'lugar']),
          n('d', []),
        ],
        sources: Uint32List(0),
        targets: Uint32List(0),
        kinds: Uint8List(0),
        weights: Float32List(0),
        seq: 0,
      ),
    );
    // Tie (two pages each): alphabetical.
    expect(m.homeTag(0), 'idea');
    expect(m.homeTag(2), 'idea');
    expect(m.homeTag(3), isNull);
    expect(m.tagGroups, ['idea', 'tecnología']);
    expect(m.tagLabel('tecnología'), 'Tecnología');
    expect(m.tagSprings([0, 1, 2, 3]).map((s) => (s.$1, s.$2)), [(2, 0)]);
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

  group('virtual tag nodes', () {
    // Two pages tagged "ocio" through TAGGED edges (kind 8) to one tag node.
    GraphData tagData() {
      GraphNode n(String id, String title, {bool tag = false}) => GraphNode(
        id: id,
        title: title,
        createdAt: 0,
        isTag: tag,
        degree: 1,
        backlinks: 0,
        cluster: 0,
        isHub: false,
        tags: const [],
      );
      final nodes = [
        n('0', 'Games'),
        n('1', 'Chess'),
        n('tag:ocio', 'ocio', tag: true),
        n('tag:a/b', 'a/b', tag: true),
      ];
      final edges = [(0, 2, 8), (1, 2, 8)];
      return GraphData(
        nodes: nodes,
        sources: Uint32List.fromList([for (final e in edges) e.$1]),
        targets: Uint32List.fromList([for (final e in edges) e.$2]),
        kinds: Uint8List.fromList([for (final e in edges) e.$3]),
        weights: Float32List.fromList([for (final _ in edges) 1.0]),
        seq: 1,
      );
    }

    test('the Tags branch is built from tag nodes, not loose notes', () {
      final m = GraphModel(tagData());
      expect(m.isTag(2), isTrue);
      expect(m.tags, [2]);
      final root = buildNamespaceTree(m);
      final tags = root.children.firstWhere((c) => c.label == 'Tags');
      final ocio = tags.children.single;
      expect(ocio.label, '#ocio');
      expect(ocio.global, 2);
      expect(ocio.children.map((c) => c.label).toSet(), {'Games', 'Chess'});
      // Tag nodes are neither loose notes nor namespaces.
      expect(
        root.children.where((c) => c.label == 'ocio' || c.label == 'a'),
        isEmpty,
      );
    });

    test('filtering by a tag node keeps the tagged pages', () {
      final m = GraphModel(tagData());
      expect(const GraphFilter().copyWith(tag: () => 2).apply(m), [0, 1, 2]);
    });

    testWidgets('a tag in the tree opens its tag view by name', (tester) async {
      String? opened;
      final m = GraphModel(tagData());
      tester.view.physicalSize = const Size(1600, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        AppScope(
          state: AppState(MemoryBackend()),
          child: MaterialApp(
            theme: buildTheme(Brightness.light),
            home: Scaffold(
              body: TreeCanvas(
                model: m,
                onTap: (_) {},
                onTag: (name) => opened = name,
              ),
            ),
          ),
        ),
      );
      final state = tester.state<TreeCanvasState>(find.byType(TreeCanvas));
      final tags = buildNamespaceTree(m).children
          .firstWhere((c) => c.label == 'Tags');
      state.openItem(tags.children.single);
      expect(opened, 'ocio');
    });
  });

  group('hubs', () {
    // Media (hub) -> TV (hub) -> Anime (page); Media -> Books; Notes (page) in Media;
    // a TV -> Media CONTAINS edge (a cycle the tree must survive); a journal (never a member).
    GraphData hubData() {
      GraphNode n(
        String id,
        String title, {
        bool hub = false,
        int? home,
        String? journal,
      }) => GraphNode(
        id: id,
        title: title,
        journalDate: journal,
        createdAt: 0,
        isTag: false,
        degree: 1,
        backlinks: 0,
        cluster: 0,
        isHub: hub,
        homeHub: home,
        tags: const [],
      );
      final nodes = [
        n('0', 'Media', hub: true, home: 0),
        n('1', 'TV', hub: true, home: 1),
        n('2', 'Anime', home: 1),
        n('3', 'Books', home: 0),
        n('4', 'Notes', home: 0),
        n('5', '2024-01-01', journal: '2024-01-01'),
      ];
      // (source, target, kind)
      final edges = [
        (0, 1, 7), // Media contains TV
        (0, 3, 7), // Media contains Books
        (0, 4, 7), // Media contains Notes
        (1, 2, 7), // TV contains Anime
        (1, 0, 7), // TV contains Media (cycle)
        (0, 5, 0), // Media -> journal link: a link never makes a member
      ];
      return GraphData(
        nodes: nodes,
        sources: Uint32List.fromList([for (final e in edges) e.$1]),
        targets: Uint32List.fromList([for (final e in edges) e.$2]),
        kinds: Uint8List.fromList([for (final e in edges) e.$3]),
        weights: Float32List.fromList([for (final _ in edges) 1.0]),
        seq: 1,
      );
    }

    GraphModel hubModel() => GraphModel(hubData());

    test('hub members are the CONTAINS targets', () {
      final m = hubModel();
      expect(EdgeKind.values[7], EdgeKind.contains);
      expect(EdgeKind.contains.structural, isTrue);
      expect(EdgeKind.values[8], EdgeKind.tagged);
      expect(const GraphFilter().kinds, contains(EdgeKind.tagged));
      expect(m.hubMembers[0], {1, 3, 4});
      expect(m.hubMembers[1], {2, 0});
      expect(m.hubOrder.first, 0, reason: 'Media is home hub of most pages');
      expect(m.hubRank[1], 1);
    });

    test('contains is a default filter kind', () {
      expect(const GraphFilter().kinds, contains(EdgeKind.contains));
    });

    test('hub springs pull members to their home hub', () {
      final m = hubModel();
      expect(m.hubSprings([0, 1, 2, 3, 4, 5]), [
        (2, 1, 0.6),
        (3, 0, 0.6),
        (4, 0, 0.6),
      ]);
    });

    test('tree hubs hang off the centre, cycle-safe', () {
      final root = buildNamespaceTree(hubModel());
      // Media is a member of TV and TV of Media: no root hub, both still placed once.
      expect(root.children.where((c) => c.label == 'Hubs'), isEmpty);
      final media = root.children.firstWhere((c) => c.label == 'Media');
      expect(media.isHub, isTrue);
      expect(media.children.map((c) => c.label).toSet(), {
        'TV',
        'Books',
        'Notes',
      });
      final tv = media.children.firstWhere((c) => c.label == 'TV');
      expect(tv.isHub, isTrue);
      // Media already on the path: a leaf, not an endless loop.
      expect(tv.children.map((c) => c.label).toSet(), {'Anime', 'Media'});
      expect(
        tv.children.firstWhere((c) => c.label == 'Media').children,
        isEmpty,
      );
    });

    testWidgets('clusters mode groups by hub by default', (tester) async {
      await pumpGraph(tester, _HubBackend(hubData()));
      await mode(tester, 'Clusters');
      expect(find.byKey(const ValueKey('cluster-group')), findsOneWidget);
      expect(find.text('Group: hubs'), findsOneWidget);
    });
  });

  group('interactive tree', () {
    // One hub with 200 pages: far more than the tree used to show ("… N more").
    GraphData bigHub() {
      GraphNode n(int i, {bool hub = false}) => GraphNode(
        id: '$i',
        title: hub ? 'Library' : 'Page $i',
        createdAt: 0,
        isTag: false,
        degree: 1,
        backlinks: 0,
        cluster: 0,
        isHub: hub,
        homeHub: 0,
        tags: const [],
      );
      const count = 200;
      return GraphData(
        nodes: [n(0, hub: true), for (var i = 1; i <= count; i++) n(i)],
        sources: Uint32List.fromList([for (var i = 1; i <= count; i++) 0]),
        targets: Uint32List.fromList([for (var i = 1; i <= count; i++) i]),
        kinds: Uint8List.fromList([for (var i = 1; i <= count; i++) 7]),
        weights: Float32List.fromList([for (var i = 1; i <= count; i++) 1]),
        seq: 1,
      );
    }

    test('nothing is capped; big branches start collapsed', () {
      final root = buildNamespaceTree(GraphModel(bigHub()));
      final hub = root.children.single;
      expect(hub.children, hasLength(200));
      expect(hub.children.every((c) => c.global != null), isTrue);
      expandInitial(root);
      expect(root.expanded, isTrue);
      expect(hub.expanded, isFalse);
    });

    test('small branches open even when the centre is crowded', () {
      final root = TreeItem('Graph');
      for (var i = 0; i < 300; i++) {
        root.children.add(TreeItem('Note $i', global: i));
      }
      final ns = TreeItem('Anthropic')..children.add(TreeItem('Claude'));
      root.children.add(ns);
      expandInitial(root);
      expect(ns.expanded, isTrue);
    });

    testWidgets('branches expand and collapse in the canvas', (tester) async {
      await pumpGraph(tester, _HubBackend(bigHub()));
      await mode(tester, 'Tree');
      final state = tester.state<TreeCanvasState>(find.byType(TreeCanvas));
      expect(state.visibleCount, 2);
      expect(find.text('2 of 202'), findsOneWidget);
      state.toggle(state.root.children.single);
      await settle(tester);
      expect(state.visibleCount, 202);
      await tester.tap(find.byKey(const ValueKey('tree-collapse-all')));
      await settle(tester);
      expect(state.visibleCount, 2);
      await tester.tap(find.byKey(const ValueKey('tree-expand-all')));
      await settle(tester);
      expect(find.text('202 of 202'), findsOneWidget);
    });
  });
}

/// A memory backend whose graph data is fixed (so tests don't depend on its demo graph).
class _HubBackend extends MemoryBackend {
  _HubBackend(this._data);
  final GraphData _data;

  @override
  Future<GraphData> graphData() async => _data;
}
