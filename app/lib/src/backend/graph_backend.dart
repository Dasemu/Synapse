import 'dart:typed_data';

import '../graph/graph_model.dart';
import '../rust/api/dto.dart';
import '../rust/api/graph.dart';
import '../rust/api/layout.dart';

export '../rust/api/dto.dart';

/// What the UI needs from a graph. [FfiGraphBackend] talks to the Rust core; tests use an
/// in-memory implementation. Keeping the UI on this interface (instead of the generated
/// opaque class) is what makes the editor testable without the native library.
abstract class GraphBackend {
  /// Today's date in the graph clock, `YYYY-MM-DD`.
  String today();

  Future<List<PageData>> journals({String? before, required int limit});
  Future<PageData> journal(String date);
  Future<PageData?> page(String id);
  Future<PageData?> pageByTitle(String title);

  /// Page by title or alias, created if missing.
  Future<PageData> openPage(String title);
  Future<List<PageDto>> listPages({
    bool includeJournals = false,
    required int limit,
    int offset = 0,
  });
  Future<PageDto> renamePage(String id, String newTitle);
  Future<void> deletePage(String id);
  Future<void> addAlias(String pageId, String alias);
  Future<void> removeAlias(String alias);

  Future<BlockDto> insertBlock(String pageId, Place place, String content);
  Future<void> updateBlock(String id, String content);
  Future<void> setCollapsed(String id, bool collapsed);
  Future<List<BlockDto>> deleteBlock(String id);
  Future<List<BlockDto>> moveBlock(String id, String pageId, Place place);
  Future<List<BlockDto>> indentBlock(String id);
  Future<List<BlockDto>> outdentBlock(String id);
  Future<List<BlockDto>> mergeBlock(String id, String into);
  Future<List<BlockDto>> outline(String pageId);

  Future<List<BacklinkGroup>> backlinks(String pageId);
  Future<List<RelatedDto>> related(String pageId, {int limit = 12});
  Future<void> recordVisit(String? from, String to);
  Future<BlockRefDto?> blockRef(String id);
  Future<TimelineDto> timeline(String from, String to);

  Future<List<TitleHit>> searchTitles(String query, {int limit = 20});
  Future<List<BlockHit>> searchBlocks(String query, {int limit = 20});

  /// Local path of an `asset://<hash>` blob, if present.
  String? assetPath(String hash);
  Future<AssetDto> putAssetFile(String path, {String? mime});

  /// The whole page-level graph (cached by the core until the graph changes).
  Future<GraphData> graphData();

  /// A force layout over `nodeCount` nodes and the given (local) edges.
  LayoutEngine createLayout(
    int nodeCount,
    Uint32List sources,
    Uint32List targets,
    Float32List weights, {
    Float32List? initial,
  });
}

class _FfiLayout implements LayoutEngine {
  _FfiLayout(this._l);
  final ForceLayout _l;

  @override
  Future<(Float32List, double)> step(int iterations) async {
    final f = await _l.step(iterations: iterations);
    return (f.positions, f.alpha);
  }

  @override
  Future<void> pin(int index, double x, double y) =>
      _l.pin(index: index, x: x, y: y);
  @override
  Future<void> unpin(int index) => _l.unpin(index: index);
  @override
  Future<void> reheat(double alpha) => _l.reheat(alpha: alpha);
  @override
  void dispose() => _l.dispose();
}

class FfiGraphBackend implements GraphBackend {
  FfiGraphBackend(this._g);

  final SynapseGraph _g;

  static Future<FfiGraphBackend> open(String dir) async =>
      FfiGraphBackend(await SynapseGraph.open(dir: dir));

  @override
  String today() => _g.today();

  @override
  Future<List<PageData>> journals({String? before, required int limit}) =>
      _g.journals(before: before, limit: limit);
  @override
  Future<PageData> journal(String date) => _g.journal(date: date);
  @override
  Future<PageData?> page(String id) => _g.page(id: id);
  @override
  Future<PageData?> pageByTitle(String title) => _g.pageByTitle(title: title);
  @override
  Future<PageData> openPage(String title) => _g.openPage(title: title);
  @override
  Future<List<PageDto>> listPages({
    bool includeJournals = false,
    required int limit,
    int offset = 0,
  }) => _g.listPages(
    includeJournals: includeJournals,
    limit: limit,
    offset: offset,
  );
  @override
  Future<PageDto> renamePage(String id, String newTitle) =>
      _g.renamePage(id: id, newTitle: newTitle);
  @override
  Future<void> deletePage(String id) => _g.deletePage(id: id);
  @override
  Future<void> addAlias(String pageId, String alias) =>
      _g.addAlias(pageId: pageId, alias: alias);
  @override
  Future<void> removeAlias(String alias) => _g.removeAlias(alias: alias);

  @override
  Future<BlockDto> insertBlock(String pageId, Place place, String content) =>
      _g.insertBlock(pageId: pageId, place: place, content: content);
  @override
  Future<void> updateBlock(String id, String content) =>
      _g.updateBlock(id: id, content: content);
  @override
  Future<void> setCollapsed(String id, bool collapsed) =>
      _g.setCollapsed(id: id, collapsed: collapsed);
  @override
  Future<List<BlockDto>> deleteBlock(String id) => _g.deleteBlock(id: id);
  @override
  Future<List<BlockDto>> moveBlock(String id, String pageId, Place place) =>
      _g.moveBlock(id: id, pageId: pageId, place: place);
  @override
  Future<List<BlockDto>> indentBlock(String id) => _g.indentBlock(id: id);
  @override
  Future<List<BlockDto>> outdentBlock(String id) => _g.outdentBlock(id: id);
  @override
  Future<List<BlockDto>> mergeBlock(String id, String into) =>
      _g.mergeBlock(id: id, into: into);
  @override
  Future<List<BlockDto>> outline(String pageId) => _g.outline(pageId: pageId);

  @override
  Future<List<BacklinkGroup>> backlinks(String pageId) =>
      _g.backlinks(pageId: pageId);
  @override
  Future<List<RelatedDto>> related(String pageId, {int limit = 12}) =>
      _g.related(pageId: pageId, limit: limit);
  @override
  Future<void> recordVisit(String? from, String to) =>
      _g.recordVisit(from: from, to: to);
  @override
  Future<BlockRefDto?> blockRef(String id) => _g.blockRef(id: id);
  @override
  Future<TimelineDto> timeline(String from, String to) =>
      _g.timeline(from: from, to: to);

  @override
  Future<List<TitleHit>> searchTitles(String query, {int limit = 20}) =>
      _g.searchTitles(query: query, limit: limit);
  @override
  Future<List<BlockHit>> searchBlocks(String query, {int limit = 20}) =>
      _g.searchBlocks(query: query, limit: limit);

  @override
  String? assetPath(String hash) => _g.assetPath(hash: hash);
  @override
  Future<AssetDto> putAssetFile(String path, {String? mime}) =>
      _g.putAssetFile(path: path, mime: mime);

  @override
  Future<GraphData> graphData() => _g.graphData();

  @override
  LayoutEngine createLayout(
    int nodeCount,
    Uint32List sources,
    Uint32List targets,
    Float32List weights, {
    Float32List? initial,
  }) => _FfiLayout(
    ForceLayout(
      nodeCount: nodeCount,
      sources: sources,
      targets: targets,
      weights: weights,
      initial: initial,
    ),
  );
}
