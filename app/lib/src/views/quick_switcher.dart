import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/type_icon.dart';

/// Ctrl+K: jump to a page by title or alias, to a page whose blocks contain the text, or
/// create it.
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

  /// Blocks containing the query (pages already listed by title are not repeated).
  List<BlockHit> _blocks = const [];
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

  // Rows: title hits, then block hits, then "Create page".
  int get _count => _hits.length + _blocks.length + (_offerCreate ? 1 : 0);

  Future<void> _search(String q) async {
    final seq = ++_seq;
    final backend = AppScope.read(context).backend;
    final results = await Future.wait([
      backend.searchTitles(q, limit: 12),
      // Content search starts at two characters: one matches nearly every block.
      q.trim().length < 2
          ? Future.value(const <BlockHit>[])
          : backend.searchBlocks(q, limit: 30),
    ]);
    if (!mounted || seq != _seq) return;
    final hits = results[0] as List<TitleHit>;
    final titled = {for (final h in hits) h.pageId};
    final seen = <String>{};
    final blocks = [
      for (final b in results[1] as List<BlockHit>)
        // One row per page, the first (most recent) matching block.
        if (!titled.contains(b.pageId) && seen.add(b.pageId)) b,
    ].take(12).toList();
    setState(() {
      _hits = hits;
      _blocks = blocks;
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
    } else if (i < _hits.length + _blocks.length) {
      route = PageTarget(id: _blocks[i - _hits.length].pageId);
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
                    for (var i = 0; i < _hits.length; i++)
                      _Item(
                        selected: i == _selected,
                        onTap: () => _open(i),
                        onMiddle: () => _open(i, newTab: true),
                        icon: Icons.description_outlined,
                        title: _hits[i].title,
                        pageType: _hits[i].pageType,
                        detail: _hits[i].isAlias ? 'alias' : null,
                      ),
                    if (_blocks.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.fromLTRB(12, 10, 12, 4),
                        child: Text(
                          'IN NOTES',
                          style: Theme.of(context).textTheme.labelSmall,
                        ),
                      ),
                    for (var j = 0; j < _blocks.length; j++)
                      _Item(
                        key: ValueKey('block-hit-${_blocks[j].id}'),
                        selected: _hits.length + j == _selected,
                        onTap: () => _open(_hits.length + j),
                        onMiddle: () => _open(_hits.length + j, newTab: true),
                        icon: Icons.notes_rounded,
                        title: _blocks[j].pageTitle,
                        snippet: _Snippet(_blocks[j].content, _query),
                      ),
                    if (_offerCreate)
                      _Item(
                        selected: _count - 1 == _selected,
                        onTap: () => _open(_count - 1),
                        onMiddle: () => _open(_count - 1, newTab: true),
                        icon: Icons.add_rounded,
                        title: 'Create page “$_query”',
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
    super.key,
    required this.selected,
    required this.onTap,
    required this.icon,
    required this.title,
    required this.onMiddle,
    this.detail,
    this.snippet,
    this.pageType,
  });
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onMiddle;
  final IconData icon;
  final String title;
  final String? detail;

  /// Type of the page hit (its emoji goes before the title).
  final String? pageType;

  /// Matching text shown under the title (content results).
  final Widget? snippet;

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
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        TypeIcon(pageType),
                        Expanded(
                          child: Text(
                            title,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 14, color: colors.ink),
                          ),
                        ),
                      ],
                    ),
                    if (snippet != null) ...[
                      const SizedBox(height: 2),
                      snippet!,
                    ],
                  ],
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

/// One line of a block around the first match of `query`, the match highlighted.
class _Snippet extends StatelessWidget {
  const _Snippet(this.content, this.query);
  final String content;
  final String query;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final text = content.replaceAll(RegExp(r'\s+'), ' ').trim();
    final at = text.toLowerCase().indexOf(query.toLowerCase());
    // Start a little before the match so it stays visible on one line.
    final from = at > 40 ? at - 30 : 0;
    final shown = (from > 0 ? '…' : '') + text.substring(from);
    final base = TextStyle(fontSize: 12.5, color: colors.muted);
    final spans = <TextSpan>[];
    final m = at < 0 ? -1 : at - from + (from > 0 ? 1 : 0);
    if (m < 0 || query.isEmpty) {
      spans.add(TextSpan(text: shown));
    } else {
      spans
        ..add(TextSpan(text: shown.substring(0, m)))
        ..add(
          TextSpan(
            text: shown.substring(m, m + query.length),
            style: TextStyle(
              color: colors.ink,
              fontWeight: FontWeight.w600,
              backgroundColor: colors.highlight,
            ),
          ),
        )
        ..add(TextSpan(text: shown.substring(m + query.length)));
    }
    return Text.rich(
      TextSpan(style: base, children: spans),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
    );
  }
}
