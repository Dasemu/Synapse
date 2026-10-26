import 'dart:math' as math;

import 'dart:typed_data';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';
import 'graph_model.dart';

/// A node of the hub / namespace / tag tree.
class TreeItem {
  TreeItem(this.label, {this.global, this.isHub = false});
  final String label;

  /// Drawn with the folder look.
  final bool isHub;

  /// Page index when the item is a page.
  int? global;
  final children = <TreeItem>[];
  TreeItem? parent;

  /// Whether the children are shown in the canvas.
  bool expanded = false;
  int depth = 0;
  int leaves = 1;
  double angle = 0;
  Offset pos = Offset.zero;

  /// Colour-space position: centre hue (degrees) of this branch's sector and its width.
  /// Siblings split their parent's sector evenly, so they are equally far apart in hue.
  double hue = 0;
  double span = 360;
  int sibling = 0;

  TreeItem child(String label) {
    for (final c in children) {
      if (c.label == label) return c;
    }
    final c = TreeItem(label);
    children.add(c);
    return c;
  }
}

/// Hub branches: root hubs (hubs that are no other hub's member) with their members,
/// recursively, plus the notes that live in no hub (orphans included). A hub is expanded once
/// (the first time it is reached) and never inside itself, so cycles and shared sub-hubs stay
/// bounded; later occurrences are leaves.
List<TreeItem> _hubBranches(GraphModel g) {
  final branches = <TreeItem>[];
  final members = g.hubMembers;
  final nested = {for (final m in members.values) ...m};
  final expanded = <int>{};
  TreeItem build(int hub) {
    final item = TreeItem(g.node(hub).title, global: hub, isHub: true);
    if (!expanded.add(hub)) return item;
    for (final m in members[hub] ?? const <int>{}) {
      item.children.add(
        g.isHub(m) ? build(m) : TreeItem(g.node(m).title, global: m),
      );
    }
    return item;
  }

  final hubs = [
    for (var i = 0; i < g.nodeCount; i++)
      if (g.isHub(i)) i,
  ];
  for (final h in hubs) {
    if (!nested.contains(h)) branches.add(build(h));
  }
  // Hubs only reachable through a cycle still need a place.
  for (final h in hubs) {
    if (!expanded.contains(h)) branches.add(build(h));
  }
  // Notes under no container hang straight off the centre (namespaced ones keep their
  // namespace branch).
  for (var i = 0; i < g.nodeCount; i++) {
    if (!g.isHub(i) &&
        !g.isJournal(i) &&
        !nested.contains(i) &&
        !g.node(i).title.contains('/')) {
      branches.add(TreeItem(g.node(i).title, global: i));
    }
  }
  return branches;
}

/// Builds a tree from hubs (folders, straight off the centre, with loose notes), namespaces (`a/b/c` titles split on `/`, attached
/// directly to the centre) and tags (each tag with the pages using it). Nothing is capped:
/// large branches are collapsed in the canvas instead.
TreeItem buildNamespaceTree(GraphModel g, {String name = 'Graph'}) {
  final root = TreeItem(name);
  root.children.addAll(_hubBranches(g));
  final ns = TreeItem('Namespaces');
  final byTitle = <String, int>{};
  for (var i = 0; i < g.nodeCount; i++) {
    if (!g.isJournal(i)) byTitle[g.node(i).title.toLowerCase()] = i;
  }
  for (var i = 0; i < g.nodeCount; i++) {
    final t = g.node(i).title;
    if (g.isJournal(i) || !t.contains('/')) continue;
    var at = ns;
    final parts = t.split('/');
    for (var k = 0; k < parts.length; k++) {
      at = at.child(parts[k].trim());
      final path = parts.sublist(0, k + 1).join('/').toLowerCase();
      at.global ??= byTitle[path];
    }
  }
  // No "Namespaces" node: top-level namespaces hang straight off the centre.
  root.children.addAll(ns.children);
  final tags = TreeItem('Tags');
  for (final tag in g.tags) {
    final item = TreeItem('#${g.node(tag).title}', global: tag);
    final seen = <int>{};
    for (final e in g.adjacency[tag]) {
      if (g.kind(e) != EdgeKind.tag || g.target(e) != tag) continue;
      final s = g.source(e);
      if (seen.add(s)) {
        item.children.add(TreeItem(g.node(s).title, global: s));
      }
    }
    tags.children.add(item);
  }
  if (tags.children.isNotEmpty) root.children.add(tags);
  _measure(root, 0, null);
  _place(root, 0, 2 * math.pi);
  _colorize(root);
  return root;
}

/// Gives every arm of `t` its own sector of the hue circle (the centre's arms share it all),
/// and every sub-branch an equal slice of its parent's sector.
void _colorize(TreeItem t) {
  final k = t.children.length;
  for (var j = 0; j < k; j++) {
    final c = t.children[j];
    c.span = t.span / k;
    c.hue = t.hue - t.span / 2 + (j + 0.5) * c.span;
    c.sibling = j;
    _colorize(c);
  }
}

/// Branch colour: hue from the sector, alternating lightness so neighbours stay apart even
/// when the slices get thin.
Color treeColor(TreeItem t, Brightness b) {
  final dark = b == Brightness.dark;
  final l =
      (dark ? 0.66 : 0.48) + (t.sibling.isOdd ? (dark ? -0.08 : 0.08) : 0);
  return HSLColor.fromAHSL(1, (t.hue % 360 + 360) % 360, 0.62, l).toColor();
}

int _measure(TreeItem t, int depth, TreeItem? parent) {
  t.depth = depth;
  t.parent = parent;
  if (t.children.isEmpty) return t.leaves = 1;
  t.children.sort(
    (a, b) => a.label.toLowerCase().compareTo(b.label.toLowerCase()),
  );
  return t.leaves = t.children.fold(0, (n, c) => n + _measure(c, depth + 1, t));
}

/// Ring spacing of the seed layout (close to the force layout's link length).
const _ring = 60.0;

void _place(TreeItem t, double a0, double a1) {
  t.angle = (a0 + a1) / 2;
  final r = t.depth * _ring;
  t.pos = Offset(r * math.cos(t.angle), r * math.sin(t.angle));
  var a = a0;
  for (final c in t.children) {
    final span = (a1 - a0) * c.leaves / t.leaves;
    _place(c, a, a + span);
    a += span;
  }
}

Iterable<TreeItem> _walk(TreeItem t) sync* {
  yield t;
  for (final c in t.children) {
    yield* _walk(c);
  }
}

/// The items on screen: the root and every child of an expanded item.
Iterable<TreeItem> _shown(TreeItem t) sync* {
  yield t;
  if (!t.expanded) return;
  for (final c in t.children) {
    yield* _shown(c);
  }
}

/// Opens levels breadth-first while the canvas stays readable (about `budget` items), so a
/// small graph shows whole and a large one its top levels. Small branches (up to `small`
/// children) always open: a crowded centre must not leave them folded.
void expandInitial(TreeItem root, {int budget = 150, int small = 12}) {
  root.expanded = true;
  var shown = 1 + root.children.length;
  var level = root.children;
  while (level.isNotEmpty) {
    final next = <TreeItem>[];
    for (final t in level) {
      if (t.children.isEmpty) continue;
      if (t.children.length > small && shown + t.children.length > budget) {
        continue;
      }
      t.expanded = true;
      shown += t.children.length;
      next.addAll(t.children);
    }
    level = next;
  }
}

/// Interactive tree of hubs, namespaces and tags laid out by the force engine (the same one
/// as the network view): click a branch to expand or collapse it, click a page (or a branch's
/// name) to open it, drag nodes, wheel or pinch to zoom.
class TreeCanvas extends StatefulWidget {
  const TreeCanvas({
    super.key,
    required this.model,
    required this.onTap,
    this.name = 'Graph',
  });
  final GraphModel model;

  /// Label of the centre: the name of the notes store.
  final String name;
  final void Function(int global) onTap;

  @override
  State<TreeCanvas> createState() => TreeCanvasState();
}

class TreeCanvasState extends State<TreeCanvas> {
  late TreeItem _root;
  List<TreeItem> _items = const [];
  final _index = <TreeItem, int>{};
  LayoutEngine? _engine;
  Float32List _pos = Float32List(0);
  double _alpha = 1;
  int _generation = 0;
  int? _loopGen;
  final _repaint = ValueNotifier<int>(0);

  // Camera: screen = world * scale + offset.
  double _scale = 1;
  Offset _offset = Offset.zero;
  bool _userMoved = false;
  Size _size = Size.zero;
  double _startScale = 1;

  TreeItem? _hover;
  int? _drag;

  /// Where each label was drawn (screen space), for clicks on names.
  final _labelRects = <(Rect, TreeItem)>[];

  @visibleForTesting
  TreeItem get root => _root;

  @visibleForTesting
  int get visibleCount => _items.length;

  @override
  void initState() {
    super.initState();
    _build();
  }

  @override
  void didUpdateWidget(TreeCanvas oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.model, widget.model) ||
        oldWidget.name != widget.name) {
      _build();
    }
  }

  @override
  void dispose() {
    _generation++;
    _engine?.dispose();
    _repaint.dispose();
    super.dispose();
  }

  void _build() {
    _root = buildNamespaceTree(widget.model, name: widget.name);
    expandInitial(_root);
    _items = const [];
    _userMoved = false;
    _relayout();
  }

  /// Restarts the layout over the shown items; known items keep their place, new ones start
  /// fanned out around their parent.
  void _relayout() {
    for (final t in _items) {
      final k = _index[t]!;
      if (2 * k + 1 < _pos.length) t.pos = Offset(_pos[2 * k], _pos[2 * k + 1]);
    }
    final known = {..._items};
    _items = _shown(_root).toList();
    _index
      ..clear()
      ..addAll({for (var k = 0; k < _items.length; k++) _items[k]: k});
    final fresh = known.isEmpty;
    for (final t in _items) {
      if (fresh || known.contains(t)) continue;
      final p = t.parent!;
      final away = p.parent == null
          ? t.angle
          : (p.pos - p.parent!.pos).direction;
      final k = p.children.indexOf(t), n = p.children.length;
      final spread = p.parent == null ? math.pi : math.pi * 0.45;
      final a = away + (n == 1 ? 0 : spread * (2 * k / (n - 1) - 1));
      t.pos = p.pos + Offset.fromDirection(a, 30 + 4 * (k % 3));
    }
    final initial = Float32List(2 * _items.length);
    for (var k = 0; k < _items.length; k++) {
      initial[2 * k] = _items[k].pos.dx;
      initial[2 * k + 1] = _items[k].pos.dy;
    }
    final n = _items.length - 1;
    final s = Uint32List(n), t = Uint32List(n);
    final w = Float32List(n)..fillRange(0, n, 1);
    var e = 0;
    for (var k = 1; k < _items.length; k++) {
      s[e] = _index[_items[k].parent!]!;
      t[e++] = k;
    }
    _generation++;
    _engine?.dispose();
    _engine = AppScope.read(context).backend
        .createLayout(_items.length, s, t, w, initial: initial);

    _pos = initial;
    _alpha = 1;
    _hover = null;
    _drag = null;
    // Name hit areas belong to the old layout until the next paint.
    _labelRects.clear();
    _repaint.value++;
    // A fresh tree relaxes its radial seed harder (warm up before the first step).
    if (fresh) {
      final gen = _generation;
      _engine!.reheat(0.8).whenComplete(() {
        if (mounted && gen == _generation) _run();
      });
    } else {
      _run();
    }
  }

  Future<void> _run() async {
    if (_loopGen == _generation) return;
    _loopGen = _generation;
    final gen = _generation;
    final engine = _engine!;
    final n = _items.length;
    final iterations = n > 3000 ? 2 : (n > 800 ? 5 : 12);
    try {
      while (mounted && gen == _generation) {
        final (pos, alpha) = await engine.step(iterations);
        if (!mounted || gen != _generation) break;
        _pos = pos;
        _alpha = alpha;
        if (!_userMoved) _fit(animate: true);
        _repaint.value++;
        if (alpha == 0 && _drag == null) break;
        await SchedulerBinding.instance.endOfFrame;
      }
    } catch (e) {
      debugPrint('tree layout stopped: $e');
    } finally {
      if (_loopGen == gen) _loopGen = null;
    }
  }

  /// Whether the layout has cooled down.
  bool get settled => _alpha == 0;

  void _fit({bool animate = false}) {
    if (_size.isEmpty || _pos.isEmpty) return;
    var x0 = double.infinity, y0 = double.infinity;
    var x1 = -double.infinity, y1 = -double.infinity;
    for (var k = 0; k + 1 < _pos.length; k += 2) {
      x0 = math.min(x0, _pos[k]);
      x1 = math.max(x1, _pos[k]);
      y0 = math.min(y0, _pos[k + 1]);
      y1 = math.max(y1, _pos[k + 1]);
    }
    if (!x0.isFinite || !y0.isFinite) return;
    const pad = 70.0;
    final w = math.max(x1 - x0, 40.0), h = math.max(y1 - y0, 40.0);
    final s = math
        .min((_size.width - 2 * pad) / w, (_size.height - 2 * pad) / h)
        .clamp(0.05, 1.8);
    final o = Offset(
      _size.width / 2 - (x0 + x1) / 2 * s,
      _size.height / 2 - (y0 + y1) / 2 * s,
    );
    if (animate) {
      _scale += (s - _scale) * 0.25;
      _offset = Offset.lerp(_offset, o, 0.25)!;
    } else {
      _scale = s;
      _offset = o;
    }
  }

  Offset _toWorld(Offset screen) => (screen - _offset) / _scale;

  Offset _at(int k) => Offset(_pos[2 * k], _pos[2 * k + 1]);

  double _radius(TreeItem t) {
    if (t.parent == null) return 8;
    if (t.children.isEmpty) return 3.5;
    return math.min(4.5 + math.sqrt(t.children.length) * 0.9, 13);
  }

  int? _hitNode(Offset screen) {
    final p = _toWorld(screen);
    int? best;
    var bestD = double.infinity;
    for (var k = 0; k < _items.length && 2 * k + 1 < _pos.length; k++) {
      final d = (_at(k) - p).distanceSquared;
      final r = _radius(_items[k]) + 5 / _scale;
      if (d <= r * r && d < bestD) {
        bestD = d;
        best = k;
      }
    }
    return best;
  }

  TreeItem? _hitLabel(Offset screen) {
    for (final (r, t) in _labelRects.reversed) {
      if (r.inflate(2).contains(screen)) return t;
    }
    return null;
  }

  /// Expands or collapses `t` (a branch).
  @visibleForTesting
  void toggle(TreeItem t) {
    if (t.children.isEmpty) return;
    setState(() => t.expanded = !t.expanded);
    _relayout();
  }

  void _setAll(bool open) {
    for (final t in _walk(_root)) {
      if (t.children.isNotEmpty) t.expanded = open;
    }
    _root.expanded = true;
    _userMoved = false;
    setState(_relayout);
  }

  void _onTap(Offset local) {
    final k = _hitNode(local);
    if (k != null) {
      final t = _items[k];
      if (t.children.isNotEmpty) {
        toggle(t);
      } else if (t.global != null) {
        widget.onTap(t.global!);
      }
      return;
    }
    final l = _hitLabel(local);
    if (l?.global != null) {
      widget.onTap(l!.global!);
    } else if (l != null) {
      toggle(l);
    }
  }

  void _zoomAt(double factor, Offset focal) {
    final s = (_scale * factor).clamp(0.03, 8.0);
    final w = _toWorld(focal);
    _scale = s;
    _offset = focal - w * s;
    _userMoved = true;
    _repaint.value++;
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    if (_root.children.isEmpty) {
      return Center(
        child: Text(
          'No hubs, namespaces (a/b titles) or tags yet.',
          style: TextStyle(color: colors.muted),
        ),
      );
    }
    final total = _walk(_root).length;
    return Stack(
      children: [
        Positioned.fill(
          child: LayoutBuilder(
            builder: (context, box) {
              final size = box.biggest;
              if (size != _size) {
                _size = size;
                if (!_userMoved) _fit();
              }
              return MouseRegion(
                cursor: _hover != null
                    ? SystemMouseCursors.click
                    : MouseCursor.defer,
                onHover: (e) {
                  final k = _hitNode(e.localPosition);
                  final h = k == null ? _hitLabel(e.localPosition) : _items[k];
                  if (h != _hover) {
                    setState(() => _hover = h);
                    _repaint.value++;
                  }
                },
                onExit: (_) {
                  setState(() => _hover = null);
                  _repaint.value++;
                },
                child: Listener(
                  onPointerSignal: (e) {
                    if (e is PointerScrollEvent) {
                      GestureBinding.instance.pointerSignalResolver.register(
                        e,
                        (e) {
                          final s = e as PointerScrollEvent;
                          _zoomAt(
                            math.pow(1.0015, -s.scrollDelta.dy).toDouble(),
                            s.localPosition,
                          );
                        },
                      );
                    }
                  },
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTapUp: (d) => _onTap(d.localPosition),
                    onScaleStart: (d) {
                      _startScale = _scale;
                      _drag = d.pointerCount == 1
                          ? _hitNode(d.localFocalPoint)
                          : null;
                      if (_drag != null) {
                        final w = _toWorld(d.localFocalPoint);
                        _engine?.pin(_drag!, w.dx, w.dy);
                        _run();
                      }
                    },
                    onScaleUpdate: (d) {
                      if (_drag != null) {
                        final w = _toWorld(d.localFocalPoint);
                        _pos[2 * _drag!] = w.dx;
                        _pos[2 * _drag! + 1] = w.dy;
                        _engine?.pin(_drag!, w.dx, w.dy);
                        _repaint.value++;
                        return;
                      }
                      if (d.scale != 1) {
                        _zoomAt(
                          _startScale * d.scale / _scale,
                          d.localFocalPoint,
                        );
                      }
                      _offset += d.focalPointDelta;
                      _userMoved = true;
                      _repaint.value++;
                    },
                    onScaleEnd: (_) {
                      if (_drag != null) {
                        _engine?.unpin(_drag!);
                        _drag = null;
                        _run();
                      }
                    },
                    child: ClipRect(
                      child: CustomPaint(
                        size: size,
                        painter: _TreePainter(
                          state: this,
                          colors: colors,
                          brightness: Theme.of(context).brightness,
                          textScaler: MediaQuery.textScalerOf(context),
                        ),
                      ),
                    ),
                  ),
                ),
              );
            },
          ),
        ),
        Positioned(
          left: 16,
          bottom: 14,
          child: _TreeBar(
            shown: _items.length,
            total: total,
            onExpandAll: () => _setAll(true),
            onCollapseAll: () => _setAll(false),
            onFit: () {
              _userMoved = false;
              _fit();
              _repaint.value++;
            },
          ),
        ),
      ],
    );
  }
}

class _TreeBar extends StatelessWidget {
  const _TreeBar({
    required this.shown,
    required this.total,
    required this.onExpandAll,
    required this.onCollapseAll,
    required this.onFit,
  });
  final int shown;
  final int total;
  final VoidCallback onExpandAll;
  final VoidCallback onCollapseAll;
  final VoidCallback onFit;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    Widget button(IconData i, String tip, VoidCallback f, String key) =>
        IconButton(
          key: ValueKey(key),
          tooltip: tip,
          iconSize: 17,
          visualDensity: VisualDensity.compact,
          color: colors.muted,
          onPressed: f,
          icon: Icon(i),
        );
    return Container(
      padding: const EdgeInsets.only(left: 12, right: 4),
      decoration: BoxDecoration(
        color: colors.paper.withValues(alpha: 0.9),
        border: Border.all(color: colors.line),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Tooltip(
            message:
                'Click a branch to expand or collapse it; click a page or a '
                'name to open it. Drag nodes, scroll to zoom.',
            child: Text(
              '$shown of $total',
              style: TextStyle(fontSize: 12, color: colors.muted),
            ),
          ),
          const SizedBox(width: 4),
          button(
            Icons.unfold_more_rounded,
            'Expand all',
            onExpandAll,
            'tree-expand-all',
          ),
          button(
            Icons.unfold_less_rounded,
            'Collapse all',
            onCollapseAll,
            'tree-collapse-all',
          ),
          button(
            Icons.center_focus_strong_outlined,
            'Fit to view',
            onFit,
            'tree-fit',
          ),
        ],
      ),
    );
  }
}

class _TreePainter extends CustomPainter {
  _TreePainter({
    required this.state,
    required this.colors,
    required this.brightness,
    required this.textScaler,
  }) : super(repaint: state._repaint);
  final TreeCanvasState state;
  final SynapseColors colors;
  final Brightness brightness;
  final TextScaler textScaler;

  /// Laid-out labels per item, invalidated when the text colours change.
  static final _cache = Expando<(Color, TextPainter)>();

  @override
  void paint(Canvas canvas, Size size) {
    final s = state;
    final items = s._items;
    final pos = s._pos;
    if (items.isEmpty || pos.length < 2 * items.length) return;
    final scale = s._scale;
    Offset screen(int k) => Offset(
      pos[2 * k] * scale + s._offset.dx,
      pos[2 * k + 1] * scale + s._offset.dy,
    );
    final view = (Offset.zero & size).inflate(40);

    // Hover: the item, its path to the centre and its shown children stand out.
    final hover = s._hover;
    final lit = <TreeItem>{};
    if (hover != null) {
      for (TreeItem? t = hover; t != null; t = t.parent) {
        lit.add(t);
      }
      if (hover.expanded) lit.addAll(hover.children);
    }
    final dim = lit.isNotEmpty;

    final link = Paint()
      ..style = PaintingStyle.stroke
      ..strokeCap = StrokeCap.round;
    for (var k = 1; k < items.length; k++) {
      final t = items[k];
      final a = screen(s._index[t.parent!]!), b = screen(k);
      if (!view.contains(a) && !view.contains(b)) continue;
      final on = lit.contains(t) && lit.contains(t.parent);
      link
        ..color = treeColor(
          t,
          brightness,
        ).withValues(alpha: dim ? (on ? 0.95 : 0.12) : 0.55)
        ..strokeWidth = on ? 2.2 : 1.3;
      canvas.drawLine(a, b, link);
    }

    s._labelRects.clear();
    final occupied = <int>{};
    final many = items.length > 250;
    // Branches first so their names win the label space.
    final order = [for (var k = 0; k < items.length; k++) k]
      ..sort((a, b) {
        int pri(int k) {
          final t = items[k];
          return lit.contains(t) ? 0 : (t.children.isEmpty ? 2 : 1);
        }

        return pri(a) - pri(b);
      });
    final labels = <(int, Offset)>[];
    for (var k = 0; k < items.length; k++) {
      final t = items[k];
      final p = screen(k);
      if (!view.contains(p)) continue;
      final faded = dim && !lit.contains(t);
      final color = t.parent == null ? colors.accent : treeColor(t, brightness);
      final r = s._radius(t) * scale.clamp(0.6, 1.6);
      final fill = Paint()
        ..color = color.withValues(
          alpha:
              (t.global == null && t.parent != null ? 0.6 : 1) *
              (faded ? 0.25 : 1),
        );
      if (t.isHub) {
        _folder(canvas, p, r, fill);
      } else {
        canvas.drawCircle(p, r, fill);
      }
      // Collapsed branches: a ring and the number of hidden children.
      if (t.children.isNotEmpty && !t.expanded) {
        canvas.drawCircle(
          p,
          r + 3,
          Paint()
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1.4
            ..color = color.withValues(alpha: faded ? 0.25 : 0.8),
        );
        final badge = TextPainter(
          text: TextSpan(
            text: '+${t.children.length}',
            style: TextStyle(
              fontFamily: uiFont,
              fontSize: 9.5,
              fontWeight: FontWeight.w700,
              color: colors.muted.withValues(alpha: faded ? 0.4 : 1),
            ),
          ),
          textDirection: TextDirection.ltr,
          textScaler: textScaler,
        )..layout();
        badge.paint(canvas, p + Offset(r + 3, -r - badge.height));
      }
      if (t == hover) {
        canvas.drawCircle(
          p,
          r + 6,
          Paint()
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1.5
            ..color = colors.ink.withValues(alpha: 0.5),
        );
      }
    }
    for (final k in order) {
      final t = items[k];
      final p = screen(k);
      if (!view.contains(p)) continue;
      final branch = t.children.isNotEmpty;
      final wanted =
          lit.contains(t) ||
          t.parent == null ||
          (branch && (scale > 0.35 || !many)) ||
          (!branch && scale > (many ? 1.1 : 0.55));
      if (wanted) labels.add((k, p));
    }
    for (final (k, p) in labels) {
      final t = items[k];
      final branch = t.children.isNotEmpty;
      final ink = branch || t.parent == null ? colors.ink : colors.muted;
      var entry = _cache[t];
      if (entry == null || entry.$1 != ink) {
        entry = _cache[t] = (
          ink,
          TextPainter(
            text: TextSpan(
              text: t.label,
              style: TextStyle(
                fontFamily: uiFont,
                fontSize: t.parent == null ? 14 : (branch ? 12.5 : 11.5),
                fontWeight: t.parent == null
                    ? FontWeight.w700
                    : (branch ? FontWeight.w600 : FontWeight.w400),
                color: ink,
              ),
            ),
            textDirection: TextDirection.ltr,
            maxLines: 1,
            ellipsis: '…',
            textScaler: textScaler,
          )..layout(maxWidth: 170),
        );
      }
      final tp = entry.$2;
      final r = s._radius(t) * scale.clamp(0.6, 1.6);
      // Labels on the outer side of the node, away from its parent.
      final par = t.parent == null ? null : s._index[t.parent!];
      final left = par != null && screen(par).dx > p.dx;
      final at = t.parent == null
          ? p + Offset(-tp.width / 2, -r - tp.height - 4)
          : p + Offset(left ? -tp.width - r - 5 : r + 5, -tp.height / 2);
      final rect = at & tp.size;
      final cells = <int>[];
      for (
        var cx = (rect.left / 30).floor();
        cx <= (rect.right / 30).floor();
        cx++
      ) {
        for (
          var cy = (rect.top / 12).floor();
          cy <= (rect.bottom / 12).floor();
          cy++
        ) {
          cells.add(cx * 100003 + cy);
        }
      }
      final important = lit.contains(t) || t.parent == null;
      if (!important && cells.any(occupied.contains)) continue;
      occupied.addAll(cells);
      final faded = dim && !lit.contains(t);
      if (faded) {
        canvas.saveLayer(
          rect.inflate(2),
          Paint()..color = const Color(0x44000000),
        );
      }
      if (important && dim) {
        canvas.drawRRect(
          RRect.fromRectAndRadius(rect.inflate(3), const Radius.circular(4)),
          Paint()..color = colors.paper.withValues(alpha: 0.85),
        );
      }
      tp.paint(canvas, at);
      if (faded) canvas.restore();
      s._labelRects.add((rect, t));
    }
  }

  /// Small folder glyph (body plus tab) centred on `c`, about `r` in radius.
  void _folder(Canvas canvas, Offset c, double r, Paint paint) {
    final w = r * 2.4, h = r * 1.7;
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromCenter(center: c.translate(0, r * 0.15), width: w, height: h),
        Radius.circular(r * 0.35),
      ),
      paint,
    );
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(c.dx - w / 2, c.dy - h / 2 - r * 0.45, w * 0.45, r * 0.6),
        Radius.circular(r * 0.2),
      ),
      paint,
    );
  }

  @override
  bool shouldRepaint(_TreePainter old) =>
      old.colors != colors ||
      old.brightness != brightness ||
      !identical(old.state, state);
}
