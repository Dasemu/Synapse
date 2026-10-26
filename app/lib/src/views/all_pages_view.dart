import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/dates.dart';
import '../widgets/pickers.dart' show errorText;

enum PageSort {
  edited('Edited'),
  created('Created'),
  title('Title');

  const PageSort(this.label);
  final String label;
}

/// Every non-journal note (hubs are folders, not pages), sortable. The whole list is loaded in
/// batches of 200 and sorted on the client; the list itself is built lazily.
class AllPagesView extends StatefulWidget {
  const AllPagesView({super.key});

  @override
  State<AllPagesView> createState() => _AllPagesViewState();
}

class _AllPagesViewState extends State<AllPagesView> {
  final _pages = <PageDto>[];
  PageSort _sort = PageSort.edited;
  bool _ascending = false;
  bool _selecting = false;
  final _chosen = <String>{};

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final backend = AppScope.read(context).backend;
    var offset = 0;
    while (true) {
      final batch = await backend.listPages(limit: 200, offset: offset);
      if (!mounted) return;
      offset += batch.length;
      setState(() => _pages.addAll(batch.where((p) => !p.isHub)));
      if (batch.length < 200) return;
    }
  }

  List<PageDto> get _sorted {
    final list = [..._pages];
    int cmp(PageDto a, PageDto b) => switch (_sort) {
      PageSort.edited => a.updatedAt.compareTo(b.updatedAt),
      PageSort.created => a.createdAt.compareTo(b.createdAt),
      PageSort.title => a.title.toLowerCase().compareTo(b.title.toLowerCase()),
    };
    list.sort((a, b) {
      final c = cmp(a, b);
      // Time sorts default to newest first, so "descending" is the natural direction there.
      final flip = _sort == PageSort.title ? _ascending : !_ascending;
      return flip ? c : -c;
    });
    return list;
  }

  Future<void> _cleanUp() async {
    final app = AppScope.read(context);
    final candidates = await app.backend.legacyCandidates();
    if (!mounted) return;
    final removed = await showDialog<List<String>>(
      context: context,
      builder: (_) => CleanupDialog(candidates: candidates),
    );
    if (removed == null || removed.isEmpty) return;
    await app.queue.run(() => app.backend.purgePages(removed));
    app.recent.removeWhere((p) => removed.contains(p.id));
    app.reload();
  }

  Future<void> _newPage() async {
    final app = AppScope.read(context);
    final id = await showDialog<String>(
      context: context,
      builder: (_) => NewPageDialog(app: app),
    );
    if (id == null) return;
    app.go(PageTarget(id: id));
  }

  /// Asks for confirmation, then deletes `targets` and tidies tabs, recents and the list.
  Future<void> _delete(List<PageDto> targets) async {
    if (targets.isEmpty) return;
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => DeletePagesDialog(pages: targets),
    );
    if (ok != true) return;
    final ids = [for (final p in targets) p.id];
    try {
      for (final id in ids) {
        await app.queue.run(() => app.backend.deletePage(id));
      }
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
      app.reload();
      return;
    }
    if (mounted) {
      setState(() {
        _pages.removeWhere((p) => ids.contains(p.id));
        _chosen.removeAll(ids);
        if (_chosen.isEmpty) _selecting = false;
      });
    }
    app.pagesDeleted(ids);
    messenger.showSnackBar(
      SnackBar(
        content: Text(
          'Deleted ${ids.length} ${ids.length == 1 ? 'page' : 'pages'}',
        ),
      ),
    );
  }

  void _toggleSelect() => setState(() {
    _selecting = !_selecting;
    _chosen.clear();
  });

  Future<void> _rowMenu(PageDto p, Offset at) async {
    final app = AppScope.read(context);
    final v = await showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(at.dx, at.dy, at.dx, at.dy),
      items: _menuItems,
    );
    if (v == 'tab') app.openIdInNewTab(p.id);
    if (v == 'delete') await _delete([p]);
  }

  static const _menuItems = [
    PopupMenuItem(value: 'tab', child: Text('Open in new tab')),
    PopupMenuItem(value: 'delete', child: Text('Delete…')),
  ];

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final colors = context.colors;
    final pages = _sorted;
    return LayoutBuilder(
      builder: (context, box) => ListView.builder(
        padding: EdgeInsets.fromLTRB(
          sidePadding(box.maxWidth),
          36,
          sidePadding(box.maxWidth),
          80,
        ),
        itemCount: pages.length + 1,
        itemBuilder: (context, i) {
          if (i == 0) {
            final title = Text(
              'All pages',
              style: Theme.of(context).textTheme.headlineLarge,
            );
            final actions = <Widget>[
              PopupMenuButton<PageSort>(
                key: const ValueKey('page-sort'),
                tooltip: 'Sort by',
                initialValue: _sort,
                onSelected: (v) => setState(() {
                  _sort = v;
                  _ascending = false;
                }),
                itemBuilder: (_) => [
                  for (final v in PageSort.values)
                    PopupMenuItem(value: v, child: Text(v.label)),
                ],
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  child: Text(
                    _sort.label,
                    style: TextStyle(fontSize: 13, color: colors.muted),
                  ),
                ),
              ),
              IconButton(
                key: const ValueKey('page-sort-direction'),
                tooltip: _ascending ? 'Ascending' : 'Descending',
                visualDensity: VisualDensity.compact,
                iconSize: 18,
                color: colors.muted,
                icon: Icon(
                  _ascending
                      ? Icons.arrow_upward_rounded
                      : Icons.arrow_downward_rounded,
                ),
                onPressed: () => setState(() => _ascending = !_ascending),
              ),
              const SizedBox(width: 8),
              TextLink(
                'Clean up',
                icon: Icons.cleaning_services_outlined,
                onTap: _cleanUp,
              ),
              const SizedBox(width: 12),
              TextLink(
                _selecting ? 'Done' : 'Select',
                key: const ValueKey('pages-select'),
                icon: _selecting
                    ? Icons.check_rounded
                    : Icons.checklist_rounded,
                onTap: _toggleSelect,
              ),
              const SizedBox(width: 12),
              if (_selecting)
                FilledButton(
                  key: const ValueKey('pages-delete-selected'),
                  onPressed: _chosen.isEmpty
                      ? null
                      : () => _delete([
                          for (final p in _pages)
                            if (_chosen.contains(p.id)) p,
                        ]),
                  child: Text('Delete (${_chosen.length})'),
                )
              else
                FilledButton.icon(
                  key: const ValueKey('new-page'),
                  onPressed: _newPage,
                  icon: const Icon(Icons.add_rounded, size: 18),
                  label: const Text('New page'),
                ),
            ];
            return Padding(
              padding: const EdgeInsets.only(bottom: 20),
              // Narrow widths: the actions wrap below the title.
              child: box.maxWidth < 760
                  ? Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        title,
                        const SizedBox(height: 8),
                        Wrap(
                          crossAxisAlignment: WrapCrossAlignment.center,
                          runSpacing: 6,
                          children: actions,
                        ),
                      ],
                    )
                  : Row(
                      children: [
                        Expanded(child: title),
                        ...actions,
                      ],
                    ),
            );
          }
          final p = pages[i - 1];
          final edited = DateTime.fromMillisecondsSinceEpoch(
            p.updatedAt.toInt(),
          );
          return InkWell(
            key: ValueKey('page-row-${p.id}'),
            borderRadius: BorderRadius.circular(8),
            onTap: _selecting
                ? () => setState(
                    () => _chosen.contains(p.id)
                        ? _chosen.remove(p.id)
                        : _chosen.add(p.id),
                  )
                : () => app.navigate(PageTarget(id: p.id)),
            onSecondaryTapUp: _selecting
                ? null
                : (d) => _rowMenu(p, d.globalPosition),
            child: GestureDetector(
              onTertiaryTapUp: _selecting
                  ? null
                  : (_) => app.openIdInNewTab(p.id),
              onLongPressStart: _selecting
                  ? null
                  : (d) => _rowMenu(p, d.globalPosition),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 9, horizontal: 8),
                child: Row(
                  children: [
                    if (_selecting)
                      Padding(
                        padding: const EdgeInsets.only(right: 10),
                        child: Icon(
                          _chosen.contains(p.id)
                              ? Icons.check_box_rounded
                              : Icons.check_box_outline_blank_rounded,
                          key: ValueKey('page-check-${p.id}'),
                          size: 20,
                          color: _chosen.contains(p.id)
                              ? colors.accent
                              : colors.faint,
                        ),
                      ),
                    Expanded(
                      child: Text(
                        p.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 14.5, color: colors.ink),
                      ),
                    ),
                    if (p.originJournal != null)
                      Text(
                        'born ${shortDate(parseDay(p.originJournal!))}',
                        style: TextStyle(fontSize: 12, color: colors.faint),
                      ),
                    const SizedBox(width: 16),
                    SizedBox(
                      width: 70,
                      child: Text(
                        shortDate(edited),
                        textAlign: TextAlign.right,
                        style: TextStyle(fontSize: 12, color: colors.muted),
                      ),
                    ),
                    if (!_selecting)
                      PopupMenuButton<String>(
                        key: ValueKey('page-menu-${p.id}'),
                        tooltip: 'More',
                        iconSize: 18,
                        padding: EdgeInsets.zero,
                        icon: Icon(
                          Icons.more_horiz_rounded,
                          color: colors.faint,
                        ),
                        itemBuilder: (_) => _menuItems,
                        onSelected: (v) {
                          if (v == 'tab') app.openIdInNewTab(p.id);
                          if (v == 'delete') _delete([p]);
                        },
                      )
                    else
                      const SizedBox(width: 8),
                  ],
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}

/// Lists legacy Logseq pages with checkboxes; pops with the ids to delete (or null).
class CleanupDialog extends StatefulWidget {
  const CleanupDialog({super.key, required this.candidates});
  final List<LegacyCandidate> candidates;

  @override
  State<CleanupDialog> createState() => _CleanupDialogState();
}

class _CleanupDialogState extends State<CleanupDialog> {
  late final Set<String> _checked = {
    for (final c in widget.candidates)
      if (c.preselected) c.id,
  };

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final items = widget.candidates;
    return AlertDialog(
      title: const Text('Remove legacy Logseq pages'),
      content: SizedBox(
        width: 460,
        child: items.isEmpty
            ? Text(
                'No legacy pages found.',
                style: TextStyle(color: colors.muted),
              )
            : Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'This deletes the selected pages and their blocks.',
                    style: TextStyle(fontSize: 13, color: colors.muted),
                  ),
                  const SizedBox(height: 8),
                  Flexible(
                    child: ListView(
                      shrinkWrap: true,
                      children: [
                        for (final c in items)
                          CheckboxListTile(
                            dense: true,
                            contentPadding: EdgeInsets.zero,
                            controlAffinity: ListTileControlAffinity.leading,
                            value: _checked.contains(c.id),
                            onChanged: (v) => setState(
                              () => v == true
                                  ? _checked.add(c.id)
                                  : _checked.remove(c.id),
                            ),
                            title: Text(c.title),
                            subtitle: Text(
                              '${c.reason} · ${c.blocks} '
                              '${c.blocks == 1 ? 'block' : 'blocks'}',
                              style: TextStyle(
                                fontSize: 12,
                                color: colors.faint,
                              ),
                            ),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        if (items.isNotEmpty)
          TextButton(
            onPressed: _checked.isEmpty
                ? null
                : () => Navigator.pop(context, _checked.toList()),
            child: Text('Delete (${_checked.length})'),
          ),
      ],
    );
  }
}

/// Asks for a page title. Pops with the id of the created page, or of the existing one when
/// the user picks "Open it" after a duplicate-title error.
class NewPageDialog extends StatefulWidget {
  const NewPageDialog({super.key, required this.app});
  final AppState app;

  @override
  State<NewPageDialog> createState() => _NewPageDialogState();
}

class _NewPageDialogState extends State<NewPageDialog> {
  final _text = TextEditingController();
  String? _error;
  String? _existingId;
  bool _busy = false;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _create() async {
    final title = _text.text.trim();
    if (title.isEmpty) {
      setState(() {
        _error = 'Enter a title';
        _existingId = null;
      });
      return;
    }
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
      _existingId = null;
    });
    final app = widget.app;
    try {
      final existing = await app.backend.pageByTitle(title);
      if (existing != null) {
        if (!mounted) return;
        setState(() {
          _busy = false;
          _error = 'A page titled "${existing.page.title}" already exists';
          _existingId = existing.page.id;
        });
        return;
      }
      final page = await app.queue.run(() => app.backend.openPage(title));
      if (mounted) Navigator.pop(context, page.page.id);
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = errorText(e);
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('New page'),
    content: SizedBox(
      width: 380,
      child: TextField(
        key: const ValueKey('new-page-title'),
        controller: _text,
        autofocus: true,
        decoration: InputDecoration(hintText: 'Title', errorText: _error),
        onChanged: (_) {
          if (_error != null) {
            setState(() {
              _error = null;
              _existingId = null;
            });
          }
        },
        onSubmitted: (_) => _create(),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      if (_existingId != null)
        TextButton(
          key: const ValueKey('open-existing'),
          onPressed: () => Navigator.pop(context, _existingId),
          child: const Text('Open it'),
        ),
      FilledButton(
        key: const ValueKey('create-page'),
        onPressed: _create,
        child: const Text('Create'),
      ),
    ],
  );
}

/// Confirmation before deleting pages; pops true when confirmed.
class DeletePagesDialog extends StatelessWidget {
  const DeletePagesDialog({super.key, required this.pages});
  final List<PageDto> pages;

  @override
  Widget build(BuildContext context) {
    final n = pages.length;
    final hubs = pages.any((p) => p.isHub);
    final journals = pages.any((p) => p.journalDate != null);
    return AlertDialog(
      title: Text('Delete $n ${n == 1 ? 'page' : 'pages'}?'),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Their blocks are removed; links to them stay as plain references.',
            ),
            if (hubs) ...[
              const SizedBox(height: 8),
              const Text(
                "Hubs: their members leave the hub, and the hub's template "
                'is deleted with it.',
              ),
            ],
            if (journals) ...[
              const SizedBox(height: 8),
              const Text('This includes journal pages.'),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const ValueKey('confirm-delete'),
          onPressed: () => Navigator.pop(context, true),
          child: const Text('Delete'),
        ),
      ],
    );
  }
}
