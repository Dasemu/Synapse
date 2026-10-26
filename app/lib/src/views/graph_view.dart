import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../graph/graph_model.dart';
import '../graph/network_canvas.dart';
import '../graph/tree_canvas.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/dates.dart';
import 'timeline_view.dart';

/// Loads the page-level graph after pending writes have reached the core.
Future<GraphModel> loadGraphModel(AppState app) async {
  final data = await app.queue.run(app.backend.graphData, report: false);
  return GraphModel(data);
}

/// One view, several ways of looking at the graph (mode selector on top).
class GraphView extends StatefulWidget {
  const GraphView({super.key, this.mode = GraphMode.network, this.focusId});
  final GraphMode mode;

  /// Page to focus in the synapse mode.
  final String? focusId;

  @override
  State<GraphView> createState() => _GraphViewState();
}

/// How the clusters mode groups pages.
enum ClusterBy { hubs, tags, communities }

class _GraphViewState extends State<GraphView> {
  late GraphMode _mode = widget.mode;
  GraphModel? _model;
  Object? _error;
  GraphFilter _filter = const GraphFilter();
  ColorBy _colorBy = ColorBy.cluster;

  /// Clusters mode grouping; null = hubs when the graph has any, else communities.
  ClusterBy? _clusterBy;
  int? _focus;
  BrainOverlay? _brain;
  SubGraph? _sub;
  final _query = TextEditingController();

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    final app = AppScope.read(context);
    try {
      final m = await loadGraphModel(app);
      if (!mounted) return;
      int? focus = widget.focusId == null ? null : m.indexOf(widget.focusId!);
      for (final r in app.recent) {
        if (focus != null) break;
        focus = m.indexOf(r.id);
      }
      if (focus != null && m.isTag(focus)) focus = null;
      if (focus == null) {
        final pages = [
          for (var i = 0; i < m.nodeCount; i++)
            if (!m.isTag(i)) i,
        ];
        if (pages.isNotEmpty) {
          focus = pages.reduce(
            (a, b) => m.node(a).degree >= m.node(b).degree ? a : b,
          );
        }
      }
      setState(() {
        _model = m;
        _focus = focus;
      });
      _rebuildSub();
    } catch (e) {
      if (mounted) setState(() => _error = e);
    }
  }

  ClusterBy get _grouping =>
      _clusterBy ??
      ((_model?.hasHubs ?? false) ? ClusterBy.hubs : ClusterBy.communities);

  void _setMode(GraphMode m) {
    setState(() => _mode = m);
    _rebuildSub();
  }

  void _setFilter(GraphFilter f) {
    setState(() => _filter = f);
    _rebuildSub();
  }

  void _setFocus(int global) {
    setState(() => _focus = global);
    _rebuildSub();
  }

  Future<void> _rebuildSub() async {
    final m = _model;
    if (m == null) return;
    switch (_mode) {
      case GraphMode.network || GraphMode.clusters:
        final nodes = _filter.apply(m);
        // Hub grouping: layout-only springs to the home hub so folders cluster spatially.
        final springs = _mode != GraphMode.clusters
            ? const <(int, int, double)>[]
            : switch (_grouping) {
                ClusterBy.hubs => m.hubSprings(nodes),
                ClusterBy.tags => m.tagSprings(nodes),
                ClusterBy.communities => const <(int, int, double)>[],
              };
        setState(
          () => _sub = m.subgraph(nodes, _filter.kinds, springs: springs),
        );
      case GraphMode.brain:
        final f = _focus;
        if (f == null) return;
        final app = AppScope.read(context);
        final List<RelatedDto> related;
        try {
          related = await app.backend.related(m.node(f).id, limit: 40);
        } catch (_) {
          return;
        }
        if (!mounted || _focus != f || _mode != GraphMode.brain) return;
        final top = related.isEmpty
            ? 1.0
            : related.map((r) => r.score).reduce(math.max);
        final activation = <int, double>{f: 1.0};
        for (final r in related) {
          final i = m.indexOf(r.page.id);
          if (i != null) activation[i] = (r.score / top).clamp(0.05, 1.0) * 0.9;
        }
        final kinds = {...EdgeKind.values};
        // Springs to the focus, stronger for more activated notes, keep the picture radial.
        final sub = m.subgraph(
          activation.keys.toList(),
          kinds,
          springs: [
            for (final MapEntry(key: i, value: a) in activation.entries)
              if (i != f) (f, i, 0.4 + 2 * a),
          ],
        );
        // Hops from the focus inside the subgraph drive the pulse.
        final hops = <int, int>{f: 0};
        var frontier = [f];
        while (frontier.isNotEmpty) {
          final next = <int>[];
          for (final i in frontier) {
            for (final e in m.adjacency[i]) {
              final j = m.other(e, i);
              if (sub.local.containsKey(j) && !hops.containsKey(j)) {
                hops[j] = hops[i]! + 1;
                next.add(j);
              }
            }
          }
          frontier = next;
        }
        // Related but unconnected inside the slice: outermost ring.
        final outer = hops.values.fold(0, math.max) + 1;
        for (final i in activation.keys) {
          hops.putIfAbsent(i, () => outer);
        }
        setState(() {
          _sub = sub;
          _brain = BrainOverlay(focus: f, activation: activation, hops: hops);
        });
      case GraphMode.tree || GraphMode.timeline:
        break;
    }
  }

  void _open(int global) {
    final m = _model!;
    final app = AppScope.read(context);
    if (m.isTag(global)) {
      app.openTag(m.node(global).title);
    } else {
      app.openId(m.node(global).id);
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
          child: Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              SegmentedButton<GraphMode>(
                key: const ValueKey('graph-modes'),
                showSelectedIcon: false,
                style: SegmentedButton.styleFrom(
                  visualDensity: VisualDensity.compact,
                  textStyle: const TextStyle(
                    fontFamily: uiFont,
                    fontSize: 12.5,
                  ),
                  side: BorderSide(color: colors.line),
                  selectedBackgroundColor: colors.accentSoft,
                ),
                segments: [
                  for (final m in GraphMode.values)
                    ButtonSegment(
                      value: m,
                      label: Text(m.label),
                      icon: Icon(m.icon, size: 16),
                    ),
                ],
                selected: {_mode},
                onSelectionChanged: (s) => _setMode(s.first),
              ),
              if (_model != null) ..._controls(context),
            ],
          ),
        ),
        Divider(height: 1, color: colors.line),
        Expanded(child: _body(context)),
      ],
    );
  }

  List<Widget> _controls(BuildContext context) {
    final m = _model!;
    final colors = context.colors;
    Widget chip(String label, bool on, ValueChanged<bool> f) => FilterChip(
      label: Text(label, style: const TextStyle(fontSize: 12)),
      selected: on,
      onSelected: f,
      visualDensity: VisualDensity.compact,
      showCheckmark: false,
      selectedColor: colors.accentSoft,
      side: BorderSide(color: colors.line),
    );
    final search = SizedBox(
      width: 200,
      child: TextField(
        controller: _query,
        style: const TextStyle(fontSize: 13),
        decoration: InputDecoration(
          isDense: true,
          prefixIcon: const Icon(Icons.search_rounded, size: 16),
          prefixIconConstraints: const BoxConstraints(minWidth: 30),
          hintText: _mode == GraphMode.network || _mode == GraphMode.clusters
              ? 'Filter pages'
              : 'Focus page',
          border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
          contentPadding: const EdgeInsets.symmetric(vertical: 8),
        ),
        onChanged: (q) {
          if (_mode == GraphMode.network || _mode == GraphMode.clusters) {
            _setFilter(_filter.copyWith(query: q));
          }
        },
        onSubmitted: (q) {
          final lq = q.toLowerCase().trim();
          for (var i = 0; i < m.nodeCount; i++) {
            if (!m.isTag(i) && m.node(i).title.toLowerCase().contains(lq)) {
              _setFocus(i);
              return;
            }
          }
        },
      ),
    );
    switch (_mode) {
      case GraphMode.network || GraphMode.clusters:
        return [
          search,
          chip(
            'Journals',
            _filter.journals,
            (v) => _setFilter(_filter.copyWith(journals: v)),
          ),
          chip(
            'Orphans',
            _filter.orphans,
            (v) => _setFilter(_filter.copyWith(orphans: v)),
          ),
          chip(
            'Mentions',
            _filter.kinds.contains(EdgeKind.mention),
            (v) => _setFilter(
              _filter.copyWith(
                kinds: v
                    ? {..._filter.kinds, EdgeKind.mention}
                    : ({..._filter.kinds}..remove(EdgeKind.mention)),
              ),
            ),
          ),
          _Labeled(
            'Min links ${_filter.minDegree}',
            SizedBox(
              width: 110,
              child: Slider(
                value: _filter.minDegree.toDouble(),
                max: 10,
                divisions: 10,
                onChanged: (v) =>
                    _setFilter(_filter.copyWith(minDegree: v.round())),
              ),
            ),
          ),
          _Dropdown<int?>(
            value: _filter.tag,
            hint: 'Any tag',
            items: {
              null: 'Any tag',
              for (final t in m.tags.take(40)) t: '#${m.node(t).title}',
            },
            onChanged: (t) => _setFilter(_filter.copyWith(tag: () => t)),
          ),
          TextButton.icon(
            icon: const Icon(Icons.date_range_outlined, size: 16),
            label: Text(
              _filter.from == null
                  ? 'Any date'
                  : '${shortDate(parseDay(_filter.from!))} – ${shortDate(parseDay(_filter.to!))}',
              style: const TextStyle(fontSize: 12),
            ),
            onPressed: () async {
              final now = DateTime.now();
              final r = await showDateRangePicker(
                context: context,
                firstDate: DateTime(1990),
                lastDate: DateTime(now.year + 1),
              );
              if (r == null) return;
              _setFilter(
                _filter.copyWith(
                  from: () => formatDay(r.start),
                  to: () => formatDay(r.end),
                ),
              );
            },
          ),
          if (_filter.from != null)
            IconButton(
              tooltip: 'Any date',
              iconSize: 15,
              visualDensity: VisualDensity.compact,
              icon: const Icon(Icons.close_rounded),
              onPressed: () => _setFilter(
                _filter.copyWith(from: () => null, to: () => null),
              ),
            ),
          if (_mode == GraphMode.network)
            _Dropdown<ColorBy>(
              value: _colorBy,
              hint: 'Colour',
              items: const {
                ColorBy.cluster: 'Colour: cluster',
                ColorBy.tag: 'Colour: tag',
                ColorBy.hub: 'Colour: hub',
                ColorBy.none: 'Colour: none',
              },
              onChanged: (c) => setState(() => _colorBy = c ?? ColorBy.cluster),
            ),
          if (_mode == GraphMode.clusters)
            _Dropdown<ClusterBy>(
              key: const ValueKey('cluster-group'),
              value: _grouping,
              hint: 'Group',
              items: const {
                ClusterBy.hubs: 'Group: hubs',
                ClusterBy.tags: 'Group: tags',
                ClusterBy.communities: 'Group: communities',
              },
              onChanged: (v) {
                _clusterBy = v;
                _rebuildSub();
              },
            ),
        ];
      case GraphMode.brain:
        return [
          search,
          if (_focus != null)
            _FocusLink(
              title: m.node(_focus!).title,
              onTap: () => _open(_focus!),
            ),
          TextButton.icon(
            icon: const Icon(Icons.replay_rounded, size: 16),
            label: const Text('Pulse', style: TextStyle(fontSize: 12)),
            onPressed: () {
              final b = _brain;
              if (b == null) return;
              // A new overlay object restarts the pulse animation.
              setState(() => _brain = null);
              WidgetsBinding.instance.addPostFrameCallback((_) {
                if (mounted) setState(() => _brain = b);
              });
            },
          ),
        ];
      case GraphMode.tree || GraphMode.timeline:
        return const [];
    }
  }

  Widget _body(BuildContext context) {
    final colors = context.colors;
    if (_error != null) {
      return Center(child: Text('Could not load the graph: $_error'));
    }
    if (_mode == GraphMode.timeline) return const TimelineView();
    final m = _model;
    if (m == null) {
      return const Center(
        child: SizedBox(
          width: 18,
          height: 18,
          child: CircularProgressIndicator(strokeWidth: 2),
        ),
      );
    }
    if (_mode == GraphMode.tree) {
      return TreeCanvas(
        model: m,
        onTap: _open,
        name: AppScope.read(context).backend.graphName,
      );
    }
    final sub = _sub;
    if (sub == null) return const SizedBox.shrink();
    final app = AppScope.read(context);
    final explore = _mode == GraphMode.brain;
    return Stack(
      children: [
        Positioned.fill(
          child: NetworkCanvas(
            key: ValueKey(_mode),
            sub: sub,
            cache: app.graphPositions,
            colorBy: _mode == GraphMode.clusters
                ? switch (_grouping) {
                    ClusterBy.hubs => ColorBy.hub,
                    ClusterBy.tags => ColorBy.tagGroup,
                    ClusterBy.communities => ColorBy.cluster,
                  }
                : _colorBy,
            hulls: _mode == GraphMode.clusters,
            focus: explore ? _focus : null,
            brain: _mode == GraphMode.brain ? _brain : null,
            onTap: explore
                ? (i) => m.isTag(i) ? _open(i) : _setFocus(i)
                : _open,
          ),
        ),
        Positioned(
          left: 20,
          bottom: 14,
          child: _Legend(
            mode: _mode,
            stats: '${sub.length} pages · ${sub.edges.length} links',
            colors: colors,
          ),
        ),
        if (explore)
          Positioned(
            right: 20,
            bottom: 14,
            child: Text(
              'Click a node to focus it',
              style: TextStyle(fontSize: 12, color: colors.faint),
            ),
          ),
      ],
    );
  }
}

class _Labeled extends StatelessWidget {
  const _Labeled(this.label, this.child);
  final String label;
  final Widget child;

  @override
  Widget build(BuildContext context) => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      Text(label, style: TextStyle(fontSize: 12, color: context.colors.muted)),
      child,
    ],
  );
}

class _Dropdown<T> extends StatelessWidget {
  const _Dropdown({
    super.key,
    required this.value,
    required this.hint,
    required this.items,
    required this.onChanged,
  });
  final T value;
  final String hint;
  final Map<T, String> items;
  final ValueChanged<T?> onChanged;

  @override
  Widget build(BuildContext context) => DropdownButtonHideUnderline(
    child: DropdownButton<T>(
      value: value,
      hint: Text(hint),
      isDense: true,
      style: TextStyle(
        fontFamily: uiFont,
        fontSize: 12.5,
        color: context.colors.ink,
      ),
      borderRadius: BorderRadius.circular(8),
      items: [
        for (final e in items.entries)
          DropdownMenuItem(value: e.key, child: Text(e.value)),
      ],
      onChanged: onChanged,
    ),
  );
}

class _FocusLink extends StatelessWidget {
  const _FocusLink({required this.title, required this.onTap});
  final String title;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => ActionChip(
    avatar: Icon(
      Icons.open_in_new_rounded,
      size: 14,
      color: context.colors.accent,
    ),
    label: Text(title, style: const TextStyle(fontSize: 12)),
    onPressed: onTap,
    visualDensity: VisualDensity.compact,
    side: BorderSide(color: context.colors.line),
  );
}

class _Legend extends StatelessWidget {
  const _Legend({
    required this.mode,
    required this.stats,
    required this.colors,
  });
  final GraphMode mode;
  final String stats;
  final SynapseColors colors;

  @override
  Widget build(BuildContext context) {
    final palette = graphPalette(Theme.of(context).brightness);
    Widget line(Color c, String label, {bool dashed = false, double w = 1.5}) =>
        Padding(
          padding: const EdgeInsets.only(right: 14),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              CustomPaint(
                size: const Size(18, 8),
                painter: _LegendLine(c, dashed, w),
              ),
              const SizedBox(width: 5),
              Text(
                label,
                style: TextStyle(fontSize: 11.5, color: colors.muted),
              ),
            ],
          ),
        );
    Widget hubMark() => Padding(
      padding: const EdgeInsets.only(right: 14),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 11,
            height: 11,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: colors.muted,
              border: Border.all(color: colors.ink, width: 1.6),
            ),
          ),
          const SizedBox(width: 5),
          Text('Hub', style: TextStyle(fontSize: 11.5, color: colors.muted)),
        ],
      ),
    );
    final items = mode == GraphMode.brain
        ? [
            line(palette[3], 'Synapse (thicker = stronger)', w: 3),
            line(colors.accent, 'Activation pulse', w: 3),
            hubMark(),
          ]
        : [
            line(colors.muted, 'Link'),
            line(colors.accent, 'Tag'),
            line(palette[2], 'Property'),
            line(colors.ref, 'Block ref / embed'),
            line(colors.ink, 'Contains (hub → note)', w: 1),
            line(colors.faint, 'Mention', dashed: true),
            hubMark(),
          ];
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: colors.paper.withValues(alpha: 0.85),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          ...items,
          Text(stats, style: TextStyle(fontSize: 11.5, color: colors.faint)),
        ],
      ),
    );
  }
}

class _LegendLine extends CustomPainter {
  _LegendLine(this.color, this.dashed, this.width);
  final Color color;
  final bool dashed;
  final double width;

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()
      ..color = color
      ..strokeWidth = width
      ..strokeCap = StrokeCap.round;
    final y = size.height / 2;
    if (!dashed) {
      canvas.drawLine(Offset(0, y), Offset(size.width, y), p);
      return;
    }
    for (var x = 0.0; x < size.width; x += 6) {
      canvas.drawLine(Offset(x, y), Offset(math.min(x + 3, size.width), y), p);
    }
  }

  @override
  bool shouldRepaint(_LegendLine old) => old.color != color;
}

/// Neighbourhood of a page as a small panel (page view side column).
class LocalGraphPanel extends StatefulWidget {
  const LocalGraphPanel({super.key, required this.pageId, this.height = 240});
  final String pageId;
  final double height;

  @override
  State<LocalGraphPanel> createState() => _LocalGraphPanelState();
}

class _LocalGraphPanelState extends State<LocalGraphPanel> {
  GraphModel? _model;
  SubGraph? _sub;
  int _depth = 1;

  Timer? _refresh;
  late final AppState _app = AppScope.read(context);

  @override
  void initState() {
    super.initState();
    _load();
    _app.queue.revision.addListener(_edited);
  }

  @override
  void dispose() {
    _app.queue.revision.removeListener(_edited);
    _refresh?.cancel();
    super.dispose();
  }

  /// Edits (e.g. a link typed on this page) refresh the panel once typing pauses.
  void _edited() {
    _refresh?.cancel();
    _refresh = Timer(const Duration(milliseconds: 1500), _load);
  }

  void _load() {
    loadGraphModel(_app).then((m) {
      if (!mounted) return;
      if (_model != null && _model!.data.seq == m.data.seq) return;
      _model = m;
      _build();
    }, onError: (Object _) {});
  }

  void _build() {
    final m = _model;
    final f = m?.indexOf(widget.pageId);
    if (m == null || f == null) return;
    const kinds = {
      EdgeKind.link,
      EdgeKind.tag,
      EdgeKind.property,
      EdgeKind.blockRef,
      EdgeKind.embed,
      EdgeKind.mention,
      EdgeKind.contains,
      EdgeKind.tagged,
    };
    final hood = m.neighbourhood(f, _depth, kinds).keys.toList();
    setState(() => _sub = m.subgraph(hood, kinds));
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final app = AppScope.read(context);
    final m = _model;
    final sub = _sub;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text('LOCAL GRAPH', style: Theme.of(context).textTheme.labelSmall),
            const Spacer(),
            for (final d in [1, 2, 3])
              InkWell(
                borderRadius: BorderRadius.circular(4),
                onTap: () {
                  _depth = d;
                  _build();
                },
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 5,
                    vertical: 2,
                  ),
                  child: Text(
                    '$d',
                    style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: d == _depth
                          ? FontWeight.w700
                          : FontWeight.w400,
                      color: d == _depth ? colors.accent : colors.faint,
                    ),
                  ),
                ),
              ),
          ],
        ),
        const SizedBox(height: 6),
        Container(
          height: widget.height,
          decoration: BoxDecoration(
            color: colors.surface,
            borderRadius: BorderRadius.circular(10),
          ),
          clipBehavior: Clip.antiAlias,
          child: m == null || sub == null
              ? const SizedBox.shrink()
              : sub.length <= 1
              ? Center(
                  child: Text(
                    'No connections yet',
                    style: TextStyle(fontSize: 12, color: colors.faint),
                  ),
                )
              : NetworkCanvas(
                  sub: sub,
                  compact: true,
                  focus: m.indexOf(widget.pageId),
                  cache: app.graphPositions,
                  onTap: (g) => m.isTag(g)
                      ? app.openTag(m.node(g).title)
                      : app.openId(m.node(g).id),
                ),
        ),
      ],
    );
  }
}
