import 'dart:async';

import 'package:flutter/foundation.dart';

import '../backend/graph_backend.dart';
import 'outline_ops.dart';
import 'write_queue.dart';

/// Editing state of one page's outline.
///
/// Edits are applied to [nodes] immediately (optimistic) and sent to the core through the
/// shared [WriteQueue]. Text is saved with a debounce; structural edits flush pending text
/// first. Blocks created locally carry a temporary id until the core assigns the real one;
/// queued tasks read ids from the node objects when they run, so they always see the real id.
/// After structural edits settle, the outline is re-read and adopted if it disagrees.
class OutlineController extends ChangeNotifier {
  OutlineController({
    required this.backend,
    required this.queue,
    required this.pageId,
    required List<BlockDto> blocks,
    this.saveDelay = const Duration(milliseconds: 350),
  }) : nodes = blocks.map(OutlineNode.fromDto).toList();

  final GraphBackend backend;
  final WriteQueue queue;
  final String pageId;
  final Duration saveDelay;
  final List<OutlineNode> nodes;

  /// Block being edited, if any.
  String? focusedId;

  /// Caret requested for the focused block; consumed by the row once applied.
  int? pendingCaret;

  final _timers = <OutlineNode, Timer>{};
  Timer? _reconcile;
  int _structuralEdits = 0;
  static int _tmpSeq = 0;
  bool _disposed = false;

  // ------------------------------------------------------------- queries

  List<int> get visible => OutlineOps.visible(nodes);

  int indexOf(String id) => nodes.indexWhere((n) => n.id == id);

  OutlineNode? byId(String id) {
    final i = indexOf(id);
    return i < 0 ? null : nodes[i];
  }

  bool hasChildren(OutlineNode n) =>
      OutlineOps.hasChildren(nodes, nodes.indexOf(n));

  OutlineNode? _visibleNeighbour(OutlineNode n, int step) {
    final vis = visible;
    final pos = vis.indexOf(nodes.indexOf(n));
    final k = pos + step;
    return pos < 0 || k < 0 || k >= vis.length ? null : nodes[vis[k]];
  }

  OutlineNode? previousVisible(OutlineNode n) => _visibleNeighbour(n, -1);
  OutlineNode? nextVisible(OutlineNode n) => _visibleNeighbour(n, 1);

  // --------------------------------------------------------------- focus

  void focus(String id, {int? caret}) {
    focusedId = id;
    pendingCaret = caret;
    notifyListeners();
  }

  /// Called by a row when it loses focus.
  void blurred(String id) {
    final n = byId(id);
    if (n != null) _flush(n);
    if (focusedId == id) {
      focusedId = null;
      pendingCaret = null;
      notifyListeners();
    }
  }

  // ---------------------------------------------------------------- text

  /// Text changed while typing (debounced save).
  void setText(OutlineNode n, String text) {
    if (n.content == text) return;
    n.content = text;
    _timers[n]?.cancel();
    _timers[n] = Timer(saveDelay, () => _flush(n));
  }

  void _flush(OutlineNode n) {
    final t = _timers.remove(n);
    if (t == null) return;
    t.cancel();
    final content = n.content;
    _enqueue(() => backend.updateBlock(n.id, content));
  }

  void flushAll() {
    for (final n in _timers.keys.toList()) {
      _flush(n);
    }
  }

  /// Replaces a block's text right away (e.g. toggling TODO from a checkbox).
  void replaceText(OutlineNode n, String text, {int? caret}) {
    setText(n, text);
    _flush(n);
    if (caret != null && focusedId == n.id) pendingCaret = caret;
    notifyListeners();
  }

  // ----------------------------------------------------------- structure

  /// Creates the first block of an empty page and focuses it.
  void insertFirst() {
    final node = _newNode('');
    OutlineOps.insert(nodes, node, PlaceKind.lastChildOf, null);
    _sendInsert(node, PlaceKind.lastChildOf, null);
    focus(node.id, caret: 0);
  }

  /// Enter at `caret`: splits the block. At the very start of a non-empty block a new empty
  /// block is inserted above and the caret stays put.
  void split(OutlineNode n, int caret) {
    final text = n.content;
    caret = caret.clamp(0, text.length);
    final i = nodes.indexOf(n);
    if (caret == 0 && text.isNotEmpty) {
      final node = _newNode('');
      OutlineOps.insert(nodes, node, PlaceKind.before, i);
      _sendInsert(node, PlaceKind.before, n);
      focus(n.id, caret: 0);
      return;
    }
    final before = text.substring(0, caret);
    var after = text.substring(caret);
    if (after.isEmpty &&
        RegExp(r'^(TODO|DOING|DONE|LATER|NOW) ').hasMatch(before)) {
      after = 'TODO ';
    }
    setText(n, before);
    _flush(n);
    final intoChildren = !n.collapsed && OutlineOps.hasChildren(nodes, i);
    final kind = intoChildren ? PlaceKind.firstChildOf : PlaceKind.after;
    final node = _newNode(after);
    OutlineOps.insert(nodes, node, kind, i);
    _sendInsert(node, kind, n);
    focus(node.id, caret: after == 'TODO ' ? after.length : 0);
  }

  void indent(OutlineNode n, {int? caret}) {
    if (!OutlineOps.indent(nodes, nodes.indexOf(n))) return;
    flushAll();
    _structural(() => backend.indentBlock(n.id));
    _refocus(n, caret);
  }

  void outdent(OutlineNode n, {int? caret}) {
    if (!OutlineOps.outdent(nodes, nodes.indexOf(n))) return;
    flushAll();
    _structural(() => backend.outdentBlock(n.id));
    _refocus(n, caret);
  }

  /// Backspace at the start of a block: merge into the previous visible block.
  /// Returns false when there is nothing to merge into.
  bool mergeIntoPrevious(OutlineNode n) {
    final prev = previousVisible(n);
    if (prev == null) return false;
    _merge(n, prev);
    return true;
  }

  /// Delete at the end of a block: pull the next visible block into it.
  bool mergeNext(OutlineNode n) {
    final next = nextVisible(n);
    if (next == null) return false;
    _merge(next, n);
    return true;
  }

  void _merge(OutlineNode n, OutlineNode into) {
    final caret = into.content.length;
    flushAll();
    OutlineOps.merge(nodes, nodes.indexOf(n), nodes.indexOf(into));
    _structural(() => backend.mergeBlock(n.id, into.id));
    focus(into.id, caret: caret);
  }

  /// Alt+Shift+Up/Down: swap with the previous/next sibling.
  void moveAmongSiblings(OutlineNode n, {required bool up, int? caret}) {
    final i = nodes.indexOf(n);
    final s = up
        ? OutlineOps.prevSibling(nodes, i)
        : OutlineOps.nextSibling(nodes, i);
    if (s == null) return;
    final kind = up ? PlaceKind.before : PlaceKind.after;
    moveTo(n, kind, nodes[s], caret: caret);
  }

  /// Drag and drop (or keyboard move) relative to `anchor`.
  void moveTo(
    OutlineNode n,
    PlaceKind kind,
    OutlineNode? anchor, {
    int? caret,
  }) {
    final a = anchor == null ? null : nodes.indexOf(anchor);
    if (!OutlineOps.move(nodes, nodes.indexOf(n), kind, a)) return;
    flushAll();
    final expand =
        anchor != null &&
        (kind == PlaceKind.firstChildOf || kind == PlaceKind.lastChildOf);
    _structural(() async {
      if (expand) await backend.setCollapsed(anchor.id, false);
      return backend.moveBlock(
        n.id,
        pageId,
        Place(kind: kind, anchor: anchor?.id),
      );
    });
    if (focusedId == n.id) {
      _refocus(n, caret);
    } else {
      notifyListeners();
    }
  }

  void delete(OutlineNode n) {
    final prev = previousVisible(n);
    flushAll();
    OutlineOps.delete(nodes, nodes.indexOf(n));
    _structural(() => backend.deleteBlock(n.id));
    if (focusedId == n.id && prev != null) {
      focus(prev.id, caret: prev.content.length);
    } else {
      notifyListeners();
    }
  }

  void toggleCollapsed(OutlineNode n) {
    if (!hasChildren(n) && !n.collapsed) return;
    n.collapsed = !n.collapsed;
    final collapsed = n.collapsed;
    _enqueue(() => backend.setCollapsed(n.id, collapsed));
    if (collapsed && focusedId != null) {
      final f = byId(focusedId!);
      if (f != null && !visible.contains(nodes.indexOf(f))) focus(n.id);
    }
    notifyListeners();
  }

  /// Ctrl+Enter: none → TODO → DONE → none. Returns the caret shift.
  int cycleTodo(OutlineNode n) {
    final t = n.content;
    String next;
    int shift;
    if (t.startsWith('TODO ') ||
        t.startsWith('DOING ') ||
        t.startsWith('NOW ') ||
        t.startsWith('LATER ')) {
      final rest = t.substring(t.indexOf(' ') + 1);
      next = 'DONE $rest';
      shift = next.length - t.length;
    } else if (t.startsWith('DONE ')) {
      next = t.substring(5);
      shift = -5;
    } else {
      next = 'TODO $t';
      shift = 5;
    }
    replaceText(n, next);
    return shift;
  }

  // ------------------------------------------------------------ plumbing

  OutlineNode _newNode(String content) =>
      OutlineNode(id: '_tmp${_tmpSeq++}', depth: 0, content: content);

  void _sendInsert(OutlineNode node, PlaceKind kind, OutlineNode? anchor) {
    final content = node.content;
    _enqueue(() async {
      final b = await backend.insertBlock(
        pageId,
        Place(kind: kind, anchor: anchor?.id),
        content,
      );
      final wasFocused = focusedId == node.id;
      node.id = b.id;
      OutlineOps.fixParents(nodes);
      if (wasFocused) focusedId = b.id;
      if (!_disposed) notifyListeners();
    });
  }

  void _refocus(OutlineNode n, int? caret) {
    if (focusedId == n.id) {
      pendingCaret = caret;
    }
    notifyListeners();
  }

  void _structural(Future<List<BlockDto>> Function() op) {
    _structuralEdits++;
    final seen = _structuralEdits;
    _enqueue(() async {
      await op();
    });
    _reconcile?.cancel();
    _reconcile = Timer(const Duration(milliseconds: 600), () {
      if (seen == _structuralEdits) _verify();
    });
  }

  /// Re-reads the outline and adopts the core's structure if it differs from ours.
  void _verify() {
    final seen = _structuralEdits;
    _enqueue(() async {
      final server = await backend.outline(pageId);
      if (_disposed || seen != _structuralEdits) return;
      if (nodes.any((n) => n.id.startsWith('_tmp'))) return;
      final same =
          server.length == nodes.length &&
          List.generate(server.length, (i) => i).every(
            (i) =>
                server[i].id == nodes[i].id &&
                server[i].depth == nodes[i].depth,
          );
      if (same) return;
      debugPrint('outline: adopting core structure for page $pageId');
      final local = {for (final n in nodes) n.id: n};
      final adopted = server.map((b) {
        final n = local[b.id] ?? OutlineNode.fromDto(b);
        n
          ..depth = b.depth
          ..parentId = b.parentId
          ..collapsed = b.collapsed;
        return n;
      }).toList();
      nodes
        ..clear()
        ..addAll(adopted);
      notifyListeners();
    });
  }

  void _enqueue(Future<void> Function() task) {
    queue.run(task).ignore();
  }

  @override
  void dispose() {
    flushAll();
    _reconcile?.cancel();
    _disposed = true;
    super.dispose();
  }
}
