import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/graph/graph_model.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/graph_view.dart';

import 'support/memory_backend.dart';

/// Like the core: related() only understands page ids.
class StrictBackend extends MemoryBackend {
  final relatedCalls = <String>[];

  @override
  Future<List<RelatedDto>> related(String pageId, {int limit = 12}) async {
    relatedCalls.add(pageId);
    if (pageId.startsWith('tag:')) throw Exception('invalid id $pageId');
    return [];
  }
}

GraphNode node(
  String id,
  String title, {
  bool tag = false,
  int createdAt = 0,
  int degree = 1,
}) => GraphNode(
  id: id,
  title: title,
  createdAt: createdAt,
  isTag: tag,
  degree: degree,
  backlinks: 0,
  cluster: 0,
  isHub: false,
  tags: const [],
);

GraphModel model(List<GraphNode> nodes, List<(int, int, EdgeKind)> edges) =>
    GraphModel(
      GraphData(
        nodes: nodes,
        sources: Uint32List.fromList([for (final e in edges) e.$1]),
        targets: Uint32List.fromList([for (final e in edges) e.$2]),
        kinds: Uint8List.fromList([for (final e in edges) e.$3.index]),
        weights: Float32List.fromList([for (final _ in edges) 1.0]),
        seq: 1,
      ),
    );

int ms(int y, int m, int d) => DateTime(y, m, d).millisecondsSinceEpoch;

void main() {
  group('memory backend matches the core', () {
    test('types are normalised like tags', () async {
      final b = MemoryBackend();
      final p = b.createPage('A');
      final q = await b.setPageType(p.id, '  #  my   type ');
      expect(q.pageType, 'my type');
    });

    test('renameTag counts only the pages that changed', () async {
      final b = MemoryBackend();
      final a = b.createPage('A');
      final c = b.createPage('C');
      await b.setPageTags(a.id, ['idea']);
      await b.setPageTags(c.id, ['Other']);
      expect(await b.renameTag('idea', 'idea'), 0);
      expect(await b.renameTag('idea', 'plan'), 1);
      expect(await b.renameTag('nothing', 'x'), 0);
    });

    test('journals accept an empty tag list', () async {
      final b = MemoryBackend();
      final j = b.createPage('2024-01-02', journalDate: '2024-01-02');
      await b.setPageTags(j.id, []);
      expect(() => b.setPageTags(j.id, ['x']), throwsException);
    });

    test('a hub is an implicit tag of its members, merged once', () async {
      final b = MemoryBackend();
      final media = b.createPage('Media');
      b.makeHub(media.id);
      final a = b.createPage('Naruto');
      final c = b.createPage('Bleach');
      await b.addToHub(a.id, media.id);
      await b.addToHub(c.id, media.id);
      await b.setPageType(c.id, 'media');
      final tmpl = await b.hubTemplate(media.id);
      expect((await b.pagesWithTag('media')).map((p) => p.id).toSet(), {
        a.id,
        c.id,
      });
      expect((await b.pagesWithTag('media')).any((p) => p.id == tmpl), isFalse);
      final cat = await b.tagCatalog();
      expect(cat.single.count, 2);
      expect(b.pages[a.id]!.tags, isEmpty);

      final data = await b.graphData();
      final tags = data.nodes.where((n) => n.isTag).toList();
      expect(tags.length, 1);
      expect(tags.single.backlinks, 2);
      final ti = data.nodes.indexOf(tags.single);
      final from = [
        for (var e = 0; e < data.kinds.length; e++)
          if (data.kinds[e] == EdgeKind.tagged.index && data.targets[e] == ti)
            data.nodes[data.sources[e]].title,
      ]..sort();
      expect(from, ['Bleach', 'Naruto']);
    });
  });

  group('graph model with tag nodes', () {
    test('tags lists only virtual tag nodes, once', () {
      // Page "Idea" (a former tag page) is targeted by an inline #Idea ref and the virtual
      // tag node "Idea" by TAGGED edges.
      final m = model(
        [
          node('0', 'A'),
          node('1', 'B'),
          node('2', 'Idea'),
          node('tag:idea', 'Idea', tag: true),
        ],
        [
          (0, 2, EdgeKind.tag),
          (1, 3, EdgeKind.tagged),
          (0, 3, EdgeKind.tagged),
        ],
      );
      expect(m.tags, [3]);
    });

    test('date filters judge tag nodes by their tagged pages', () {
      final m = model(
        [
          node('0', 'Old', createdAt: ms(2020, 1, 1)),
          node('1', 'New', createdAt: ms(2024, 6, 1)),
          node('tag:old', 'old', tag: true),
          node('tag:new', 'new', tag: true),
        ],
        [(0, 2, EdgeKind.tagged), (1, 3, EdgeKind.tagged)],
      );
      expect(const GraphFilter().apply(m), [0, 1, 2, 3]);
      expect(const GraphFilter(from: '2023-01-01').apply(m), [1, 3]);
      expect(const GraphFilter(to: '2021-01-01').apply(m), [0, 2]);
    });
  });

  group('brain mode and tag nodes', () {
    Future<StrictBackend> pump(WidgetTester tester) async {
      final b = StrictBackend();
      final hub = b.createPage('Hub');
      b.seed(hub.id, [(0, 'x')]);
      for (final t in ['A', 'B', 'C']) {
        final p = b.createPage(t);
        await b.setPageTags(p.id, ['shared']);
      }
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
      for (var i = 0; i < 6; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      await tester.tap(find.text('Synapses'));
      for (var i = 0; i < 6; i++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      return b;
    }

    testWidgets('the fallback focus is never a tag node', (tester) async {
      final b = await pump(tester);
      expect(b.relatedCalls, isNotEmpty);
      expect(b.relatedCalls.any((c) => c.startsWith('tag:')), isFalse);
      expect(tester.takeException(), isNull);
    });
  });
}
