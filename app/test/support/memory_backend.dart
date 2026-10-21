import 'dart:math' as math;
import 'dart:typed_data';

import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/graph/graph_model.dart';
import 'package:synapse/src/state/outline_ops.dart';

/// In-memory [GraphBackend] for widget tests. Outline semantics reuse [OutlineOps], which
/// mirror the Rust FFI (the Rust side has its own tests).
class MemoryBackend implements GraphBackend {
  MemoryBackend({this.todayDate = '2026-10-21'});

  final String todayDate;
  final pages = <String, PageDto>{};
  final outlines = <String, List<OutlineNode>>{};
  final aliases = <String, String>{};
  final calls = <String>[];
  int _seq = 0;

  String _id() => 'id-${_seq++}';

  PageDto createPage(String title, {String? journalDate}) {
    final p = PageDto(
      id: journalDate != null ? 'j-$journalDate' : _id(),
      title: title,
      journalDate: journalDate,
      createdAt: 0,
      updatedAt: 0,
    );
    pages[p.id] = p;
    outlines[p.id] = [];
    return p;
  }

  /// Adds blocks `(depth, content)` to a page.
  List<String> seed(String pageId, List<(int, String)> rows) {
    final l = outlines[pageId]!;
    final ids = <String>[];
    for (final (d, c) in rows) {
      final id = _id();
      l.add(OutlineNode(id: id, depth: d, content: c));
      ids.add(id);
    }
    OutlineOps.fixParents(l);
    return ids;
  }

  List<BlockDto> _dto(String pageId) {
    final l = outlines[pageId]!;
    return [
      for (var i = 0; i < l.length; i++)
        BlockDto(
          id: l[i].id,
          parentId: l[i].parentId,
          depth: l[i].depth,
          content: l[i].content,
          collapsed: l[i].collapsed,
          hasChildren: OutlineOps.hasChildren(l, i),
        ),
    ];
  }

  (String, int) _find(String blockId) {
    for (final e in outlines.entries) {
      final i = e.value.indexWhere((n) => n.id == blockId);
      if (i >= 0) return (e.key, i);
    }
    throw StateError('block $blockId not found');
  }

  PageData _view(PageDto p) => PageData(
    page: p,
    blocks: _dto(p.id),
    aliases: [
      for (final e in aliases.entries)
        if (e.value == p.id) e.key,
    ],
  );

  PageDto? _byTitle(String title) {
    final k = title.trim().toLowerCase();
    for (final p in pages.values) {
      if (p.title.toLowerCase() == k) return p;
    }
    final a = aliases[k];
    return a == null ? null : pages[a];
  }

  @override
  String today() => todayDate;

  @override
  Future<List<PageData>> journals({String? before, required int limit}) async {
    if (before == null) await journal(todayDate);
    final js =
        pages.values
            .where(
              (p) =>
                  p.journalDate != null &&
                  (before == null || p.journalDate!.compareTo(before) < 0),
            )
            .toList()
          ..sort((a, b) => b.journalDate!.compareTo(a.journalDate!));
    return js.take(limit).map(_view).toList();
  }

  @override
  Future<PageData> journal(String date) async =>
      _view(pages['j-$date'] ?? createPage(date, journalDate: date));

  @override
  Future<PageData?> page(String id) async {
    final p = pages[id];
    return p == null ? null : _view(p);
  }

  @override
  Future<PageData?> pageByTitle(String title) async {
    final p = _byTitle(title);
    return p == null ? null : _view(p);
  }

  @override
  Future<PageData> openPage(String title) async =>
      _view(_byTitle(title) ?? createPage(title));

  @override
  Future<List<PageDto>> listPages({
    bool includeJournals = false,
    required int limit,
    int offset = 0,
  }) async => pages.values
      .where((p) => includeJournals || p.journalDate == null)
      .skip(offset)
      .take(limit)
      .toList();

  @override
  Future<PageDto> renamePage(String id, String newTitle) async {
    final p = pages[id]!;
    return pages[id] = PageDto(
      id: p.id,
      title: newTitle,
      createdAt: p.createdAt,
      updatedAt: p.updatedAt,
    );
  }

  @override
  Future<void> deletePage(String id) async {
    pages.remove(id);
    outlines.remove(id);
  }

  @override
  Future<void> addAlias(String pageId, String alias) async =>
      aliases[alias.toLowerCase()] = pageId;

  @override
  Future<void> removeAlias(String alias) async =>
      aliases.remove(alias.toLowerCase());

  int? _index(String pageId, String? id) =>
      id == null ? null : outlines[pageId]!.indexWhere((n) => n.id == id);

  @override
  Future<BlockDto> insertBlock(
    String pageId,
    Place place,
    String content,
  ) async {
    calls.add('insert');
    final n = OutlineNode(id: _id(), depth: 0, content: content);
    OutlineOps.insert(
      outlines[pageId]!,
      n,
      place.kind,
      _index(pageId, place.anchor),
    );
    return BlockDto(
      id: n.id,
      parentId: n.parentId,
      depth: n.depth,
      content: content,
      collapsed: false,
      hasChildren: false,
    );
  }

  @override
  Future<void> updateBlock(String id, String content) async {
    calls.add('update');
    final (p, i) = _find(id);
    outlines[p]![i].content = content;
  }

  @override
  Future<void> setCollapsed(String id, bool collapsed) async {
    final (p, i) = _find(id);
    outlines[p]![i].collapsed = collapsed;
  }

  @override
  Future<List<BlockDto>> deleteBlock(String id) async {
    final (p, i) = _find(id);
    OutlineOps.delete(outlines[p]!, i);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> moveBlock(
    String id,
    String pageId,
    Place place,
  ) async {
    final (p, i) = _find(id);
    OutlineOps.move(outlines[p]!, i, place.kind, _index(p, place.anchor));
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> indentBlock(String id) async {
    calls.add('indent');
    final (p, i) = _find(id);
    OutlineOps.indent(outlines[p]!, i);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> outdentBlock(String id) async {
    calls.add('outdent');
    final (p, i) = _find(id);
    OutlineOps.outdent(outlines[p]!, i);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> mergeBlock(String id, String into) async {
    calls.add('merge');
    final (p, i) = _find(id);
    OutlineOps.merge(outlines[p]!, i, _index(p, into)!);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> outline(String pageId) async => _dto(pageId);

  @override
  Future<List<BacklinkGroup>> backlinks(String pageId) async => [];
  @override
  Future<List<RelatedDto>> related(String pageId, {int limit = 12}) async => [];
  @override
  Future<void> recordVisit(String? from, String to) async {}
  @override
  Future<BlockRefDto?> blockRef(String id) async => null;
  @override
  Future<TimelineDto> timeline(String from, String to) async =>
      const TimelineDto(days: [], pages: []);

  @override
  Future<List<TitleHit>> searchTitles(String query, {int limit = 20}) async {
    final q = query.toLowerCase();
    return pages.values
        .where(
          (p) => p.journalDate == null && p.title.toLowerCase().contains(q),
        )
        .take(limit)
        .map((p) => TitleHit(pageId: p.id, title: p.title, isAlias: false))
        .toList();
  }

  @override
  Future<List<BlockHit>> searchBlocks(String query, {int limit = 20}) async =>
      [];

  @override
  String? assetPath(String hash) => null;

  @override
  Future<AssetDto> putAssetFile(String path, {String? mime}) =>
      throw UnimplementedError();

  /// Links (`[[x]]`) and tags (`#x`) between pages, built like the Rust side but simpler.
  @override
  Future<GraphData> graphData() async {
    final ids = pages.keys.toList();
    final index = {for (var i = 0; i < ids.length; i++) ids[i]: i};
    final s = <int>[], t = <int>[], k = <int>[];
    final link = RegExp(r'\[\[([^\]]+)\]\]');
    final tag = RegExp(r'(?:^|\s)#(\w+)');
    final degree = List.filled(ids.length, 0, growable: true);
    for (final id in List.of(ids)) {
      for (final n in outlines[id]!) {
        void add(String title, EdgeKind kind) {
          final p = _byTitle(title) ?? createPageForGraph(title, index, ids);
          s.add(index[id]!);
          t.add(index[p.id]!);
          k.add(kind.index);
        }

        for (final m in link.allMatches(n.content)) {
          add(m.group(1)!, EdgeKind.link);
        }
        for (final m in tag.allMatches(n.content)) {
          add(m.group(1)!, EdgeKind.tag);
        }
      }
    }
    while (degree.length < ids.length) {
      degree.add(0);
    }
    for (var e = 0; e < s.length; e++) {
      degree[s[e]]++;
      degree[t[e]]++;
    }
    return GraphData(
      nodes: [
        for (var i = 0; i < ids.length; i++)
          GraphNode(
            id: ids[i],
            title: pages[ids[i]]!.title,
            journalDate: pages[ids[i]]!.journalDate,
            createdAt: 0,
            degree: degree[i],
            backlinks: 0,
            cluster: 0,
          ),
      ],
      sources: Uint32List.fromList(s),
      targets: Uint32List.fromList(t),
      kinds: Uint8List.fromList(k),
      weights: Float32List.fromList(List.filled(s.length, 1)),
      seq: 0,
    );
  }

  PageDto createPageForGraph(
    String title,
    Map<String, int> index,
    List<String> ids,
  ) {
    final p = createPage(title);
    index[p.id] = ids.length;
    ids.add(p.id);
    return p;
  }

  final layouts = <FakeLayout>[];

  /// Steps each fake layout needs before it reports settled.
  int layoutSteps = 1;

  @override
  LayoutEngine createLayout(
    int nodeCount,
    Uint32List sources,
    Uint32List targets,
    Float32List weights, {
    Float32List? initial,
  }) {
    final l = FakeLayout(nodeCount, settleAfter: layoutSteps);
    layouts.add(l);
    return l;
  }
}

/// Places nodes on a circle and is settled after `settleAfter` steps.
class FakeLayout implements LayoutEngine {
  FakeLayout(this.n, {this.settleAfter = 1});
  final int n;
  final int settleAfter;
  int steps = 0;
  bool disposed = false;
  final pins = <int>[];

  @override
  Future<(Float32List, double)> step(int iterations) async {
    steps++;
    final p = Float32List(2 * n);
    for (var i = 0; i < n; i++) {
      final a = 2 * math.pi * i / math.max(n, 1);
      p[2 * i] = 200 * math.cos(a);
      p[2 * i + 1] = 200 * math.sin(a);
    }
    return (p, steps >= settleAfter ? 0.0 : 0.5);
  }

  @override
  Future<void> pin(int index, double x, double y) async => pins.add(index);
  @override
  Future<void> unpin(int index) async {}
  @override
  Future<void> reheat(double alpha) async {}
  @override
  void dispose() => disposed = true;
}
