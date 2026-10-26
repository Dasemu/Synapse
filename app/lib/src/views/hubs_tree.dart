import 'dart:async';

import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/pickers.dart';

/// Sidebar "HUBS" section: a collapsible folder tree (root hubs, their sub-hubs and pages).
/// Always visible, with a "+" to create a hub; root hubs start expanded.
class HubsSection extends StatefulWidget {
  const HubsSection({super.key, required this.onOpen, this.selectedId});
  final void Function(String id) onOpen;
  final String? selectedId;

  @override
  State<HubsSection> createState() => _HubsSectionState();
}

class _HubsSectionState extends State<HubsSection> {
  HubsDto? _data;
  Map<String, HubDto> _byId = const {};
  int? _generation;
  AppState? _app;
  Timer? _refresh;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final app = AppScope.of(context);
    if (_app != app) {
      _app?.queue.revision.removeListener(_edited);
      _app = app..queue.revision.addListener(_edited);
    }
    if (_generation != app.generation) {
      _generation = app.generation;
      _load();
    }
  }

  @override
  void dispose() {
    _refresh?.cancel();
    _app?.queue.revision.removeListener(_edited);
    super.dispose();
  }

  void _edited() {
    _refresh?.cancel();
    _refresh = Timer(const Duration(milliseconds: 1500), _load);
  }

  Future<void> _load() async {
    final app = _app;
    if (app == null) return;
    try {
      final d = await app.backend.hubs();
      if (!mounted) return;
      setState(() {
        _data = d;
        _byId = {for (final h in d.hubs) h.id: h};
      });
    } catch (e) {
      debugPrint('hubs load failed: $e');
    }
  }

  Future<void> _create() async {
    final app = AppScope.read(context);
    final hub = await showNameDialog<PageDto>(
      context,
      heading: 'New hub',
      confirm: 'Create',
      submit: (t) =>
          app.queue.run(() => app.backend.createHub(t, ''), report: false),
    );
    if (hub == null) return;
    app.reload();
    app.openId(hub.id);
  }

  void _toggle(String id) {
    final open = AppScope.read(context).expandedHubs;
    setState(() => open.contains(id) ? open.remove(id) : open.add(id));
  }

  /// Rows of a hub's subtree. `path` holds the hubs above, which guards against cycles.
  void _rows(
    List<Widget> out,
    HubDto hub,
    int depth,
    Set<String> path,
    Set<String> open,
  ) {
    final expanded = open.contains(hub.id);
    // Keys use the full path: a sub-hub can sit in several hubs.
    final inner = {...path, hub.id};
    final at = inner.join('/');
    out.add(
      _TreeRow(
        key: ValueKey('hub-$at'),
        label: hub.title,
        depth: depth,
        isHub: true,
        expanded: expanded,
        selected: widget.selectedId == hub.id,
        onToggle: () => _toggle(hub.id),
        onTap: () => widget.onOpen(hub.id),
      ),
    );
    if (!expanded) return;
    // Only sub-hubs are children here; notes are listed on the hub's own screen.
    for (final m in hub.members) {
      final sub = hub.subHubs.contains(m.id) ? _byId[m.id] : null;
      if (sub != null && !inner.contains(sub.id)) {
        _rows(out, sub, depth + 1, inner, open);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final data = _data;
    final open = AppScope.read(context).expandedHubs;
    final rows = <Widget>[];
    for (final id in data?.roots ?? const <String>[]) {
      final h = _byId[id];
      if (h != null) _rows(rows, h, 0, const {}, open);
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 0, 20, 6),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  'HUBS',
                  style: Theme.of(context).textTheme.labelSmall,
                ),
              ),
              InkResponse(
                key: const ValueKey('new-hub'),
                radius: 12,
                onTap: _create,
                child: Icon(
                  Icons.add_rounded,
                  size: 16,
                  color: context.colors.faint,
                ),
              ),
            ],
          ),
        ),
        ...rows,
        const SizedBox(height: 22),
      ],
    );
  }
}

class _TreeRow extends StatefulWidget {
  const _TreeRow({
    super.key,
    required this.label,
    required this.depth,
    required this.isHub,
    required this.selected,
    required this.onTap,
    this.expanded = false,
    this.onToggle,
  });
  final String label;
  final int depth;
  final bool isHub;
  final bool expanded;
  final bool selected;
  final VoidCallback onTap;

  /// Non-null for hubs that can be expanded.
  final VoidCallback? onToggle;

  @override
  State<_TreeRow> createState() => _TreeRowState();
}

class _TreeRowState extends State<_TreeRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final active = widget.selected;
    final toggle = widget.onToggle;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: Container(
        margin: const EdgeInsets.symmetric(horizontal: 10, vertical: 1),
        padding: EdgeInsets.only(left: 4.0 + 14 * widget.depth, right: 8),
        decoration: BoxDecoration(
          color: active
              ? colors.accentSoft
              : _hover
              ? colors.line.withAlpha(0x66)
              : null,
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 22,
              height: 28,
              child: toggle == null
                  ? null
                  : GestureDetector(
                      behavior: HitTestBehavior.opaque,
                      onTap: toggle,
                      child: Icon(
                        widget.expanded
                            ? Icons.expand_more_rounded
                            : Icons.chevron_right_rounded,
                        size: 16,
                        color: colors.faint,
                      ),
                    ),
            ),
            Expanded(
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: widget.onTap,
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 6),
                  child: Row(
                    children: [
                      Icon(
                        widget.isHub
                            ? (widget.expanded
                                  ? Icons.folder_open_outlined
                                  : Icons.folder_outlined)
                            : Icons.description_outlined,
                        size: 14,
                        color: active ? colors.accent : colors.faint,
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          widget.label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 13,
                            fontWeight: active
                                ? FontWeight.w600
                                : FontWeight.w500,
                            color: active ? colors.ink : colors.muted,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
