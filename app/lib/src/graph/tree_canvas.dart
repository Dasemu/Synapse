import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../theme/theme.dart';
import 'graph_model.dart';
import 'network_canvas.dart';

/// A node of the namespace / tag tree.
class TreeItem {
  TreeItem(this.label, {this.global});
  final String label;

  /// Page index when the item is a page.
  int? global;
  final children = <TreeItem>[];
  int depth = 0;
  int leaves = 1;
  double angle = 0;
  Offset pos = Offset.zero;

  TreeItem child(String label) {
    for (final c in children) {
      if (c.label == label) return c;
    }
    final c = TreeItem(label);
    children.add(c);
    return c;
  }
}

/// Builds a tree with two branches: namespaces (`a/b/c` titles split on `/`) and tags (each
/// tag with the pages using it, most used tags first).
TreeItem buildNamespaceTree(
  GraphModel g, {
  int maxTags = 24,
  int maxPerTag = 30,
}) {
  final root = TreeItem('Graph');
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
  if (ns.children.isNotEmpty) root.children.add(ns);
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
  // With a single branch, that branch is the centre.
  final top = root.children.length == 1 ? root.children.single : root;
  _measure(top, 0);
  _place(top, 0, 2 * math.pi);
  return top;
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

Iterable<TreeItem> _walk(TreeItem t) sync* {
  yield t;
  for (final c in t.children) {
    yield* _walk(c);
  }
}

/// Radial tree of namespaces and tags; pan/zoom with the GPU transform (no repaint).
class TreeCanvas extends StatefulWidget {
  const TreeCanvas({super.key, required this.model, required this.onTap});
  final GraphModel model;
  final void Function(int global) onTap;

  @override
  State<TreeCanvas> createState() => _TreeCanvasState();
}

class _TreeCanvasState extends State<TreeCanvas> {
  late TreeItem _root = buildNamespaceTree(widget.model);
  final _transform = TransformationController();
  bool _placed = false;

  @override
  void didUpdateWidget(TreeCanvas oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.model, widget.model)) {
      _root = buildNamespaceTree(widget.model);
    }
  }

  @override
  void dispose() {
    _transform.dispose();
    super.dispose();
  }

  double get _extent {
    final maxDepth = _walk(_root).fold(0, (m, t) => math.max(m, t.depth));
    return maxDepth * _ring + 170;
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    if (_root.children.isEmpty) {
      return Center(
        child: Text(
          'No namespaces (a/b titles) or tags yet.',
          style: TextStyle(color: colors.muted),
        ),
      );
    }
    final extent = _extent;
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
              box.maxWidth / 2 - extent * s,
              box.maxHeight / 2 - extent * s,
              0,
              1,
            )
            ..scaleByDouble(s, s, 1, 1);
        }
        return InteractiveViewer(
          transformationController: _transform,
          constrained: false,
          minScale: 0.05,
          maxScale: 6,
          boundaryMargin: const EdgeInsets.all(double.infinity),
          child: GestureDetector(
            onTapUp: (d) {
              final p = d.localPosition - Offset(extent, extent);
              for (final t in _walk(_root)) {
                if (t.global != null && (t.pos - p).distance < 10) {
                  widget.onTap(t.global!);
                  return;
                }
              }
            },
            child: CustomPaint(
              size: Size(2 * extent, 2 * extent),
              painter: _TreePainter(
                _root,
                extent,
                colors,
                graphPalette(Theme.of(context).brightness),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _TreePainter extends CustomPainter {
  _TreePainter(this.root, this.extent, this.colors, this.palette);
  final TreeItem root;
  final double extent;
  final SynapseColors colors;
  final List<Color> palette;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.translate(extent, extent);
    final link = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.2
      ..color = colors.line;
    for (final t in _walk(root)) {
      for (final c in t.children) {
        // Curve from parent to child bending through the child's angle.
        final mid = Offset.fromDirection(
          c.angle,
          t.depth * _ring + _ring * 0.5,
        );
        canvas.drawPath(
          Path()
            ..moveTo(t.pos.dx, t.pos.dy)
            ..quadraticBezierTo(mid.dx, mid.dy, c.pos.dx, c.pos.dy),
          link,
        );
      }
    }
    var branch = 0;
    // Colour per first-level branch; with a single-branch tree each subtree gets its own.
    for (final b in root.children) {
      final color = palette[branch++ % palette.length];
      for (final t in _walk(b)) {
        final r = t.children.isEmpty ? 3.5 : 5.5;
        canvas.drawCircle(
          t.pos,
          r,
          Paint()..color = t.global == null ? colors.faint : color,
        );
        final tp = TextPainter(
          text: TextSpan(
            text: t.label,
            style: TextStyle(
              fontFamily: uiFont,
              fontSize: t.children.isEmpty ? 11 : 12.5,
              fontWeight: t.children.isEmpty
                  ? FontWeight.w400
                  : FontWeight.w600,
              color: t.children.isEmpty ? colors.muted : colors.ink,
            ),
          ),
          textDirection: TextDirection.ltr,
          maxLines: 1,
          ellipsis: '…',
        )..layout(maxWidth: 150);
        // Labels outside the circle, left of the node on the left half.
        final left = math.cos(t.angle) < 0;
        final at = t.pos + Offset(left ? -tp.width - 8 : 8, -tp.height / 2);
        tp.paint(canvas, at);
      }
    }
    canvas.drawCircle(Offset.zero, 7, Paint()..color = colors.accent);
  }

  @override
  bool shouldRepaint(_TreePainter old) =>
      !identical(old.root, root) || old.colors != colors;
}
