import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';

/// Ctrl+K: jump to a page by title or alias, or create it.
Future<void> showQuickSwitcher(BuildContext context) => showDialog<void>(
  context: context,
  barrierColor: Colors.black.withAlpha(0x33),
  builder: (_) =>
      AppScope(state: AppScope.read(context), child: const QuickSwitcher()),
);

class QuickSwitcher extends StatefulWidget {
  const QuickSwitcher({super.key});

  @override
  State<QuickSwitcher> createState() => _QuickSwitcherState();
}

class _QuickSwitcherState extends State<QuickSwitcher> {
  final _text = TextEditingController();
  List<TitleHit> _hits = const [];
  int _selected = 0;
  int _seq = 0;

  @override
  void initState() {
    super.initState();
    _search('');
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  String get _query => _text.text.trim();

  bool get _offerCreate =>
      _query.isNotEmpty &&
      !_hits.any((h) => h.title.toLowerCase() == _query.toLowerCase());

  int get _count => _hits.length + (_offerCreate ? 1 : 0);

  Future<void> _search(String q) async {
    final seq = ++_seq;
    final hits = await AppScope.read(context).backend
        .searchTitles(q, limit: 12);
    if (!mounted || seq != _seq) return;
    setState(() {
      _hits = hits;
      _selected = 0;
    });
  }

  /// Ctrl/Cmd held (or `newTab`, for middle-click) opens the result in a new tab.
  void _open(int i, {bool newTab = false}) {
    final app = AppScope.read(context);
    final query = _query;
    Navigator.of(context).pop();
    final AppRoute route;
    if (i < _hits.length) {
      route = PageTarget(id: _hits[i].pageId);
    } else if (query.isNotEmpty) {
      route = PageTarget(title: query);
    } else {
      return;
    }
    newTab ? app.openInNewTab(route) : app.navigate(route);
  }

  KeyEventResult _key(FocusNode _, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    if (_count == 0) return KeyEventResult.ignored;
    if (e.logicalKey == LogicalKeyboardKey.arrowDown) {
      setState(() => _selected = (_selected + 1) % _count);
      return KeyEventResult.handled;
    }
    if (e.logicalKey == LogicalKeyboardKey.arrowUp) {
      setState(() => _selected = (_selected - 1 + _count) % _count);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Align(
      alignment: const Alignment(0, -0.55),
      child: Material(
        color: colors.paper,
        borderRadius: BorderRadius.circular(14),
        clipBehavior: Clip.antiAlias,
        child: Container(
          width: 560,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: colors.line),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(18, 14, 18, 12),
                child: Row(
                  children: [
                    Icon(Icons.search_rounded, size: 18, color: colors.muted),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Focus(
                        onKeyEvent: _key,
                        child: TextField(
                          controller: _text,
                          autofocus: true,
                          style: TextStyle(fontSize: 16, color: colors.ink),
                          decoration: const InputDecoration.collapsed(
                            hintText: 'Search or create a page',
                          ),
                          onChanged: (q) => _search(q),
                          onSubmitted: (_) => _open(_selected),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              if (_count > 0) Divider(height: 1, color: colors.line),
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 380),
                child: ListView(
                  shrinkWrap: true,
                  padding: const EdgeInsets.all(6),
                  children: [
                    for (var i = 0; i < _count; i++)
                      _Item(
                        selected: i == _selected,
                        onTap: () => _open(i),
                        onMiddle: () => _open(i, newTab: true),
                        icon: i < _hits.length
                            ? Icons.description_outlined
                            : Icons.add_rounded,
                        title: i < _hits.length
                            ? _hits[i].title
                            : 'Create page “$_query”',
                        detail: i < _hits.length && _hits[i].isAlias
                            ? 'alias'
                            : null,
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Item extends StatelessWidget {
  const _Item({
    required this.selected,
    required this.onTap,
    required this.icon,
    required this.title,
    required this.onMiddle,
    this.detail,
  });
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onMiddle;
  final IconData icon;
  final String title;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        onTertiaryTapUp: (_) => onMiddle(),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
          decoration: BoxDecoration(
            color: selected ? colors.accentSoft : null,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              Icon(
                icon,
                size: 16,
                color: selected ? colors.accent : colors.muted,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 14, color: colors.ink),
                ),
              ),
              if (detail != null)
                Text(
                  detail!,
                  style: TextStyle(fontSize: 12, color: colors.faint),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
