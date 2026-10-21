import '../backend/graph_backend.dart';

/// One outline row held by the editor. Mutable on purpose: the editor applies edits in place
/// and rebuilds only what changed.
class OutlineNode {
  OutlineNode({
    required this.id,
    this.parentId,
    required this.depth,
    required this.content,
    this.collapsed = false,
  });

  factory OutlineNode.fromDto(BlockDto b) => OutlineNode(
    id: b.id,
    parentId: b.parentId,
    depth: b.depth,
    content: b.content,
    collapsed: b.collapsed,
  );

  String id;
  String? parentId;
  int depth;
  String content;
  bool collapsed;

  OutlineNode copy() => OutlineNode(
    id: id,
    parentId: parentId,
    depth: depth,
    content: content,
    collapsed: collapsed,
  );

  @override
  String toString() => '$depth:$content';
}

/// Outline algorithms on a pre-order list of nodes. They mirror `synapse-ffi`'s outline
/// module so the editor can apply edits optimistically and agree with the core.
class OutlineOps {
  OutlineOps._();

  /// Index right after the subtree rooted at `i`.
  static int subtreeEnd(List<OutlineNode> l, int i) {
    var j = i + 1;
    while (j < l.length && l[j].depth > l[i].depth) {
      j++;
    }
    return j;
  }

  static bool hasChildren(List<OutlineNode> l, int i) =>
      i + 1 < l.length && l[i + 1].depth > l[i].depth;

  static int? prevSibling(List<OutlineNode> l, int i) {
    for (var j = i - 1; j >= 0; j--) {
      if (l[j].depth == l[i].depth) return j;
      if (l[j].depth < l[i].depth) return null;
    }
    return null;
  }

  static int? nextSibling(List<OutlineNode> l, int i) {
    final j = subtreeEnd(l, i);
    return j < l.length && l[j].depth == l[i].depth ? j : null;
  }

  static int? parent(List<OutlineNode> l, int i) {
    for (var j = i - 1; j >= 0; j--) {
      if (l[j].depth < l[i].depth) return j;
    }
    return null;
  }

  /// Indices of rows not hidden under a collapsed ancestor.
  static List<int> visible(List<OutlineNode> l) {
    final out = <int>[];
    var i = 0;
    while (i < l.length) {
      out.add(i);
      i = l[i].collapsed ? subtreeEnd(l, i) : i + 1;
    }
    return out;
  }

  /// Recomputes `parentId` from depths.
  static void fixParents(List<OutlineNode> l) {
    final stack = <OutlineNode>[];
    for (final n in l) {
      while (stack.isNotEmpty && stack.last.depth >= n.depth) {
        stack.removeLast();
      }
      n.parentId = stack.isEmpty ? null : stack.last.id;
      stack.add(n);
    }
  }

  static void _shift(List<OutlineNode> l, int from, int to, int by) {
    for (var k = from; k < to; k++) {
      l[k].depth += by;
    }
  }

  /// Tab. Returns false when there is no previous sibling.
  static bool indent(List<OutlineNode> l, int i) {
    final p = prevSibling(l, i);
    if (p == null) return false;
    l[p].collapsed = false;
    _shift(l, i, subtreeEnd(l, i), 1);
    fixParents(l);
    return true;
  }

  /// Shift-Tab with direct outdenting: following siblings become children of the block.
  static bool outdent(List<OutlineNode> l, int i) {
    if (l[i].depth == 0) return false;
    final end = subtreeEnd(l, i);
    final hadFollowing = end < l.length && l[end].depth == l[i].depth;
    _shift(l, i, end, -1);
    if (hadFollowing) l[i].collapsed = false;
    fixParents(l);
    return true;
  }

  /// Merges row `i` into row `into` (its previous visible row).
  static void merge(List<OutlineNode> l, int i, int into) {
    final target = l[into];
    final isParent = parent(l, i) == into;
    target.content += l[i].content;
    final end = subtreeEnd(l, i);
    if (end > i + 1) {
      if (isParent) {
        _shift(l, i + 1, end, -1);
      } else {
        target.collapsed = false;
        _shift(l, i + 1, end, target.depth - l[i].depth);
      }
    }
    l.removeAt(i);
    fixParents(l);
  }

  /// Deletes row `i` with its subtree.
  static void delete(List<OutlineNode> l, int i) {
    l.removeRange(i, subtreeEnd(l, i));
    fixParents(l);
  }

  /// Insertion index and depth for a placement, or null when the anchor is unknown.
  static (int, int)? slot(List<OutlineNode> l, PlaceKind kind, int? anchor) {
    switch (kind) {
      case PlaceKind.before:
        if (anchor == null) return null;
        return (anchor, l[anchor].depth);
      case PlaceKind.after:
        if (anchor == null) return null;
        return (subtreeEnd(l, anchor), l[anchor].depth);
      case PlaceKind.firstChildOf:
        if (anchor == null) return (0, 0);
        return (anchor + 1, l[anchor].depth + 1);
      case PlaceKind.lastChildOf:
        if (anchor == null) return (l.length, 0);
        return (subtreeEnd(l, anchor), l[anchor].depth + 1);
    }
  }

  static int insert(
    List<OutlineNode> l,
    OutlineNode node,
    PlaceKind kind,
    int? anchor,
  ) {
    final (at, depth) = slot(l, kind, anchor)!;
    if (anchor != null &&
        (kind == PlaceKind.firstChildOf || kind == PlaceKind.lastChildOf)) {
      l[anchor].collapsed = false;
    }
    node.depth = depth;
    l.insert(at, node);
    fixParents(l);
    return at;
  }

  /// Moves row `i` with its subtree. Returns false for moves into its own subtree.
  static bool move(List<OutlineNode> l, int i, PlaceKind kind, int? anchor) {
    final end = subtreeEnd(l, i);
    if (anchor != null && anchor >= i && anchor < end) return false;
    final s = slot(l, kind, anchor);
    if (s == null) return false;
    if (anchor != null &&
        (kind == PlaceKind.firstChildOf || kind == PlaceKind.lastChildOf)) {
      l[anchor].collapsed = false;
    }
    var (at, depth) = s;
    final moved = l.sublist(i, end);
    final delta = depth - moved.first.depth;
    for (final n in moved) {
      n.depth += delta;
    }
    l.removeRange(i, end);
    if (at > i) at -= moved.length;
    l.insertAll(at, moved);
    fixParents(l);
    return true;
  }
}
