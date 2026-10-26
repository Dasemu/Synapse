import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import 'common.dart';
import 'pickers.dart';
import 'type_icon.dart';

/// The page's metadata as one compact wrapping line: the type pill, the tag chips (sorted;
/// tapping one opens its [TagRoute]) and a "+ tag" link. A page has at most one type; tags
/// are names, not pages.
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

  Future<void> _pickType(BuildContext context) async {
    final app = AppScope.read(context);
    final catalog = await app.backend.typeCatalog();
    if (!context.mounted) return;
    final picked = await showDialog<_TypeChoice>(
      context: context,
      builder: (_) => _TypePicker(catalog: catalog, current: page.pageType),
    );
    if (picked == null || !context.mounted) return;
    await _run(context, (b) => b.setPageType(page.id, picked.name));
  }

  Future<void> _addTags(BuildContext context) async {
    final app = AppScope.read(context);
    final catalog = await app.backend.tagCatalog();
    if (!context.mounted) return;
    await showDialog<void>(
      context: context,
      builder: (_) => _TagPicker(
        catalog: catalog,
        taken: page.tags,
        onAdd: (name) => _run(context, (b) => b.addPageTag(page.id, name)),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final colors = context.colors;
    final type = page.pageType;
    return Wrap(
      spacing: 6,
      runSpacing: 6,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        if (type != null)
          _TypePill(name: type, onTap: () => _pickType(context))
        else
          _GhostPill(
            key: const ValueKey('add-type'),
            label: 'Add type',
            onTap: () => _pickType(context),
          ),
        for (final t in page.tags)
          _TagChip(
            name: t,
            onTap: () => app.openTag(t),
            onRemove: () => _run(context, (b) => b.removePageTag(page.id, t)),
          ),
        TextLink(
          '+ tag',
          key: const ValueKey('add-tag'),
          onTap: () => _addTags(context),
          style: TextStyle(fontSize: 12.5, color: colors.faint),
        ),
      ],
    );
  }
}

class _TypePill extends StatelessWidget {
  const _TypePill({required this.name, required this.onTap});
  final String name;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        key: const ValueKey('type-pill'),
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
          decoration: BoxDecoration(
            color: colors.accentSoft,
            borderRadius: BorderRadius.circular(6),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              TypeIcon(name, size: 12, gap: 4),
              Text(
                name,
                style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                  color: colors.accent,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Faint outlined pill that invites setting the (missing) type.
class _GhostPill extends StatelessWidget {
  const _GhostPill({super.key, required this.label, required this.onTap});
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
          decoration: BoxDecoration(
            border: Border.all(color: colors.line),
            borderRadius: BorderRadius.circular(6),
          ),
          child: Text(
            label,
            style: TextStyle(fontSize: 12.5, color: colors.faint),
          ),
        ),
      ),
    );
  }
}

/// `#name` outlined chip; hovering shows the accent border and a remove button.
class _TagChip extends StatefulWidget {
  const _TagChip({
    required this.name,
    required this.onTap,
    required this.onRemove,
  });
  final String name;
  final VoidCallback onTap;
  final VoidCallback onRemove;

  @override
  State<_TagChip> createState() => _TagChipState();
}

class _TagChipState extends State<_TagChip> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: Container(
        key: ValueKey('tag-${widget.name}'),
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        decoration: BoxDecoration(
          border: Border.all(color: _hover ? colors.accent : colors.line),
          borderRadius: BorderRadius.circular(999),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: widget.onTap,
              child: Text(
                '#${widget.name}',
                style: TextStyle(fontSize: 12.5, color: colors.muted),
              ),
            ),
            AnimatedSize(
              duration: const Duration(milliseconds: 120),
              child: _hover
                  ? GestureDetector(
                      key: ValueKey('untag-${widget.name}'),
                      behavior: HitTestBehavior.opaque,
                      onTap: widget.onRemove,
                      child: Padding(
                        padding: const EdgeInsets.only(left: 4),
                        child: Icon(
                          Icons.close_rounded,
                          size: 12,
                          color: colors.accent,
                        ),
                      ),
                    )
                  : const SizedBox.shrink(),
            ),
          ],
        ),
      ),
    );
  }
}

/// Result of the type picker: a name, or `null` to remove the type.
class _TypeChoice {
  const _TypeChoice(this.name);
  final String? name;
}

/// Single-select type picker: types in use (with counts), suggestions, free text and
/// "Remove type".
class _TypePicker extends StatefulWidget {
  const _TypePicker({required this.catalog, required this.current});
  final List<TagDto> catalog;
  final String? current;

  @override
  State<_TypePicker> createState() => _TypePickerState();
}

class _TypePickerState extends State<_TypePicker> {
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
    final q = _query.toLowerCase();
    final cur = widget.current?.toLowerCase();
    final options = [
      for (final t in widget.catalog)
        if (t.name.toLowerCase() != cur && t.name.toLowerCase().contains(q)) t,
    ];
    final exact = widget.catalog.any((t) => t.name.toLowerCase() == q);
    return AlertDialog(
      title: const Text('Type'),
      content: SizedBox(
        width: 360,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              key: const ValueKey('type-input'),
              controller: _text,
              autofocus: true,
              decoration: const InputDecoration(
                hintText: 'Search or type a new type',
              ),
              onChanged: (_) => setState(() {}),
              onSubmitted: (_) {
                if (_query.isNotEmpty) {
                  Navigator.pop(context, _TypeChoice(_query));
                }
              },
            ),
            const SizedBox(height: 12),
            Flexible(
              child: SingleChildScrollView(
                child: Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    if (_query.isNotEmpty && !exact && q != cur)
                      ActionChip(
                        key: const ValueKey('type-create'),
                        avatar: const Icon(Icons.add_rounded, size: 14),
                        label: Text('Create "$_query"'),
                        onPressed: () =>
                            Navigator.pop(context, _TypeChoice(_query)),
                      ),
                    for (final t in options)
                      ActionChip(
                        key: ValueKey('type-opt-${t.name}'),
                        avatar: t.icon == null ? null : Text(t.icon!),
                        label: Text(
                          t.count > 0 ? '${t.name} · ${t.count}' : t.name,
                          style: TextStyle(
                            fontSize: 12.5,
                            color: t.count > 0 ? colors.ink : colors.muted,
                          ),
                        ),
                        onPressed: () =>
                            Navigator.pop(context, _TypeChoice(t.name)),
                      ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
      actions: [
        if (widget.current != null)
          TextButton(
            key: const ValueKey('type-remove'),
            onPressed: () => Navigator.pop(context, const _TypeChoice(null)),
            child: const Text('Remove type'),
          ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

/// Multi-add tag picker: every choice is applied at once and the dialog stays open.
class _TagPicker extends StatefulWidget {
  const _TagPicker({
    required this.catalog,
    required this.taken,
    required this.onAdd,
  });
  final List<TagDto> catalog;
  final List<String> taken;
  final Future<void> Function(String name) onAdd;

  @override
  State<_TagPicker> createState() => _TagPickerState();
}

class _TagPickerState extends State<_TagPicker> {
  final _text = TextEditingController();
  late final _taken = {for (final t in widget.taken) t.toLowerCase()};

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  String get _query =>
      _text.text.trim().replaceFirst(RegExp(r'^#+'), '').trim();

  void _add(String name) {
    if (name.isEmpty || !_taken.add(name.toLowerCase())) return;
    _text.clear();
    setState(() {});
    widget.onAdd(name);
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final q = _query.toLowerCase();
    final options = [
      for (final t in widget.catalog)
        if (!_taken.contains(t.name.toLowerCase()) &&
            t.name.toLowerCase().contains(q))
          t,
    ];
    final exact = widget.catalog.any((t) => t.name.toLowerCase() == q);
    return AlertDialog(
      title: const Text('Add tags'),
      content: SizedBox(
        width: 360,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              key: const ValueKey('tag-input'),
              controller: _text,
              autofocus: true,
              decoration: const InputDecoration(
                hintText: 'Search or type a new tag',
              ),
              onChanged: (_) => setState(() {}),
              onSubmitted: (_) => _add(_query),
            ),
            const SizedBox(height: 12),
            Flexible(
              child: SingleChildScrollView(
                child: Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    if (_query.isNotEmpty && !exact && !_taken.contains(q))
                      ActionChip(
                        key: const ValueKey('tag-create'),
                        avatar: const Icon(Icons.add_rounded, size: 14),
                        label: Text('Create "$_query"'),
                        onPressed: () => _add(_query),
                      ),
                    for (final t in options)
                      ActionChip(
                        key: ValueKey('tag-opt-${t.name}'),
                        label: Text(
                          '${t.name} · ${t.count}',
                          style: TextStyle(fontSize: 12.5, color: colors.ink),
                        ),
                        onPressed: () => _add(t.name),
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
          key: const ValueKey('tag-done'),
          onPressed: () => Navigator.pop(context),
          child: const Text('Done'),
        ),
      ],
    );
  }
}
