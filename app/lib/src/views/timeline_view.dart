import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/dates.dart';

/// Loaded timeline data, fetched in fixed chunks of days as the view pans.
class TimelineModel extends ChangeNotifier {
  TimelineModel(this.backend, this.today);

  static const chunkDays = 32;
  final GraphBackend backend;

  /// Day index 0.
  final DateTime today;

  final _loaded = <int>{};
  final days = <int, TimelineDayDto>{};
  final titles = <String, PageBrief>{};

  /// Where each page hangs: (day index, row).
  final born = <String, (int, int)>{};

  DateTime dayAt(int index) => today.add(Duration(days: index));
  int indexOf(String ymd) => parseDay(ymd).difference(today).inDays;

  void ensure(int from, int to) {
    final c0 = (from / chunkDays).floor();
    final c1 = (to / chunkDays).floor();
    for (var c = c0; c <= c1; c++) {
      if (_loaded.add(c)) _load(c);
    }
  }

  Future<void> _load(int chunk) async {
    final start = chunk * chunkDays;
    try {
      final t = await backend.timeline(
        formatDay(dayAt(start)),
        formatDay(dayAt(start + chunkDays - 1)),
      );
      for (final p in t.pages) {
        titles[p.id] = p;
      }
      for (final d in t.days) {
        final i = indexOf(d.date);
        days[i] = d;
        for (var k = 0; k < d.created.length; k++) {
          born[d.created[k]] = (i, k);
        }
      }
      notifyListeners();
    } catch (e) {
      _loaded.remove(chunk);
      debugPrint('timeline chunk $chunk failed: $e');
    }
  }
}

/// Journals on a horizontal line, each linked to the next; pages hang below the journal
/// they were born in, with faint edges to the days they were edited. Pan by dragging, zoom
/// with the wheel or a pinch, tap a node to open it.
class TimelineView extends StatefulWidget {
  const TimelineView({super.key});

  @override
  State<TimelineView> createState() => _TimelineViewState();
}

class _TimelineViewState extends State<TimelineView> {
  late final TimelineModel _model;
  final _view = _Viewport();
  final _hits = <(Rect, String)>[];
  double _startDayWidth = 0;
  bool _placed = false;

  @override
  void initState() {
    super.initState();
    final backend = AppScope.read(context).backend;
    _model = TimelineModel(backend, parseDay(backend.today()));
  }

  @override
  void dispose() {
    _model.dispose();
    _view.dispose();
    super.dispose();
  }

  void _zoom(double factor, double focalX) {
    final dw = (_view.dayWidth * factor).clamp(4.0, 260.0);
    // Keep the day under the focal point fixed.
    final day = (focalX - _view.x) / _view.dayWidth;
    _view.update(dayWidth: dw, x: focalX - day * dw);
  }

  void _tap(Offset p) {
    for (final (r, id) in _hits.reversed) {
      if (r.inflate(3).contains(p)) {
        AppScope.read(context).openId(id);
        return;
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return LayoutBuilder(
      builder: (context, box) {
        if (!_placed) {
          _placed = true;
          _view.x = box.maxWidth - 140;
        }
        return Stack(
          children: [
            Positioned.fill(
              child: ClipRect(
                child: Listener(
                  onPointerSignal: (e) {
                    if (e is PointerScrollEvent) {
                      if (HardwareKeyboard.instance.isShiftPressed) {
                        _view.update(x: _view.x - e.scrollDelta.dy);
                      } else {
                        _zoom(
                          math.pow(1.0015, -e.scrollDelta.dy).toDouble(),
                          e.localPosition.dx,
                        );
                      }
                    }
                  },
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTapUp: (d) => _tap(d.localPosition),
                    onScaleStart: (_) => _startDayWidth = _view.dayWidth,
                    onScaleUpdate: (d) {
                      if (d.scale != 1.0) {
                        final target = _startDayWidth * d.scale;
                        _zoom(target / _view.dayWidth, d.localFocalPoint.dx);
                      }
                      _view.update(
                        x: _view.x + d.focalPointDelta.dx,
                        y: math.min(0, _view.y + d.focalPointDelta.dy),
                      );
                    },
                    child: CustomPaint(
                      painter: _TimelinePainter(
                        model: _model,
                        view: _view,
                        colors: colors,
                        hits: _hits,
                        textScaler: MediaQuery.textScalerOf(context),
                      ),
                      size: Size.infinite,
                    ),
                  ),
                ),
              ),
            ),
            Positioned(
              right: 20,
              top: 16,
              child: _Toolbar(
                onToday: () => _view.update(x: box.maxWidth - 140, y: 0),
                onZoomIn: () => _zoom(1.4, box.maxWidth / 2),
                onZoomOut: () => _zoom(1 / 1.4, box.maxWidth / 2),
              ),
            ),
            Positioned(left: 24, bottom: 18, child: _Legend(colors: colors)),
          ],
        );
      },
    );
  }
}

class _Viewport extends ChangeNotifier {
  double dayWidth = 64;

  /// Screen x of day 0 (today).
  double x = 0;

  /// Vertical scroll of the hanging pages (<= 0).
  double y = 0;

  void update({double? dayWidth, double? x, double? y}) {
    this.dayWidth = dayWidth ?? this.dayWidth;
    this.x = x ?? this.x;
    this.y = y ?? this.y;
    notifyListeners();
  }
}

class _TimelinePainter extends CustomPainter {
  _TimelinePainter({
    required this.model,
    required this.view,
    required this.colors,
    required this.hits,
    required this.textScaler,
  }) : super(repaint: Listenable.merge([model, view]));

  final TimelineModel model;
  final _Viewport view;
  final SynapseColors colors;
  final List<(Rect, String)> hits;
  final TextScaler textScaler;

  static const axisY = 120.0;
  static const rowH = 28.0;

  double xOf(int day) => view.x + day * view.dayWidth;

  TextPainter _label(String text, TextStyle style, double maxWidth) {
    return TextPainter(
      text: TextSpan(text: text, style: style),
      textDirection: TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
      textScaler: textScaler,
    )..layout(maxWidth: math.max(0, maxWidth));
  }

  @override
  void paint(Canvas canvas, Size size) {
    hits.clear();
    final dw = view.dayWidth;
    final first = ((0 - view.x) / dw).floor() - 1;
    final last = ((size.width - view.x) / dw).ceil() + 1;
    // Load a margin around the viewport so edges to nearby days can be drawn.
    final margin = (size.width / dw).ceil();
    model.ensure(first - margin, last + margin);

    // Axis.
    canvas.drawLine(
      Offset(0, axisY),
      Offset(size.width, axisY),
      Paint()
        ..color = colors.line
        ..strokeWidth = 1,
    );

    // Month and day ticks.
    final labelStyle = TextStyle(
      fontFamily: uiFont,
      fontSize: 11,
      color: colors.muted,
    );
    for (var d = first; d <= last; d++) {
      final date = model.dayAt(d);
      final x = xOf(d);
      if (date.day == 1) {
        canvas.drawLine(
          Offset(x, axisY - 40),
          Offset(x, axisY - 8),
          Paint()..color = colors.line,
        );
        final tp = _label(
          '${monthName(date.month)} ${date.year}',
          labelStyle.copyWith(fontWeight: FontWeight.w600, color: colors.ink),
          200,
        );
        tp.paint(canvas, Offset(x + 6, axisY - 44));
      }
      if (dw >= 22) {
        final tp = _label(
          '${date.day}',
          labelStyle.copyWith(color: colors.faint),
          dw,
        );
        tp.paint(canvas, Offset(x - tp.width / 2, axisY - 24));
      }
    }

    // Month of the first visible day, pinned to the left edge.
    final firstDate = model.dayAt(first + 1);
    final sticky = _label(
      '${monthName(firstDate.month)} ${firstDate.year}',
      labelStyle.copyWith(fontWeight: FontWeight.w600, color: colors.ink),
      200,
    );
    final nextMonth = DateTime.utc(firstDate.year, firstDate.month + 1);
    final nextX = xOf(nextMonth.difference(model.today).inDays);
    final stickyX = math.min(24.0, nextX - sticky.width - 12);
    canvas.drawRect(
      Rect.fromLTWH(stickyX - 6, axisY - 48, sticky.width + 12, 22),
      Paint()..color = colors.paper,
    );
    sticky.paint(canvas, Offset(stickyX, axisY - 44));

    // Today marker.
    final tx = xOf(0);
    if (tx >= 0 && tx <= size.width) {
      canvas.drawLine(
        Offset(tx, axisY - 30),
        Offset(tx, size.height),
        Paint()
          ..color = colors.accent.withAlpha(0x33)
          ..strokeWidth = 1,
      );
    }

    final journalDays =
        model.days.entries
            .where((e) => e.value.journalId != null)
            .map((e) => e.key)
            .toList()
          ..sort();

    // Episodic chain: each journal linked to the next.
    final chain = Paint()
      ..color = colors.accent.withAlpha(0x88)
      ..strokeWidth = 2
      ..strokeCap = StrokeCap.round;
    for (var k = 0; k + 1 < journalDays.length; k++) {
      final a = journalDays[k], b = journalDays[k + 1];
      if (b < first || a > last) continue;
      canvas.drawLine(Offset(xOf(a), axisY), Offset(xOf(b), axisY), chain);
    }

    final y0 = axisY + 44 + view.y;
    final showLabels = dw >= 34;
    // Labels may run until the next day that also has pages hanging from it.
    final withPages =
        model.days.entries
            .where((e) => e.value.created.isNotEmpty)
            .map((e) => e.key)
            .toList()
          ..sort();
    double labelWidth(int d) {
      final next = withPages.where((x) => x > d).firstOrNull;
      final room = next == null ? 220.0 : (next - d) * dw - 18;
      return room.clamp(40.0, 220.0);
    }

    // Edges from pages to the days they were edited.
    final edge = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1;
    for (final e in model.days.entries) {
      final d = e.key;
      for (final ed in e.value.edited) {
        final b = model.born[ed.pageId];
        if (b == null || b.$1 == d) continue;
        if ((b.$1 < first && d < first) || (b.$1 > last && d > last)) continue;
        final from = Offset(xOf(b.$1) + 4, y0 + b.$2 * rowH);
        final to = Offset(xOf(d), axisY);
        edge.color = colors.ref.withAlpha(
          (30 + 20 * math.min(ed.edits, 6)).toInt(),
        );
        final mid = Offset(
          (from.dx + to.dx) / 2,
          from.dy + 24 + (from.dx - to.dx).abs() * 0.08,
        );
        canvas.drawPath(
          Path()
            ..moveTo(from.dx, from.dy)
            ..quadraticBezierTo(mid.dx, mid.dy, to.dx, to.dy),
          edge,
        );
      }
    }

    // Pages hanging from their birth day, and journal nodes.
    for (var d = first; d <= last; d++) {
      final day = model.days[d];
      if (day == null) continue;
      final x = xOf(d);
      if (day.created.isNotEmpty) {
        canvas.drawLine(
          Offset(x, axisY),
          Offset(x, y0 + (day.created.length - 1) * rowH),
          Paint()
            ..color = colors.line
            ..strokeWidth = 1,
        );
      }
      for (var k = 0; k < day.created.length; k++) {
        final id = day.created[k];
        final c = Offset(x, y0 + k * rowH);
        if (c.dy < axisY + 8) continue;
        canvas.drawCircle(c, 4, Paint()..color = colors.paper);
        canvas.drawCircle(
          c,
          4,
          Paint()
            ..color = colors.muted
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1.5,
        );
        var hit = Rect.fromCircle(center: c, radius: 6);
        if (showLabels) {
          final title = model.titles[id]?.title ?? '…';
          final tp = _label(
            title,
            TextStyle(fontFamily: uiFont, fontSize: 12, color: colors.ink),
            labelWidth(d),
          );
          final at = Offset(c.dx + 9, c.dy - tp.height / 2);
          tp.paint(canvas, at);
          hit = hit.expandToInclude(at & tp.size);
        }
        hits.add((hit, id));
      }
      final jid = day.journalId;
      if (jid != null) {
        final c = Offset(x, axisY);
        final r = day.created.isNotEmpty || day.edited.isNotEmpty ? 6.0 : 4.5;
        canvas.drawCircle(c, r + 2, Paint()..color = colors.paper);
        canvas.drawCircle(c, r, Paint()..color = colors.accent);
        hits.add((Rect.fromCircle(center: c, radius: r + 6), jid));
      }
    }
  }

  @override
  bool shouldRepaint(_TimelinePainter old) =>
      old.colors != colors || old.model != model || old.view != view;
}

class _Toolbar extends StatelessWidget {
  const _Toolbar({
    required this.onToday,
    required this.onZoomIn,
    required this.onZoomOut,
  });
  final VoidCallback onToday;
  final VoidCallback onZoomIn;
  final VoidCallback onZoomOut;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    Widget btn(IconData i, String tip, VoidCallback f) => IconButton(
      tooltip: tip,
      onPressed: f,
      icon: Icon(i, size: 18, color: colors.muted),
    );
    return Container(
      decoration: BoxDecoration(
        color: colors.paper,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: colors.line),
      ),
      child: Row(
        children: [
          btn(Icons.today_outlined, 'Today', onToday),
          btn(Icons.remove_rounded, 'Zoom out', onZoomOut),
          btn(Icons.add_rounded, 'Zoom in', onZoomIn),
        ],
      ),
    );
  }
}

class _Legend extends StatelessWidget {
  const _Legend({required this.colors});
  final SynapseColors colors;

  @override
  Widget build(BuildContext context) {
    Widget item(Widget mark, String text) => Padding(
      padding: const EdgeInsets.only(right: 16),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          mark,
          const SizedBox(width: 6),
          Text(text, style: TextStyle(fontSize: 12, color: colors.muted)),
        ],
      ),
    );
    return Row(
      children: [
        item(
          Container(
            width: 9,
            height: 9,
            decoration: BoxDecoration(
              color: colors.accent,
              shape: BoxShape.circle,
            ),
          ),
          'Journal',
        ),
        item(
          Container(
            width: 9,
            height: 9,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              border: Border.all(color: colors.muted, width: 1.5),
            ),
          ),
          'Page born that day',
        ),
        item(Container(width: 16, height: 1.5, color: colors.ref), 'Edited on'),
      ],
    );
  }
}
