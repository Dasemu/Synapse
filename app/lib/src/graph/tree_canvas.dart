import 'dart:math' as math;

import 'package:flutter/material.dart';

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
/// recursively, plus the notes that live in no hub (orphans included). A hub is expanded once (the first time it is reached) and never inside
/// itself, so cycles and shared sub-hubs stay bounded; later occurrences are leaves.
List<TreeItem> _hubBranches(GraphModel g, int maxPerHub, int maxLoose) {
  final branches = <TreeItem>[];
  final members = g.hubMembers;
  final nested = {for (final m in members.values) ...m};
  final expanded = <int>{};
  TreeItem build(int hub) {
    final item = TreeItem(g.node(hub).title, global: hub, isHub: true);
    if (!expanded.add(hub)) return item;
    var shown = 0;
    final all = members[hub] ?? const <int>{};
    // Sub-hubs are always listed; plain pages are capped.
    for (final m in all) {
      if (g.isHub(m)) {
        item.children.add(build(m));
      } else if (shown < maxPerHub) {
        shown++;
        item.children.add(TreeItem(g.node(m).title, global: m));
      }
    }
    final hidden = all.where((m) => !g.isHub(m)).length - shown;
    if (hidden > 0) item.children.add(TreeItem('… $hidden more'));
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
  final loose = [
    for (var i = 0; i < g.nodeCount; i++)
      if (!g.isHub(i) &&
          !g.isJournal(i) &&
          !nested.contains(i) &&
          !g.node(i).title.contains('/'))
        i,
  ];
  for (final i in loose.take(maxLoose)) {
    branches.add(TreeItem(g.node(i).title, global: i));
  }
  if (loose.length > maxLoose) {
    branches.add(TreeItem('… ${loose.length - maxLoose} more'));
  }
  return branches;
}

/// Builds a tree from hubs (folders, straight off the centre, with loose notes), namespaces (`a/b/c` titles split on `/`, attached
/// directly to the centre) and tags (each tag with the pages using it, most used tags first).
TreeItem buildNamespaceTree(
  GraphModel g, {
  String name = 'Graph',
  int maxTags = 24,
  int maxPerTag = 30,
  int maxPerHub = 40,
  int maxLoose = 80,
}) {
  final root = TreeItem(name);
  root.children.addAll(_hubBranches(g, maxPerHub, maxLoose));
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
  for (final tag in g.tags.take(maxTags)) {
    final item = TreeItem('#${g.node(tag).title}', global: tag);
    final seen = <int>{};
    for (final e in g.adjacency[tag]) {
      if (g.kind(e) != EdgeKind.tag || g.target(e) != tag) continue;
      final s = g.source(e);
      if (seen.add(s) && seen.length <= maxPerTag) {
        item.children.add(TreeItem(g.node(s).title, global: s));
      }
    }
    tags.children.add(item);
  }
  if (tags.children.isNotEmpty) root.children.add(tags);
  _measure(root, 0);
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

int _measure(TreeItem t, int depth) {
  t.depth = depth;
  if (t.children.isEmpty) return t.leaves = 1;
  t.children.sort(
    (a, b) => a.label.toLowerCase().compareTo(b.label.toLowerCase()),
  );
  return t.leaves = t.children.fold(0, (n, c) => n + _measure(c, depth + 1));
}

const _ring = 120.0;

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

void _shift(TreeItem t, Offset d) {
  for (final x in _walk(t)) {
    x.pos += d;
  }
}

class _Tick extends ChangeNotifier {
  void bump() => notifyListeners();
}

Iterable<TreeItem> _walk(TreeItem t) sync* {
  yield t;
  for (final c in t.children) {
    yield* _walk(c);
  }
}

/// Radial tree of hubs, namespaces and tags; pan/zoom with the GPU transform (no repaint).
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
  State<TreeCanvas> createState() => _TreeCanvasState();
}

class _TreeCanvasState extends State<TreeCanvas> {
  late TreeItem _root = buildNamespaceTree(widget.model, name: widget.name);
  final _tick = _Tick();
  TreeItem? _drag;
  bool _pan = true;
  final _transform = TransformationController();
  bool _placed = false;

  @override
  void didUpdateWidget(TreeCanvas oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.model, widget.model) ||
        oldWidget.name != widget.name) {
      _root = buildNamespaceTree(widget.model, name: widget.name);
    }
  }

  @override
  void dispose() {
    _transform.dispose();
    _tick.dispose();
    super.dispose();
  }

  double get _extent {
    final maxDepth = _walk(_root).fold(0, (m, t) => math.max(m, t.depth));
    return maxDepth * _ring + 170;
  }

  /// Nodes can be dragged away from the rings, so the scene is larger than the layout.
  double get _half => _extent * 1.6;

  TreeItem? _hit(Offset scene, double half) {
    final p = scene - Offset(half, half);
    TreeItem? best;
    var d = 12.0;
    for (final t in _walk(_root)) {
      final x = (t.pos - p).distance;
      if (x < d) {
        d = x;
        best = t;
      }
    }
    return best;
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
    final extent = _extent;
    final half = _half;
    return LayoutBuilder(
      builder: (context, box) {
        if (!_placed) {
          _placed = true;
          final s = math.min(
            1.0,
            math.min(box.maxWidth, box.maxHeight) / (2 * extent),
          );
          _transform.value = Matrix4.identity()
            ..translateByDouble(
              box.maxWidth / 2 - half * s,
              box.maxHeight / 2 - half * s,
              0,
              1,
            )
            ..scaleByDouble(s, s, 1, 1);
        }
        return InteractiveViewer(
          transformationController: _transform,
          constrained: false,
          panEnabled: _pan,
          minScale: 0.05,
          maxScale: 6,
          boundaryMargin: const EdgeInsets.all(double.infinity),
          child: Listener(
            // Dragging a node moves it with its whole branch; empty space pans the view.
            onPointerDown: (e) {
              final t = _hit(e.localPosition, half);
              if (t != null) {
                _drag = t;
                setState(() => _pan = false);
              }
            },
            onPointerMove: (e) {
              final t = _drag;
              if (t == null) return;
              _shift(t, e.delta / _transform.value.getMaxScaleOnAxis());
              _tick.bump();
            },
            onPointerUp: (_) => _release(),
            onPointerCancel: (_) => _release(),
            child: GestureDetector(
              onTapUp: (d) {
                final t = _hit(d.localPosition, half);
                if (t?.global != null) widget.onTap(t!.global!);
              },
              child: CustomPaint(
                size: Size(2 * half, 2 * half),
                painter: _TreePainter(
                  _root,
                  half,
                  colors,
                  Theme.of(context).brightness,
                  _tick,
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  void _release() {
    if (_drag == null) return;
    _drag = null;
    setState(() => _pan = true);
  }
}

class _TreePainter extends CustomPainter {
  _TreePainter(
    this.root,
    this.extent,
    this.colors,
    this.brightness,
    Listenable tick,
  ) : super(repaint: tick);
  final TreeItem root;
  final double extent;
  final SynapseColors colors;
  final Brightness brightness;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.translate(extent, extent);
    final link = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.4;
    for (final t in _walk(root)) {
      for (final c in t.children) {
        // Curve from parent to child bending through the child's ray.
        final mid = c.pos - Offset.fromDirection(c.angle, _ring * 0.5);
        link.color = treeColor(c, brightness).withValues(alpha: 0.7);
        canvas.drawPath(
          Path()
            ..moveTo(t.pos.dx, t.pos.dy)
            ..quadraticBezierTo(mid.dx, mid.dy, c.pos.dx, c.pos.dy),
          link,
        );
      }
    }
    void node(TreeItem t, Color color) {
      final r = t.children.isEmpty ? 3.5 : 5.5;
      final fill = Paint()
        ..color = t.global == null ? color.withValues(alpha: 0.55) : color;
      if (t.isHub) {
        _folder(canvas, t.pos, fill);
      } else {
        canvas.drawCircle(t.pos, r, fill);
      }
      final tp = TextPainter(
        text: TextSpan(
          text: t.label,
          style: TextStyle(
            fontFamily: uiFont,
            fontSize: t.children.isEmpty ? 11 : 12.5,
            fontWeight: t.children.isEmpty ? FontWeight.w400 : FontWeight.w600,
            color: t.children.isEmpty ? colors.muted : colors.ink,
          ),
        ),
        textDirection: TextDirection.ltr,
        maxLines: 1,
        ellipsis: '…',
      )..layout(maxWidth: 150);
      // Labels outside the circle, left of the node on the left half.
      final left = math.cos(t.angle) < 0;
      tp.paint(
        canvas,
        t.pos + Offset(left ? -tp.width - 8 : 8, -tp.height / 2),
      );
    }

    for (final t in _walk(root).skip(1)) {
      node(t, treeColor(t, brightness));
    }
    // The centre carries the name of the notes store.
    final c = TextPainter(
      text: TextSpan(
        text: root.label,
        style: TextStyle(
          fontFamily: uiFont,
          fontSize: 14,
          fontWeight: FontWeight.w700,
          color: colors.ink,
        ),
      ),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: 180);
    c.paint(canvas, root.pos + Offset(-c.width / 2, -c.height - 12));
    canvas.drawCircle(root.pos, 7, Paint()..color = colors.accent);
  }

  /// Small folder glyph (body plus tab) centred on `c`.
  void _folder(Canvas canvas, Offset c, Paint paint) {
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromCenter(center: c.translate(0, 1), width: 13, height: 9),
        const Radius.circular(2),
      ),
      paint,
    );
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(c.dx - 6.5, c.dy - 5, 6, 3),
        const Radius.circular(1.2),
      ),
      paint,
    );
  }

  @override
  bool shouldRepaint(_TreePainter old) =>
      !identical(old.root, root) ||
      old.colors != colors ||
      old.brightness != brightness;
}
