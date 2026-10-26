import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/dates.dart';

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
            return Padding(
              padding: const EdgeInsets.only(bottom: 20),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      'All pages',
                      style: Theme.of(context).textTheme.headlineLarge,
                    ),
                  ),
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
                ],
              ),
            );
          }
          final p = pages[i - 1];
          final edited = DateTime.fromMillisecondsSinceEpoch(
            p.updatedAt.toInt(),
          );
          return InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: () => app.openId(p.id),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 9, horizontal: 8),
              child: Row(
                children: [
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
                ],
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
