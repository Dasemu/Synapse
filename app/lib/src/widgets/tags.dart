import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import 'common.dart';
import 'pickers.dart';

/// Type and tag chips of a page, each with a remove button, and "Type" / "Tag" links that open
/// a picker. Types are plain names; tags are pages (tapping one opens it). Tag pages cannot
/// be tagged, so they only get types.
class TagBar extends StatelessWidget {
  const TagBar({super.key, required this.page});
  final PageDto page;

  Future<void> _run(
    BuildContext context,
    Future<PageDto> Function(GraphBackend b) op,
  ) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    try {
      await app.queue.run(() => op(app.backend), report: false);
      app.reload();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  Future<void> _add(BuildContext context, {required bool type}) async {
    final app = AppScope.read(context);
    final catalog = type
        ? await app.backend.typeCatalog()
        : await app.backend.tagCatalog();
    if (!context.mounted) return;
    final name = await showDialog<String>(
      context: context,
      builder: (_) => _TagPicker(
        catalog: catalog,
        taken: type ? page.types : page.tags,
        label: type ? 'type' : 'tag',
      ),
    );
    if (name == null || !context.mounted) return;
    await _run(
      context,
      (b) => type ? b.addPageType(page.id, name) : b.addPageTag(page.id, name),
    );
  }

  Widget _chip(
    BuildContext context,
    String keyId,
    String name, {
    VoidCallback? onTap,
    required VoidCallback onRemove,
  }) {
    final colors = context.colors;
    return Container(
      key: ValueKey(keyId),
      padding: const EdgeInsets.only(left: 9, right: 3),
      decoration: BoxDecoration(
        color: colors.surface,
        borderRadius: BorderRadius.circular(20),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          InkWell(
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Text(
                name,
                style: TextStyle(fontSize: 12, color: colors.muted),
              ),
            ),
          ),
          InkResponse(
            key: ValueKey('un$keyId'),
            radius: 10,
            onTap: onRemove,
            child: Padding(
              padding: const EdgeInsets.all(4),
              child: Icon(Icons.close_rounded, size: 12, color: colors.faint),
            ),
          ),
        ],
      ),
    );
  }

  Widget _row(
    BuildContext context,
    IconData icon,
    String label,
    List<Widget> chips,
    Widget add,
  ) {
    final colors = context.colors;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 76,
          child: Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Row(
              children: [
                Icon(icon, size: 13, color: colors.faint),
                const SizedBox(width: 5),
                Text(
                  label,
                  style: TextStyle(fontSize: 12, color: colors.faint),
                ),
              ],
            ),
          ),
        ),
        Expanded(
          child: Wrap(
            spacing: 6,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [...chips, add],
          ),
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _row(
          context,
          Icons.category_outlined,
          'Type',
          [
            for (final t in page.types)
              _chip(
                context,
                'type-$t',
                t,
                onRemove: () =>
                    _run(context, (b) => b.removePageType(page.id, t)),
              ),
          ],
          TextLink(
            '+ add',
            key: const ValueKey('add-type'),
            onTap: () => _add(context, type: true),
            style: const TextStyle(fontSize: 12),
          ),
        ),
        if (!page.isTag) ...[
          const SizedBox(height: 6),
          _row(
            context,
            Icons.sell_outlined,
            'Tags',
            [
              for (final t in page.tags)
                _chip(
                  context,
                  'tag-$t',
                  '#$t',
                  onTap: () => app.openTitle(t),
                  onRemove: () =>
                      _run(context, (b) => b.removePageTag(page.id, t)),
                ),
            ],
            TextLink(
              '+ add',
              key: const ValueKey('add-tag'),
              onTap: () => _add(context, type: false),
              style: const TextStyle(fontSize: 12),
            ),
          ),
        ],
      ],
    );
  }
}

/// Notes carrying the tag page `tag`, as links under the header of the tag page.
class TaggedNotes extends StatelessWidget {
  const TaggedNotes({super.key, required this.tag});
  final String tag;

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final colors = context.colors;
    return FutureBuilder<List<PageBrief>>(
      future: app.backend.pagesWithTag(tag),
      builder: (context, snap) {
        final pages = snap.data ?? const <PageBrief>[];
        if (pages.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.only(top: 10),
          child: Wrap(
            key: const ValueKey('tagged-notes'),
            spacing: 14,
            runSpacing: 4,
            children: [
              Text(
                'Tagged',
                style: TextStyle(fontSize: 12, color: colors.faint),
              ),
              for (final p in pages)
                TextLink(
                  p.title,
                  icon: Icons.description_outlined,
                  onTap: () => app.openId(p.id),
                  style: const TextStyle(fontSize: 12),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// Lists the pages carrying `tag`; picking one opens it.
Future<void> showTagPages(BuildContext context, String tag) async {
  final app = AppScope.read(context);
  final pages = await app.backend.pagesWithTag(tag);
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(tag),
      content: SizedBox(
        width: 360,
        child: pages.isEmpty
            ? const Text('No notes with this tag yet.')
            : ListView(
                shrinkWrap: true,
                children: [
                  for (final p in pages)
                    ListTile(
                      dense: true,
                      leading: Icon(
                        p.isHub
                            ? Icons.folder_outlined
                            : Icons.description_outlined,
                        size: 18,
                      ),
                      title: Text(p.title),
                      onTap: () {
                        Navigator.pop(ctx);
                        app.openId(p.id);
                      },
                    ),
                ],
              ),
      ),
    ),
  );
}

class _TagPicker extends StatefulWidget {
  const _TagPicker({
    required this.catalog,
    required this.taken,
    required this.label,
  });
  final List<TagDto> catalog;
  final List<String> taken;
  final String label;

  @override
  State<_TagPicker> createState() => _TagPickerState();
}

class _TagPickerState extends State<_TagPicker> {
  final _text = TextEditingController();

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  String get _query => _text.text.trim().replaceFirst(RegExp(r'^#+'), '');

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final taken = {for (final t in widget.taken) t.toLowerCase()};
    final q = _query.toLowerCase();
    final options = [
      for (final t in widget.catalog)
        if (!taken.contains(t.name.toLowerCase()) &&
            t.name.toLowerCase().contains(q))
          t,
    ];
    final exact = widget.catalog.any((t) => t.name.toLowerCase() == q);
    return AlertDialog(
      title: Text('Add ${widget.label}'),
      content: SizedBox(
        width: 360,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              key: ValueKey('${widget.label}-input'),
              controller: _text,
              autofocus: true,
              decoration: InputDecoration(
                hintText: 'Search or type a new ${widget.label}',
              ),
              onChanged: (_) => setState(() {}),
              onSubmitted: (_) {
                if (_query.isNotEmpty) Navigator.pop(context, _query);
              },
            ),
            const SizedBox(height: 12),
            Flexible(
              child: SingleChildScrollView(
                child: Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    if (_query.isNotEmpty && !exact && !taken.contains(q))
                      ActionChip(
                        key: ValueKey('${widget.label}-create'),
                        avatar: const Icon(Icons.add_rounded, size: 14),
                        label: Text('Create "$_query"'),
                        onPressed: () => Navigator.pop(context, _query),
                      ),
                    for (final t in options)
                      ActionChip(
                        key: ValueKey('${widget.label}-opt-${t.name}'),
                        label: Text(
                          t.count > 0 ? '${t.name} · ${t.count}' : t.name,
                          style: TextStyle(
                            fontSize: 12.5,
                            color: t.count > 0 ? colors.ink : colors.muted,
                          ),
                        ),
                        onPressed: () => Navigator.pop(context, t.name),
                      ),
                  ],
                ),
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
      ],
    );
  }
}
