import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/type_icon.dart';
import '../widgets/common.dart';
import '../widgets/pickers.dart';

/// A tag: the pages carrying it (listed in their tags or implied by their type), with
/// actions to rename it or remove it from every page. A tag is a name, not a page.
class TagView extends StatelessWidget {
  const TagView({super.key, required this.name});
  final String name;

  Future<void> _rename(BuildContext context) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    final next = await showDialog<String>(
      context: context,
      builder: (_) => _RenameDialog(name: name),
    );
    if (next == null || next.isEmpty || next == name) return;
    try {
      await app.queue.run(
        () => app.backend.renameTag(name, next),
        report: false,
      );
      // A type of that name moves its emoji to the new name.
      await app.loadTypeIcons();
      app.replaceRoute(TagRoute(next));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  Future<void> _remove(BuildContext context) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('Remove #$name?'),
        content: const Text(
          'The tag is removed from every page that lists it. Pages whose type '
          'has this name keep it.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel'),
          ),
          TextButton(
            key: const ValueKey('tag-remove-confirm'),
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (ok != true) return;
    try {
      await app.queue.run(() => app.backend.removeTag(name), report: false);
      await app.loadTypeIcons();
      app.reload();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final colors = context.colors;
    return FutureBuilder<List<PageBrief>>(
      future: app.backend.pagesWithTag(name),
      builder: (context, snap) {
        final pages = snap.data;
        return LayoutBuilder(
          builder: (context, box) => ListView(
            padding: EdgeInsets.fromLTRB(
              sidePadding(box.maxWidth),
              36,
              sidePadding(box.maxWidth),
              80,
            ),
            children: [
              Row(
                children: [
                  TypeIcon(name, size: 26, gap: 10),
                  Expanded(
                    child: Text(
                      '#$name',
                      key: const ValueKey('tag-title'),
                      style: Theme.of(context).textTheme.headlineLarge,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              Wrap(
                spacing: 14,
                runSpacing: 4,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Text(
                    pages == null
                        ? ''
                        : '${pages.length} ${pages.length == 1 ? 'page' : 'pages'}',
                    key: const ValueKey('tag-count'),
                    style: TextStyle(fontSize: 12.5, color: colors.faint),
                  ),
                  TextLink(
                    'Rename',
                    key: const ValueKey('tag-rename'),
                    icon: Icons.edit_outlined,
                    onTap: () => _rename(context),
                    style: const TextStyle(fontSize: 12.5),
                  ),
                  TextLink(
                    'Remove from all pages',
                    key: const ValueKey('tag-remove'),
                    icon: Icons.delete_outline_rounded,
                    onTap: () => _remove(context),
                    style: const TextStyle(fontSize: 12.5),
                  ),
                ],
              ),
              const SizedBox(height: 24),
              if (pages != null && pages.isEmpty)
                Text(
                  'No pages carry this tag.',
                  style: TextStyle(color: colors.muted),
                ),
              for (final p in pages ?? const <PageBrief>[])
                InkWell(
                  key: ValueKey('tag-page-${p.id}'),
                  onTap: () => app.openId(p.id),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 8),
                    child: Row(
                      children: [
                        Icon(
                          p.isHub
                              ? Icons.folder_outlined
                              : Icons.description_outlined,
                          size: 16,
                          color: colors.faint,
                        ),
                        const SizedBox(width: 10),
                        TypeIcon(p.pageType, size: 14, gap: 6),
                        Expanded(
                          child: Text(
                            p.title,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 15, color: colors.ink),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}

class _RenameDialog extends StatefulWidget {
  const _RenameDialog({required this.name});
  final String name;

  @override
  State<_RenameDialog> createState() => _RenameDialogState();
}

class _RenameDialogState extends State<_RenameDialog> {
  late final _text = TextEditingController(text: widget.name);

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  String get _value =>
      _text.text.trim().replaceFirst(RegExp(r'^#+'), '').trim();

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Rename tag'),
    content: TextField(
      key: const ValueKey('tag-rename-input'),
      controller: _text,
      autofocus: true,
      onSubmitted: (_) => Navigator.pop(context, _value),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      TextButton(
        key: const ValueKey('tag-rename-confirm'),
        onPressed: () => Navigator.pop(context, _value),
        child: const Text('Rename'),
      ),
    ],
  );
}
