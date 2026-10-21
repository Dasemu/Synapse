import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';
import 'graph_model.dart';

enum ColorBy { cluster, tag, none }

/// Calm categorical palette (cluster / tag colours), tuned for each theme.
List<Color> graphPalette(Brightness b) => b == Brightness.light
    ? const [
        Color(0xFF1F7A74),
        Color(0xFFB0741E),
        Color(0xFF5B6BB5),
        Color(0xFFA2536B), //
        Color(0xFF5E8F4A),
        Color(0xFF7A5BA6),
        Color(0xFF2F86A8),
        Color(0xFFB35A3C),
        Color(0xFF8A8A3A), Color(0xFF4F7A8F),
      ]
    : const [
        Color(0xFF63C2B8),
        Color(0xFFD9A55A),
        Color(0xFF8E9BE0),
        Color(0xFFD9839B), //
        Color(0xFF93C27E),
        Color(0xFFAE90D6),
        Color(0xFF6FB8D6),
        Color(0xFFE08D6E),
        Color(0xFFC4C46E), Color(0xFF86AFC4),
      ];

/// Remembers node positions by page id across views, so switching modes or filters keeps
/// the picture stable.
class PositionCache {
  final _pos = <String, Offset>{};
  Offset? operator [](String id) => _pos[id];
  void operator []=(String id, Offset p) => _pos[id] = p;
}

/// Extra per-node data for the synapse (brain) mode.
class BrainOverlay {
  const BrainOverlay({
    required this.focus,
    required this.activation,
    required this.hops,
  });

  /// Global index of the focused node.
  final int focus;

  /// Global index → activation 0..1.
  final Map<int, double> activation;

  /// Global index → hop distance from the focus (drives the pulse).
  final Map<int, int> hops;
}

/// A force-directed drawing of a [SubGraph]: Rust lays it out off the UI thread, this widget
/// paints culled batches (one draw call per edge kind and node colour/size bucket), and
/// handles pan/zoom, node drag, hover highlighting and taps.
class NetworkCanvas extends StatefulWidget {
  const NetworkCanvas({
    super.key,
    required this.sub,
    required this.onTap,
    this.colorBy = ColorBy.cluster,
    this.hulls = false,
    this.focus,
    this.brain,
    this.cache,
    this.compact = false,
  });

  final SubGraph sub;
  final void Function(int global) onTap;
  final ColorBy colorBy;
  final bool hulls;

  /// Highlighted node (local graph, brain).
  final int? focus;
  final BrainOverlay? brain;
  final PositionCache? cache;

  /// Side-panel use: fewer labels, no legend.
  final bool compact;

  @override
  State<NetworkCanvas> createState() => NetworkCanvasState();
}

class NetworkCanvasState extends State<NetworkCanvas>
    with SingleTickerProviderStateMixin {
  LayoutEngine? _engine;
  Float32List _pos = Float32List(0);
  double _alpha = 1;

  /// Generation whose layout loop is running, if any.
  int? _loopGen;
  int _generation = 0;
  final _repaint = ValueNotifier<int>(0);

  // Camera: screen = world * scale + offset.
  double _scale = 1;
  Offset _offset = Offset.zero;
  bool _userMoved = false;
  Size _size = Size.zero;

  int? _hover;
  int? _drag;

  // Reused point buffers (no per-frame allocation or boxing).
  final _edgeBufs = <EdgeKind, _Buf>{};
  final _nodeBufs = <(int, int), _Buf>{};
  Float32List _meshPos = Float32List(0);
  Int32List _meshCol = Int32List(0);
  double _startScale = 1;
  late final AnimationController _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1800),
  )..addListener(() => _repaint.value++);

  // Per-node style, rebuilt with the subgraph.
  late Float32List _radius;
  late List<int> _colorIndex;
  late List<List<int>> _localAdj;

  @visibleForTesting
  int get nodeCount => widget.sub.length;

  /// Whether the layout has cooled down.
  bool get settled => _alpha == 0;

  @override
  void initState() {
    super.initState();
    _rebuild();
  }

  @override
  void didUpdateWidget(NetworkCanvas oldWidget) {
    final old = oldWidget;
    super.didUpdateWidget(old);
    if (!identical(old.sub, widget.sub)) {
      _saveCache(old.sub);
      _rebuild();
    } else if (old.colorBy != widget.colorBy) {
      _styles();
    }
    if (widget.brain != null && old.brain?.focus != widget.brain!.focus) {
      _pulse.forward(from: 0);
    }
  }

  @override
  void dispose() {
    _saveCache(widget.sub);
    _generation++;
    _engine?.dispose();
    _pulse.dispose();
    _repaint.dispose();
    super.dispose();
  }

  /// Stores the current positions, which belong to `sub` (the subgraph being replaced).
  void _saveCache(SubGraph sub) {
    final c = widget.cache;
    if (c == null || _pos.length != 2 * sub.length) return;
    for (var k = 0; k < sub.length && 2 * k + 1 < _pos.length; k++) {
      c[sub.model.node(sub.nodes[k]).id] = Offset(_pos[2 * k], _pos[2 * k + 1]);
    }
  }

  void _styles() {
    final sub = widget.sub;
    final g = sub.model;
    _radius = Float32List(sub.length);
    _colorIndex = List.filled(sub.length, -1);
    for (var k = 0; k < sub.length; k++) {
      final n = g.node(sub.nodes[k]);
      _radius[k] = (3.0 + math.sqrt(n.degree + n.backlinks) * 1.7).clamp(
        3.0,
        22.0,
      );
      _colorIndex[k] = switch (widget.colorBy) {
        ColorBy.cluster => n.cluster < 10 ? n.cluster : -1,
        ColorBy.tag =>
          n.primaryTag == null
              ? -1
              : (() {
                  final i = g.tags.indexOf(n.primaryTag!);
                  return i < 10 ? i : -1;
                })(),
        ColorBy.none => -1,
      };
    }
  }

  void _rebuild() {
    _engine?.dispose();
    _generation++;
    final sub = widget.sub;
    _styles();
    _localAdj = List.generate(sub.length, (_) => <int>[]);
    for (final e in sub.edges) {
      final a = sub.local[sub.model.source(e)]!;
      final b = sub.local[sub.model.target(e)]!;
      _localAdj[a].add(b);
      _localAdj[b].add(a);
    }
    final cache = widget.cache;
    Float32List? initial;
    if (cache != null) {
      var hits = 0;
      final p = Float32List(2 * sub.length);
      for (var k = 0; k < sub.length; k++) {
        final c = cache[sub.model.node(sub.nodes[k]).id];
        if (c != null) {
          hits++;
          p[2 * k] = c.dx;
          p[2 * k + 1] = c.dy;
        } else {
          // New nodes start near a known neighbour, or spread around the origin.
          final nb = _localAdj[k].map(
            (j) => cache[sub.model.node(sub.nodes[j]).id],
          );
          final anchor = nb.firstWhere((o) => o != null, orElse: () => null);
          final a = k * 2.399963;
          final r = anchor == null ? 30.0 * math.sqrt(k + 1) : 18.0;
          p[2 * k] = (anchor?.dx ?? 0) + r * math.cos(a);
          p[2 * k + 1] = (anchor?.dy ?? 0) + r * math.sin(a);
        }
      }
      if (hits > sub.length / 3) initial = p;
    }
    final (s, t, w) = sub.layoutEdges();
    final backend = AppScope.read(context).backend;
    _engine = backend.createLayout(sub.length, s, t, w, initial: initial);
    _pos = initial ?? Float32List(2 * sub.length);
    _alpha = 1;
    _hover = null;
    _drag = null;
    _run();
    if (widget.brain != null) _pulse.forward(from: 0);
  }

  Future<void> _run() async {
    if (_loopGen == _generation) return;
    _loopGen = _generation;
    final gen = _generation;
    final engine = _engine!;
    final n = widget.sub.length;
    // Keep each batch around a frame's worth of work.
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
      debugPrint('layout stopped: $e');
    } finally {
      if (_loopGen == gen) _loopGen = null;
    }
  }

  /// Fits the camera to the nodes (eased while the layout settles).
  void _fit({bool animate = false}) {
    if (_size.isEmpty || _pos.isEmpty) return;
    // Robust bounds: a few far-flung orphans should not shrink the main body to a dot.
    final xs = <double>[], ys = <double>[];
    // Sample large graphs: bounds need percentiles, not every point.
    final stride = 2 * math.max(1, _pos.length ~/ 3000).toInt();
    for (var k = 0; k + 1 < _pos.length; k += stride) {
      if (_pos[k].isFinite && _pos[k + 1].isFinite) {
        xs.add(_pos[k]);
        ys.add(_pos[k + 1]);
      }
    }
    if (xs.isEmpty) return;
    xs.sort();
    ys.sort();
    final cut = xs.length > 40 ? (xs.length * 0.03).floor() : 0;
    final x0 = xs[cut], x1 = xs[xs.length - 1 - cut];
    final y0 = ys[cut], y1 = ys[ys.length - 1 - cut];
    final pad = widget.compact ? 16.0 : (widget.sub.length < 60 ? 110.0 : 60.0);
    final w = math.max(x1 - x0, 40.0), h = math.max(y1 - y0, 40.0);
    final s = math
        .min((_size.width - 2 * pad) / w, (_size.height - 2 * pad) / h)
        .clamp(0.05, 1.6);
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

  int? _hit(Offset screen) {
    final p = _toWorld(screen);
    int? best;
    var bestD = double.infinity;
    for (var k = 0; k < widget.sub.length && 2 * k + 1 < _pos.length; k++) {
      final dx = _pos[2 * k] - p.dx, dy = _pos[2 * k + 1] - p.dy;
      final d = dx * dx + dy * dy;
      final r = _radius[k] + 4 / _scale;
      if (d <= r * r && d < bestD) {
        bestD = d;
        best = k;
      }
    }
    return best;
  }

  void _zoomAt(double factor, Offset focal) {
    final s = (_scale * factor).clamp(0.03, 8.0);
    final w = _toWorld(focal);
    _scale = s;
    _offset = focal - w * s;
    _userMoved = true;
    _repaint.value++;
  }

  /// Re-fits and lets the camera follow the layout again.
  void recenter() {
    _userMoved = false;
    _fit();
    _repaint.value++;
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final palette = graphPalette(Theme.of(context).brightness);
    return LayoutBuilder(
      builder: (context, box) {
        final size = box.biggest;
        if (size != _size) {
          _size = size;
          if (!_userMoved) _fit();
        }
        return MouseRegion(
          cursor: _hover != null ? SystemMouseCursors.click : MouseCursor.defer,
          onHover: (e) {
            final h = _hit(e.localPosition);
            if (h != _hover) setState(() => _hover = h);
          },
          onExit: (_) => setState(() => _hover = null),
          child: Listener(
            onPointerSignal: (e) {
              if (e is PointerScrollEvent) {
                // Claim the wheel so an enclosing scroll view does not scroll too.
                GestureBinding.instance.pointerSignalResolver.register(e, (e) {
                  final s = e as PointerScrollEvent;
                  _zoomAt(
                    math.pow(1.0015, -s.scrollDelta.dy).toDouble(),
                    s.localPosition,
                  );
                });
              }
            },
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapUp: (d) {
                final h = _hit(d.localPosition);
                if (h != null) widget.onTap(widget.sub.nodes[h]);
              },
              onScaleStart: (d) {
                _startScale = _scale;
                _drag = d.pointerCount == 1 ? _hit(d.localFocalPoint) : null;
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
                  _zoomAt(_startScale * d.scale / _scale, d.localFocalPoint);
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
                  painter: _NetworkPainter(
                    state: this,
                    colors: colors,
                    palette: palette,
                    textScaler: MediaQuery.textScalerOf(context),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _NetworkPainter extends CustomPainter {
  _NetworkPainter({
    required this.state,
    required this.colors,
    required this.palette,
    required this.textScaler,
  }) : super(repaint: state._repaint);

  final NetworkCanvasState state;
  final SynapseColors colors;
  final List<Color> palette;
  final TextScaler textScaler;

  /// Laid-out labels per subgraph, invalidated when the text colour changes.
  static final _labels = Expando<(Color, Map<int, TextPainter>)>();

  Color _edgeColor(EdgeKind k) => switch (k) {
    EdgeKind.link => colors.muted,
    EdgeKind.tag => colors.accent,
    EdgeKind.property => palette[2],
    EdgeKind.blockRef => colors.ref,
    EdgeKind.embed => colors.ref,
    EdgeKind.mention => colors.faint,
    EdgeKind.synapse => palette[3],
  };

  @override
  void paint(Canvas canvas, Size size) {
    final s = state;
    final sub = s.widget.sub;
    final g = sub.model;
    final pos = s._pos;
    if (pos.length < 2 * sub.length || sub.length == 0) return;
    final scale = s._scale;
    final brain = s.widget.brain;
    final hover = s._hover;
    final focusLocal = s.widget.focus == null
        ? null
        : sub.local[s.widget.focus!];
    final highlight = <int>{
      if (hover != null) ...[hover, ...s._localAdj[hover]],
    };
    final dim = hover != null;

    // Visible world rect.
    final view = Rect.fromLTWH(
      -s._offset.dx / scale,
      -s._offset.dy / scale,
      size.width / scale,
      size.height / scale,
    ).inflate(30 / scale);
    bool visible(int k) => view.contains(Offset(pos[2 * k], pos[2 * k + 1]));

    canvas.save();
    canvas.translate(s._offset.dx, s._offset.dy);
    canvas.scale(scale);

    // Cluster hulls.
    if (s.widget.hulls) _hulls(canvas, sub, pos, scale);

    // Edges, batched by kind.
    final pulse = s._pulse.value;
    final maxHop = brain == null ? 0 : brain.hops.values.fold(0, math.max);
    final front = pulse * (maxHop + 1);
    final batches = s._edgeBufs;
    for (final b in batches.values) {
      b.n = 0;
    }
    final strong = <(int, int, EdgeKind, double)>[];
    for (final e in sub.edges) {
      final a = sub.local[g.source(e)]!, b = sub.local[g.target(e)]!;
      final ax = pos[2 * a],
          ay = pos[2 * a + 1],
          bx = pos[2 * b],
          by = pos[2 * b + 1];
      if (math.max(ax, bx) < view.left ||
          math.min(ax, bx) > view.right ||
          math.max(ay, by) < view.top ||
          math.min(ay, by) > view.bottom) {
        continue;
      }
      final k = g.kind(e);
      if (brain != null ||
          (dim &&
              highlight.contains(a) &&
              highlight.contains(b) &&
              (a == hover || b == hover))) {
        strong.add((a, b, k, g.weight(e)));
        continue;
      }
      final list = batches.putIfAbsent(k, _Buf.new);
      if (k == EdgeKind.mention) {
        _dashes(list, ax, ay, bx, by, 5 / scale);
      } else {
        list.add4(ax, ay, bx, by);
      }
    }
    _edgeMesh(canvas, batches, scale, dim);
    for (final (a, b, k, w) in strong) {
      final p = Offset(pos[2 * a], pos[2 * a + 1]),
          q = Offset(pos[2 * b], pos[2 * b + 1]);
      if (brain != null) {
        final aa = brain.activation[sub.nodes[a]] ?? 0,
            ab = brain.activation[sub.nodes[b]] ?? 0;
        final act = math.max(aa, ab);
        final width =
            (k == EdgeKind.synapse ? 1 + w.clamp(0, 10) * 0.6 : 1.0) / scale;
        canvas.drawLine(
          p,
          q,
          Paint()
            ..color = (k == EdgeKind.synapse ? palette[3] : colors.faint)
                .withValues(alpha: 0.15 + 0.6 * act)
            ..strokeWidth = width
            ..strokeCap = StrokeCap.round,
        );
        // Pulse travelling outward along edges from hop h to h + 1.
        final ha = brain.hops[sub.nodes[a]], hb = brain.hops[sub.nodes[b]];
        if (ha != null && hb != null && ha != hb && pulse > 0 && pulse < 1) {
          final (from, to, h) = ha < hb ? (p, q, ha) : (q, p, hb);
          final t = (front - h).clamp(0.0, 1.0);
          if (t > 0 && t < 1) {
            canvas.drawLine(
              from,
              Offset.lerp(from, to, t)!,
              Paint()
                ..color = colors.accent.withValues(alpha: 0.9)
                ..strokeWidth = width + 1.5 / scale
                ..strokeCap = StrokeCap.round,
            );
          }
        }
      } else {
        canvas.drawLine(
          p,
          q,
          Paint()
            ..color = _edgeColor(k).withValues(alpha: 0.9)
            ..strokeWidth = 1.6 / scale,
        );
      }
    }

    // Nodes, batched by colour and radius (one drawRawPoints per bucket).
    final buckets = s._nodeBufs;
    for (final b in buckets.values) {
      b.n = 0;
    }
    final journalColor = colors.faint;
    for (var k = 0; k < sub.length; k++) {
      if (!visible(k)) continue;
      final r = brain == null
          ? s._radius[k]
          : 3 + 9 * (brain.activation[sub.nodes[k]] ?? 0);
      final ci = g.isJournal(sub.nodes[k]) ? -2 : s._colorIndex[k];
      final key = (ci, (r * 2).round());
      buckets.putIfAbsent(key, _Buf.new).add2(pos[2 * k], pos[2 * k + 1]);
    }
    for (final MapEntry(key: (ci, d2), value: pts) in buckets.entries) {
      if (pts.n == 0) continue;
      var c = ci == -2 ? journalColor : (ci < 0 ? colors.muted : palette[ci]);
      if (dim) c = c.withValues(alpha: 0.25);
      if (brain != null) c = colors.faint;
      canvas.drawRawPoints(
        ui.PointMode.points,
        pts.view,
        Paint()
          ..color = c
          ..strokeWidth = d2.toDouble()
          ..strokeCap = StrokeCap.round,
      );
    }
    // Activation glow (brain), highlighted and focused nodes on top.
    if (brain != null) {
      for (var k = 0; k < sub.length; k++) {
        final act = brain.activation[sub.nodes[k]] ?? 0;
        final c = Offset(pos[2 * k], pos[2 * k + 1]);
        final r = 3 + 9 * act;
        canvas.drawCircle(
          c,
          r,
          Paint()
            ..color = Color.lerp(colors.faint, colors.accent, act.clamp(0, 1))!,
        );
        final h = brain.hops[sub.nodes[k]];
        if (h != null && pulse > 0 && pulse < 1) {
          final d = (front - h).abs();
          if (d < 0.6) {
            canvas.drawCircle(
              c,
              r + (6 + 10 * (0.6 - d)) / scale,
              Paint()
                ..style = PaintingStyle.stroke
                ..strokeWidth = 2 / scale
                ..color = colors.accent.withValues(alpha: (0.6 - d) / 0.6),
            );
          }
        }
      }
    }
    for (final k in highlight) {
      final ci = s._colorIndex[k];
      canvas.drawCircle(
        Offset(pos[2 * k], pos[2 * k + 1]),
        s._radius[k],
        Paint()..color = ci < 0 ? colors.ink : palette[ci],
      );
    }
    if (focusLocal != null) {
      canvas.drawCircle(
        Offset(pos[2 * focusLocal], pos[2 * focusLocal + 1]),
        (brain == null ? s._radius[focusLocal] : 12) + 4 / scale,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2 / scale
          ..color = colors.accent,
      );
    }
    canvas.restore();

    _paintLabels(canvas, size, sub, pos, highlight, focusLocal);
  }

  /// All plain edges as one triangle mesh with per-vertex colours: a single draw call
  /// and no per-frame stroke tessellation (which dominates raster time with 10k+ lines).
  void _edgeMesh(
    Canvas canvas,
    Map<EdgeKind, _Buf> batches,
    double scale,
    bool dim,
  ) {
    var segs = 0;
    for (final b in batches.values) {
      segs += b.n ~/ 4;
    }
    if (segs == 0) return;
    final s = state;
    if (s._meshPos.length < segs * 12) {
      s._meshPos = Float32List(segs * 24);
      s._meshCol = Int32List(segs * 12);
    }
    final pos = s._meshPos, col = s._meshCol;
    var p = 0, c = 0;
    // Dense graphs fade edges so structure shows instead of a solid hairball.
    final density = math.sqrt(math.min(1.0, 2500 / segs));
    for (final MapEntry(key: k, value: b) in batches.entries) {
      if (b.n == 0) continue;
      final color = _edgeColor(k)
          .withValues(
            alpha: (dim ? 0.12 : (k == EdgeKind.link ? 0.4 : 0.55)) * density,
          )
          .toARGB32()
          .toSigned(32);
      final half = (k == EdgeKind.tag ? 1.3 : 1.1) / scale / 2;
      final d = b.data;
      for (var i = 0; i + 3 < b.n; i += 4) {
        final ax = d[i], ay = d[i + 1], bx = d[i + 2], by = d[i + 3];
        final dx = bx - ax, dy = by - ay;
        final len = math.sqrt(dx * dx + dy * dy);
        if (len == 0) continue;
        final nx = -dy / len * half, ny = dx / len * half;
        pos[p++] = ax + nx;
        pos[p++] = ay + ny;
        pos[p++] = ax - nx;
        pos[p++] = ay - ny;
        pos[p++] = bx + nx;
        pos[p++] = by + ny;
        pos[p++] = bx + nx;
        pos[p++] = by + ny;
        pos[p++] = ax - nx;
        pos[p++] = ay - ny;
        pos[p++] = bx - nx;
        pos[p++] = by - ny;
        for (var v = 0; v < 6; v++) {
          col[c++] = color;
        }
      }
    }
    canvas.drawVertices(
      ui.Vertices.raw(
        ui.VertexMode.triangles,
        Float32List.sublistView(pos, 0, p),
        colors: Int32List.sublistView(col, 0, c),
      ),
      BlendMode.dst,
      Paint(),
    );
  }

  void _dashes(
    _Buf out,
    double ax,
    double ay,
    double bx,
    double by,
    double dash,
  ) {
    final dx = bx - ax, dy = by - ay;
    final len = math.sqrt(dx * dx + dy * dy);
    if (len == 0) return;
    final n = math.min(40, (len / (2 * dash)).floor());
    if (n <= 1) {
      out.add4(ax, ay, bx, by);
      return;
    }
    final step = 1 / n;
    for (var i = 0; i < n; i++) {
      final t0 = i * step, t1 = t0 + step / 2;
      out.add4(ax + dx * t0, ay + dy * t0, ax + dx * t1, ay + dy * t1);
    }
  }

  void _hulls(Canvas canvas, SubGraph sub, Float32List pos, double scale) {
    final byCluster = <int, List<Offset>>{};
    for (var k = 0; k < sub.length; k++) {
      final c = sub.model.node(sub.nodes[k]).cluster;
      if (c >= 10) continue;
      byCluster
          .putIfAbsent(c, () => [])
          .add(Offset(pos[2 * k], pos[2 * k + 1]));
    }
    for (final MapEntry(key: c, value: pts) in byCluster.entries) {
      if (pts.length < 3) continue;
      final hull = convexHull(corePoints(pts));
      if (hull.length < 3) continue;
      final path = Path()..addPolygon(hull, true);
      final color = palette[c];
      canvas.drawPath(
        path,
        Paint()
          ..color = color.withValues(alpha: 0.08)
          ..style = PaintingStyle.fill,
      );
      canvas.drawPath(
        path,
        Paint()
          ..color = color.withValues(alpha: 0.25)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 28 / scale
          ..strokeJoin = StrokeJoin.round,
      );
    }
  }

  void _paintLabels(
    Canvas canvas,
    Size size,
    SubGraph sub,
    Float32List pos,
    Set<int> highlight,
    int? focus,
  ) {
    final s = state;
    final scale = s._scale;
    final compact = s.widget.compact;
    var entry = _labels[sub];
    if (entry == null || entry.$1 != colors.ink) {
      entry = _labels[sub] = (colors.ink, <int, TextPainter>{});
    }
    final cache = entry.$2;
    // Candidates: big enough on screen, or highlighted; biggest first.
    final candidates = <int>[];
    for (var k = 0; k < sub.length; k++) {
      final sx = pos[2 * k] * scale + s._offset.dx;
      final sy = pos[2 * k + 1] * scale + s._offset.dy;
      if (sx < -50 ||
          sy < -20 ||
          sx > size.width + 50 ||
          sy > size.height + 20) {
        continue;
      }
      final screenR = s._radius[k] * scale;
      final brainAct = s.widget.brain?.activation[sub.nodes[k]];
      if (highlight.contains(k) ||
          k == focus ||
          (brainAct != null && brainAct > 0.15) ||
          screenR >= (compact ? 7 : (sub.length > 200 ? 8.5 : 5.5)) ||
          (sub.length <= 40 && !compact)) {
        candidates.add(k);
      }
    }
    candidates.sort((a, b) {
      int pri(int k) => highlight.contains(k) || k == focus ? 1 : 0;
      final p = pri(b) - pri(a);
      return p != 0 ? p : s._radius[b].compareTo(s._radius[a]);
    });
    final occupied = <int>{};
    var drawn = 0;
    final maxLabels = compact ? 14 : (sub.length > 200 ? 70 : 180);
    for (final k in candidates) {
      if (drawn >= maxLabels) break;
      final tp = cache[k] ??= TextPainter(
        text: TextSpan(
          text: sub.model.node(sub.nodes[k]).title,
          style: TextStyle(
            fontFamily: uiFont,
            fontSize: compact ? 10.5 : 11.5,
            color: colors.ink,
          ),
        ),
        textDirection: TextDirection.ltr,
        maxLines: 1,
        ellipsis: '…',
        textScaler: textScaler,
      )..layout(maxWidth: 160);
      final sx = pos[2 * k] * scale + s._offset.dx;
      final sy =
          pos[2 * k + 1] * scale + s._offset.dy + s._radius[k] * scale + 3;
      final rect = Rect.fromLTWH(sx - tp.width / 2, sy, tp.width, tp.height);
      // Coarse occupancy grid avoids overlapping labels.
      final cells = <int>[];
      for (
        var cx = (rect.left / 40).floor();
        cx <= (rect.right / 40).floor();
        cx++
      ) {
        for (
          var cy = (rect.top / 14).floor();
          cy <= (rect.bottom / 14).floor();
          cy++
        ) {
          cells.add(cx * 100003 + cy);
        }
      }
      final important = highlight.contains(k) || k == focus;
      if (!important && cells.any(occupied.contains)) continue;
      occupied.addAll(cells);
      final dimmed = highlight.isNotEmpty && !highlight.contains(k);
      if (dimmed) {
        canvas.saveLayer(
          rect.inflate(2),
          Paint()..color = const Color(0x55000000),
        );
      }
      canvas.drawRRect(
        RRect.fromRectAndRadius(rect.inflate(2), const Radius.circular(3)),
        Paint()..color = colors.paper.withValues(alpha: 0.75),
      );
      tp.paint(canvas, rect.topLeft);
      if (dimmed) canvas.restore();
      drawn++;
    }
  }

  @override
  bool shouldRepaint(_NetworkPainter old) =>
      old.colors != colors || !identical(old.state, state);
}

/// Growable float buffer reused across frames.
class _Buf {
  Float32List data = Float32List(256);
  int n = 0;

  void _grow(int extra) {
    if (n + extra <= data.length) return;
    final next = Float32List(math.max(data.length * 2, n + extra));
    next.setRange(0, n, data);
    data = next;
  }

  void add2(double a, double b) {
    _grow(2);
    data[n++] = a;
    data[n++] = b;
  }

  void add4(double a, double b, double c, double d) {
    _grow(4);
    data[n++] = a;
    data[n++] = b;
    data[n++] = c;
    data[n++] = d;
  }

  Float32List get view => Float32List.sublistView(data, 0, n);
}

/// Drops stray members (beyond 2.5× the median distance to the centroid) so a hull hugs
/// the body of its cluster instead of spanning the canvas.
List<Offset> corePoints(List<Offset> pts) {
  if (pts.length < 6) return pts;
  var cx = 0.0, cy = 0.0;
  for (final p in pts) {
    cx += p.dx;
    cy += p.dy;
  }
  final c = Offset(cx / pts.length, cy / pts.length);
  final d = [for (final p in pts) (p - c).distance]..sort();
  final limit = 2.5 * math.max(d[d.length ~/ 2], 1.0);
  return [
    for (final p in pts)
      if ((p - c).distance <= limit) p,
  ];
}

/// Convex hull (monotone chain), counter-clockwise.
List<Offset> convexHull(List<Offset> points) {
  final p = List.of(
    points,
  )..sort((a, b) => a.dx != b.dx ? a.dx.compareTo(b.dx) : a.dy.compareTo(b.dy));
  if (p.length < 3) return p;
  double cross(Offset o, Offset a, Offset b) =>
      (a.dx - o.dx) * (b.dy - o.dy) - (a.dy - o.dy) * (b.dx - o.dx);
  final lower = <Offset>[];
  for (final q in p) {
    while (lower.length >= 2 &&
        cross(lower[lower.length - 2], lower.last, q) <= 0) {
      lower.removeLast();
    }
    lower.add(q);
  }
  final upper = <Offset>[];
  for (final q in p.reversed) {
    while (upper.length >= 2 &&
        cross(upper[upper.length - 2], upper.last, q) <= 0) {
      upper.removeLast();
    }
    upper.add(q);
  }
  lower.removeLast();
  upper.removeLast();
  return [...lower, ...upper];
}
