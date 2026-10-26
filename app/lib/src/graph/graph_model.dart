import 'dart:collection';
import 'dart:typed_data';

import '../backend/graph_backend.dart';

/// Edge kinds, matching `synapse_ffi::api::dto::edge_kind`.
enum EdgeKind {
  link('Link'),
  tag('Tag'),
  property('Property'),
  blockRef('Block ref'),
  embed('Embed'),
  mention('Mention'),
  synapse('Synapse'),
  contains('Contains'),
  tagged('Shared tag');

  const EdgeKind(this.label);
  final String label;

  bool get structural => this != EdgeKind.mention && this != EdgeKind.synapse;
}

/// The page-level graph with adjacency, built once per [GraphData].
class GraphModel {
  GraphModel(this.data)
    : _kinds = [for (final k in data.kinds) EdgeKind.values[k]] {
    final n = data.nodes.length;
    adjacency = List.generate(n, (_) => <int>[]);
    for (var e = 0; e < edgeCount; e++) {
      adjacency[data.sources[e]].add(e);
      adjacency[data.targets[e]].add(e);
    }
    for (var i = 0; i < n; i++) {
      _byId[data.nodes[i].id] = i;
    }
  }

  final GraphData data;
  final List<EdgeKind> _kinds;
  late final List<List<int>> adjacency;
  final _byId = <String, int>{};

  int get nodeCount => data.nodes.length;
  int get edgeCount => data.kinds.length;
  GraphNode node(int i) => data.nodes[i];
  EdgeKind kind(int e) => _kinds[e];
  int source(int e) => data.sources[e];
  int target(int e) => data.targets[e];
  double weight(int e) => data.weights[e];
  int? indexOf(String id) => _byId[id];
  int other(int e, int i) => source(e) == i ? target(e) : source(e);

  bool isJournal(int i) => node(i).journalDate != null;

  bool isHub(int i) => node(i).isHub;

  /// Whether node `i` is a virtual tag node (a name, not a page).
  bool isTag(int i) => node(i).isTag;

  /// Whether the graph has any hub (folder-like page).
  late final bool hasHubs = data.nodes.any((n) => n.isHub);

  /// Hubs that are the home hub of the most pages first (journals excluded); ties by index.
  /// A hub's position here is its colour rank in the hub grouping.
  late final List<int> hubOrder = () {
    final count = <int, int>{};
    for (var i = 0; i < nodeCount; i++) {
      final h = node(i).homeHub;
      if (h != null && !isJournal(i)) count[h] = (count[h] ?? 0) + 1;
    }
    return count.keys.toList()..sort((a, b) {
      final c = count[b]!.compareTo(count[a]!);
      return c != 0 ? c : a.compareTo(b);
    });
  }();

  /// Hub node index → colour rank (see [hubOrder]).
  late final Map<int, int> hubRank = {
    for (var r = 0; r < hubOrder.length; r++) hubOrder[r]: r,
  };

  /// Members of each hub: the targets of its `CONTAINS` edges (hub -> member), in edge order.
  late final Map<int, Set<int>> hubMembers = () {
    final m = <int, Set<int>>{};
    for (var e = 0; e < edgeCount; e++) {
      if (kind(e) != EdgeKind.contains) continue;
      final hub = source(e), page = target(e);
      if (hub == page) continue;
      (m[hub] ??= <int>{}).add(page);
    }
    return m;
  }();

  /// Layout-only springs from every page in `nodes` to its home hub, so folders cluster.
  List<(int, int, double)> hubSprings(
    Iterable<int> nodes, {
    double strength = 0.6,
  }) => [
    for (final i in nodes)
      if (node(i).homeHub case final h? when h != i) (i, h, strength),
  ];

  /// The tag that groups page `i` in the "by tags" clusters: the page's most widely used tag
  /// (ties: alphabetical). Keys are lower-case; journals and untagged pages have none.
  String? homeTag(int i) {
    if (isJournal(i)) return null;
    String? best;
    for (final t in node(i).tags) {
      final k = t.toLowerCase();
      if (best == null ||
          _tagUse[k]! > _tagUse[best]! ||
          (_tagUse[k] == _tagUse[best] && k.compareTo(best) < 0)) {
        best = k;
      }
    }
    return best;
  }

  /// Pages per tag key and the first spelling met, over every non-journal page.
  late final Map<String, int> _tagUse = () {
    final m = <String, int>{};
    for (var i = 0; i < nodeCount; i++) {
      if (isJournal(i)) continue;
      for (final t in node(i).tags) {
        final k = t.toLowerCase();
        m[k] = (m[k] ?? 0) + 1;
        _tagName.putIfAbsent(k, () => t);
      }
    }
    return m;
  }();
  final _tagName = <String, String>{};

  /// Tag keys that are the home tag of the most pages first; a tag's position is its colour
  /// rank in the tag grouping.
  late final List<String> tagGroups = () {
    final count = <String, int>{};
    for (var i = 0; i < nodeCount; i++) {
      final t = homeTag(i);
      if (t != null) count[t] = (count[t] ?? 0) + 1;
    }
    return count.keys.toList()..sort((a, b) {
      final c = count[b]!.compareTo(count[a]!);
      return c != 0 ? c : a.compareTo(b);
    });
  }();

  late final Map<String, int> tagGroupRank = {
    for (var r = 0; r < tagGroups.length; r++) tagGroups[r]: r,
  };

  /// Display name of a tag key.
  String tagLabel(String key) => _tagName[key] ?? key;

  /// Layout-only springs from every page in `nodes` to the best connected page of its home
  /// tag, so pages sharing a tag cluster spatially.
  List<(int, int, double)> tagSprings(
    Iterable<int> nodes, {
    double strength = 0.6,
  }) {
    final anchor = <String, int>{};
    for (final i in nodes) {
      final t = homeTag(i);
      if (t == null) continue;
      final a = anchor[t];
      if (a == null || node(i).degree > node(a).degree) anchor[t] = i;
    }
    return [
      for (final i in nodes)
        if (homeTag(i) case final t? when anchor[t] != i)
          (i, anchor[t]!, strength),
    ];
  }

  /// Virtual tag nodes in use (targets of `TAGGED` edges), most used first. Pages targeted
  /// by inline `#x` references are ordinary notes and are not listed.
  late final List<int> tags = () {
    final count = <int, int>{};
    for (var e = 0; e < edgeCount; e++) {
      if (kind(e) == EdgeKind.tagged && isTag(target(e))) {
        count[target(e)] = (count[target(e)] ?? 0) + 1;
      }
    }
    final l = count.keys.toList()
      ..sort((a, b) => count[b]!.compareTo(count[a]!));
    return l;
  }();

  /// Hop distance from `seed` over edges of `kinds`, up to `depth`.
  Map<int, int> neighbourhood(int seed, int depth, Set<EdgeKind> kinds) {
    final dist = <int, int>{seed: 0};
    final q = Queue<int>()..add(seed);
    while (q.isNotEmpty) {
      final i = q.removeFirst();
      final d = dist[i]!;
      if (d >= depth) continue;
      for (final e in adjacency[i]) {
        if (!kinds.contains(kind(e))) continue;
        final j = other(e, i);
        if (!dist.containsKey(j)) {
          dist[j] = d + 1;
          q.add(j);
        }
      }
    }
    return dist;
  }

  /// The induced subgraph on `nodes` (global indices) with edges of `kinds`. Parallel
  /// edges between the same pair are merged for the layout but kept for drawing.
  ///
  /// `springs` adds layout-only attractions (global pairs and strengths) that are not drawn.
  SubGraph subgraph(
    List<int> nodes,
    Set<EdgeKind> kinds, {
    List<(int, int, double)> springs = const [],
  }) {
    final local = <int, int>{};
    for (var k = 0; k < nodes.length; k++) {
      local[nodes[k]] = k;
    }
    final edges = <int>[];
    for (final i in nodes) {
      for (final e in adjacency[i]) {
        if (source(e) != i) continue; // visit each edge once, from its source
        if (!kinds.contains(kind(e))) continue;
        if (local.containsKey(target(e))) edges.add(e);
      }
    }
    return SubGraph(this, nodes, local, edges, springs: springs);
  }
}

/// Filters of the network modes.
class GraphFilter {
  const GraphFilter({
    this.journals = true,
    this.orphans = true,
    this.minDegree = 0,
    this.tag,
    this.query = '',
    this.from,
    this.to,
    this.kinds = const {
      EdgeKind.link,
      EdgeKind.tag,
      EdgeKind.property,
      EdgeKind.blockRef,
      EdgeKind.embed,
      EdgeKind.mention,
      EdgeKind.contains,
      EdgeKind.tagged,
    },
  });

  final bool journals;
  final bool orphans;
  final int minDegree;

  /// Only pages tagged with this tag page (and the tag itself).
  final int? tag;

  /// Title substring.
  final String query;

  /// Created-at range (inclusive days, `YYYY-MM-DD`).
  final String? from;
  final String? to;
  final Set<EdgeKind> kinds;

  GraphFilter copyWith({
    bool? journals,
    bool? orphans,
    int? minDegree,
    int? Function()? tag,
    String? query,
    String? Function()? from,
    String? Function()? to,
    Set<EdgeKind>? kinds,
  }) => GraphFilter(
    journals: journals ?? this.journals,
    orphans: orphans ?? this.orphans,
    minDegree: minDegree ?? this.minDegree,
    tag: tag == null ? this.tag : tag(),
    query: query ?? this.query,
    from: from == null ? this.from : from(),
    to: to == null ? this.to : to(),
    kinds: kinds ?? this.kinds,
  );

  List<int> apply(GraphModel g) {
    final q = query.trim().toLowerCase();
    final tagged = tag == null
        ? null
        : {
            tag!,
            for (final e in g.adjacency[tag!])
              if ((g.kind(e) == EdgeKind.tag || g.kind(e) == EdgeKind.tagged) &&
                  g.target(e) == tag)
                g.source(e),
          };
    String day(int i) {
      final ms = g.node(i).createdAt.toInt();
      final d = DateTime.fromMillisecondsSinceEpoch(ms);
      return '${d.year.toString().padLeft(4, '0')}-${d.month.toString().padLeft(2, '0')}'
          '-${d.day.toString().padLeft(2, '0')}';
    }

    bool inRange(int i) =>
        (from == null || day(i).compareTo(from!) >= 0) &&
        (to == null || day(i).compareTo(to!) <= 0);
    // A tag node has no creation date: it stays when one of its tagged pages is in range.
    bool tagInRange(int i) {
      if (from == null && to == null) return true;
      for (final e in g.adjacency[i]) {
        if (g.kind(e) != EdgeKind.tagged || g.target(e) != i) continue;
        final p = g.source(e);
        if (!g.isTag(p) && (journals || !g.isJournal(p)) && inRange(p)) {
          return true;
        }
      }
      return false;
    }

    final base = <int>[
      for (var i = 0; i < g.nodeCount; i++)
        if ((journals || !g.isJournal(i)) &&
            (tagged == null || tagged.contains(i)) &&
            (q.isEmpty || g.node(i).title.toLowerCase().contains(q)) &&
            (g.isTag(i) ? tagInRange(i) : inRange(i)))
          i,
    ];
    if (orphans && minDegree == 0) return base;
    // Degree counts only neighbours that are themselves shown, over the visible kinds.
    final shown = base.toSet();
    int degree(int i) {
      final seen = <int>{};
      for (final e in g.adjacency[i]) {
        if (!kinds.contains(g.kind(e))) continue;
        final j = g.other(e, i);
        if (shown.contains(j)) seen.add(j);
      }
      return seen.length;
    }

    return [
      for (final i in base)
        if (() {
          final d = degree(i);
          return d >= minDegree && (orphans || d > 0);
        }())
          i,
    ];
  }
}

/// A view-sized piece of the graph: global node indices, a global→local map and the global
/// ids of the edges between them.
class SubGraph {
  SubGraph(
    this.model,
    this.nodes,
    this.local,
    this.edges, {
    this.springs = const [],
  });

  final GraphModel model;
  final List<int> nodes;
  final Map<int, int> local;
  final List<int> edges;

  /// Layout-only attractions between global node pairs.
  final List<(int, int, double)> springs;

  int get length => nodes.length;

  /// Edge endpoints in local indices, deduplicated per pair, with summed weights (for the
  /// layout). Synapse weights are scaled so strong synapses pull harder.
  (Uint32List, Uint32List, Float32List) layoutEdges() {
    final pairs = <int, double>{};
    for (final e in edges) {
      final a = local[model.source(e)]!;
      final b = local[model.target(e)]!;
      final key = a < b ? a * length + b : b * length + a;
      final w = model.kind(e) == EdgeKind.synapse
          ? model.weight(e)
          : model.kind(e) == EdgeKind.mention
          ? 0.3
          : 1.0;
      pairs[key] = (pairs[key] ?? 0) + w;
    }
    for (final (ga, gb, w) in springs) {
      final a = local[ga], b = local[gb];
      if (a == null || b == null || a == b) continue;
      final key = a < b ? a * length + b : b * length + a;
      pairs[key] = (pairs[key] ?? 0) + w;
    }
    final s = Uint32List(pairs.length);
    final t = Uint32List(pairs.length);
    final w = Float32List(pairs.length);
    var k = 0;
    pairs.forEach((key, weight) {
      s[k] = key ~/ length;
      t[k] = key % length;
      w[k] = weight;
      k++;
    });
    return (s, t, w);
  }
}

/// Runs a force layout; implemented over the Rust `ForceLayout` (and by a fake in tests).
abstract class LayoutEngine {
  /// Runs up to `iterations` ticks: positions `x0, y0, ...` and the remaining alpha (0 when
  /// settled).
  Future<(Float32List, double)> step(int iterations);
  Future<void> pin(int index, double x, double y);
  Future<void> unpin(int index);
  Future<void> reheat(double alpha);
  void dispose();
}
