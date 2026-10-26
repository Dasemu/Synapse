import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/emoji_picker.dart';
import '../widgets/pickers.dart';

/// Manages the vocabulary. Types: emoji (shown before the names of their pages), name and
/// page count; create, rename, delete. Tags: name, page count and where they come from
/// (listed on pages, a type, a hub); explicit ones can be renamed or removed. Tapping a name
/// opens its tag view.
class TagsTypesView extends StatefulWidget {
  const TagsTypesView({super.key});

  @override
  State<TagsTypesView> createState() => _TagsTypesViewState();
}

class _TagsTypesViewState extends State<TagsTypesView> {
  bool _tags = false;
  late Future<(List<TagDto>, List<TagDto>)> _data = _load();

  Future<(List<TagDto>, List<TagDto>)> _load() async {
    final backend = AppScope.read(context).backend;
    return (await backend.typeCatalog(), await backend.tagCatalog());
  }

  void _reload() {
    AppScope.read(context).loadTypeIcons().ignore();
    setState(() {
      _data = _load();
    });
  }

  /// Runs a write through the queue (errors become a snack bar), then reloads the lists.
  Future<void> _run(Future<void> Function(GraphBackend b) op) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    try {
      await app.queue.run(() => op(app.backend), report: false);
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    } finally {
      if (mounted) _reload();
    }
  }

  Future<void> _newType() async {
    final app = AppScope.read(context);
    await showNameDialog<String>(
      context,
      heading: 'New type',
      confirm: 'Create',
      hint: 'Type name',
      submit: (name) =>
          app.queue.run(() => app.backend.createType(name), report: false),
    );
    if (mounted) _reload();
  }

  Future<void> _pickIcon(TagDto t) async {
    final picked = await showEmojiPicker(
      context,
      title: 'Emoji for ${t.name}',
      current: t.icon,
    );
    if (picked == null || picked.emoji == t.icon) return;
    await _run((b) => b.setTypeIcon(t.name, picked.emoji));
  }

  Future<void> _renameType(TagDto t) async {
    final app = AppScope.read(context);
    await showNameDialog<int>(
      context,
      heading: 'Rename type',
      confirm: 'Rename',
      hint: 'Type name',
      initial: t.name,
      submit: (name) => app.queue.run(
        () => app.backend.renameType(t.name, name),
        report: false,
      ),
    );
    if (mounted) _reload();
  }

  Future<void> _deleteType(TagDto t) async {
    final ok = await _confirm(
      title: 'Delete type “${t.name}”?',
      body: t.count == 0
          ? 'No page has this type. Its emoji is removed too.'
          : '${_pages(t.count)} lose this type (their tags stay). '
                'Its emoji is removed too.',
      action: 'Delete',
    );
    if (ok) await _run((b) => b.removeType(t.name));
  }

  Future<void> _renameTag(TagDto t) async {
    final app = AppScope.read(context);
    await showNameDialog<int>(
      context,
      heading: 'Rename tag',
      confirm: 'Rename',
      hint: 'Tag name',
      initial: t.name,
      submit: (name) => app.queue.run(
        () => app.backend.renameTag(t.name, name),
        report: false,
      ),
    );
    if (mounted) _reload();
  }

  Future<void> _deleteTag(TagDto t) async {
    final ok = await _confirm(
      title: 'Delete tag #${t.name}?',
      body:
          'It is removed from the ${_pages(t.explicit)} that list it.'
          '${t.typed > 0 ? ' Pages with the type “${t.name}” keep it.' : ''}'
          '${t.fromHub > 0 ? ' Members of the hub “${t.name}” keep it.' : ''}',
      action: 'Delete',
    );
    if (ok) await _run((b) => b.removeTag(t.name));
  }

  Future<bool> _confirm({
    required String title,
    required String body,
    required String action,
  }) async =>
      await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: Text(title),
          content: Text(body),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel'),
            ),
            TextButton(
              key: const ValueKey('tt-confirm'),
              onPressed: () => Navigator.pop(ctx, true),
              child: Text(action),
            ),
          ],
        ),
      ) ??
      false;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return FutureBuilder<(List<TagDto>, List<TagDto>)>(
      future: _data,
      builder: (context, snap) {
        final data = snap.data;
        return LayoutBuilder(
          builder: (context, box) => ListView(
            padding: EdgeInsets.fromLTRB(
              sidePadding(box.maxWidth),
              36,
              sidePadding(box.maxWidth),
              80,
            ),
            children: [
              Text(
                'Tags & types',
                style: Theme.of(context).textTheme.headlineLarge,
              ),
              const SizedBox(height: 6),
              Text(
                'A type says what a page is (one per page) and shows its emoji '
                'before the page’s name. Tags file pages under topics.',
                style: TextStyle(fontSize: 12.5, color: colors.faint),
              ),
              const SizedBox(height: 16),
              Row(
                children: [
                  SegmentedButton<bool>(
                    showSelectedIcon: false,
                    segments: [
                      ButtonSegment(
                        value: false,
                        label: Text(
                          'Types',
                          key: const ValueKey('tt-tab-types'),
                        ),
                      ),
                      ButtonSegment(
                        value: true,
                        label: Text('Tags', key: const ValueKey('tt-tab-tags')),
                      ),
                    ],
                    selected: {_tags},
                    onSelectionChanged: (s) => setState(() => _tags = s.first),
                  ),
                  const Spacer(),
                  if (!_tags)
                    TextButton.icon(
                      key: const ValueKey('new-type'),
                      onPressed: _newType,
                      icon: const Icon(Icons.add_rounded, size: 18),
                      label: const Text('New type'),
                    ),
                ],
              ),
              const SizedBox(height: 12),
              if (data == null)
                const SizedBox.shrink()
              else if (_tags)
                ..._tagRows(data.$2)
              else
                ..._typeRows(data.$1),
            ],
          ),
        );
      },
    );
  }

  Iterable<Widget> _typeRows(List<TagDto> types) sync* {
    final colors = context.colors;
    final app = AppScope.read(context);
    if (types.isEmpty) {
      yield Text('No types yet.', style: TextStyle(color: colors.muted));
    }
    for (final t in types) {
      yield _Row(
        key: ValueKey('type-row-${t.name}'),
        leading: Tooltip(
          message: t.icon == null ? 'Choose an emoji' : 'Change the emoji',
          child: InkWell(
            key: ValueKey('type-icon-btn-${t.name}'),
            borderRadius: BorderRadius.circular(8),
            onTap: () => _pickIcon(t),
            child: Container(
              width: 34,
              height: 34,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: colors.surface,
                border: Border.all(color: colors.line),
                borderRadius: BorderRadius.circular(8),
              ),
              child: t.icon == null
                  ? Icon(
                      Icons.add_reaction_outlined,
                      size: 17,
                      color: colors.faint,
                    )
                  : Text(t.icon!, style: const TextStyle(fontSize: 18)),
            ),
          ),
        ),
        name: t.name,
        detail: t.count == 0
            ? (t.suggested ? 'suggested · no pages' : 'no pages')
            : _pages(t.count),
        onOpen: () => app.navigate(TagRoute(t.name)),
        menu: PopupMenuButton<String>(
          key: ValueKey('type-menu-${t.name}'),
          tooltip: 'Type actions',
          icon: Icon(Icons.more_horiz_rounded, color: colors.muted, size: 20),
          onSelected: (v) => switch (v) {
            'icon' => _pickIcon(t),
            'rename' => _renameType(t),
            _ => _deleteType(t),
          },
          itemBuilder: (_) => const [
            PopupMenuItem(value: 'icon', child: Text('Emoji…')),
            PopupMenuItem(value: 'rename', child: Text('Rename…')),
            PopupMenuItem(value: 'delete', child: Text('Delete…')),
          ],
        ),
      );
    }
  }

  Iterable<Widget> _tagRows(List<TagDto> tags) sync* {
    final colors = context.colors;
    final app = AppScope.read(context);
    if (tags.isEmpty) {
      yield Text('No tags yet.', style: TextStyle(color: colors.muted));
    }
    for (final t in tags) {
      final sources = [
        if (t.explicit > 0) 'listed on ${t.explicit}',
        if (t.typed > 0) 'type',
        if (t.fromHub > 0) 'hub',
      ];
      final String? locked = t.explicit > 0
          ? null
          : t.fromHub > 0
          ? 'Comes from the hub “${t.name}”: rename or delete the hub instead.'
          : 'Comes from the type “${t.name}”: manage it under Types.';
      yield _Row(
        key: ValueKey('tag-row-${t.name}'),
        leading: Icon(Icons.sell_outlined, size: 17, color: colors.faint),
        name: '#${t.name}',
        detail: '${_pages(t.count)} · ${sources.join(', ')}',
        onOpen: () => app.navigate(TagRoute(t.name)),
        menu: locked != null
            ? Tooltip(
                key: ValueKey('tag-locked-${t.name}'),
                message: locked,
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: Icon(
                    Icons.lock_outline_rounded,
                    size: 17,
                    color: colors.faint,
                  ),
                ),
              )
            : PopupMenuButton<String>(
                key: ValueKey('tag-menu-${t.name}'),
                tooltip: 'Tag actions',
                icon: Icon(
                  Icons.more_horiz_rounded,
                  color: colors.muted,
                  size: 20,
                ),
                onSelected: (v) =>
                    v == 'rename' ? _renameTag(t) : _deleteTag(t),
                itemBuilder: (_) => const [
                  PopupMenuItem(value: 'rename', child: Text('Rename…')),
                  PopupMenuItem(value: 'delete', child: Text('Delete…')),
                ],
              ),
      );
    }
  }
}

String _pages(int n) => '$n ${n == 1 ? 'page' : 'pages'}';

class _Row extends StatelessWidget {
  const _Row({
    super.key,
    required this.leading,
    required this.name,
    required this.detail,
    required this.onOpen,
    required this.menu,
  });
  final Widget leading;
  final String name;
  final String detail;
  final VoidCallback onOpen;
  final Widget menu;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        children: [
          leading,
          const SizedBox(width: 12),
          Expanded(
            child: InkWell(
              borderRadius: BorderRadius.circular(6),
              onTap: onOpen,
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 6),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 14.5, color: colors.ink),
                    ),
                    Text(
                      detail,
                      style: TextStyle(fontSize: 12, color: colors.faint),
                    ),
                  ],
                ),
              ),
            ),
          ),
          menu,
        ],
      ),
    );
  }
}
