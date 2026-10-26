import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/dates.dart';
import '../widgets/pickers.dart';
import '../widgets/tags.dart';
import 'graph_view.dart';
import 'page_screen.dart';

/// A hub: title, short description and its members (sub-hubs first, then notes). It has no
/// outline. Membership is stored on the member, so adding/removing edits the member.
class HubScreen extends StatefulWidget {
  const HubScreen({
    super.key,
    required this.page,
    required this.aliases,
    required this.structure,
    required this.hubSizes,
  });
  final PageDto page;
  final List<String> aliases;
  final PageStructure? structure;
  final Map<String, int> hubSizes;

  @override
  State<HubScreen> createState() => _HubScreenState();
}

class _HubScreenState extends State<HubScreen> {
  late final _desc = TextEditingController(text: widget.page.description);
  late final _focus = FocusNode()..addListener(_blur);
  late String _committed = widget.page.description;
  AppState? _app;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _app = AppScope.read(context);
  }

  @override
  void dispose() {
    _commit();
    _desc.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _blur() {
    if (!_focus.hasFocus) _commit();
  }

  void _commit() {
    final text = _desc.text.trim();
    if (text == _committed) return;
    _committed = text;
    final app = _app!;
    app.queue
        .run(() => app.backend.setHubDescription(widget.page.id, text))
        .ignore();
  }

  Future<void> _add() async {
    final app = AppScope.read(context);
    final hub = widget.page;
    final st = widget.structure;
    final taken = {hub.id, ...?st?.members.map((m) => m.id)};
    final messenger = ScaffoldMessenger.of(context);
    final r = await showPagePicker(
      context,
      heading: 'Add to ${hub.title}',
      canCreateNote: true,
      canCreateHub: true,
      search: (q) async {
        if (q.isEmpty) {
          final l = await app.backend.listPages(limit: 20);
          return [
            for (final p in l)
              if (!taken.contains(p.id))
                PickItem(id: p.id, title: p.title, isHub: p.isHub),
          ];
        }
        final hits = await app.backend.searchTitles(q);
        return [
          for (final h in hits)
            if (!taken.contains(h.pageId))
              PickItem(id: h.pageId, title: h.title),
        ];
      },
    );
    if (r == null || !mounted) return;
    try {
      await app.queue.run(() async {
        final String id;
        if (r.item != null) {
          id = r.item!.id;
        } else if (r.newHub) {
          id = (await app.backend.createHub(r.newTitle!, '')).id;
        } else {
          id = (await app.backend.createPageFromTemplate(
            hub.id,
            r.newTitle!,
          )).id;
        }
        await app.backend.addToHub(id, hub.id);
      }, report: false);
      app.reload();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  Future<void> _openTemplate() async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    try {
      app.openId(await app.backend.hubTemplate(widget.page.id));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  Future<void> _remove(PageBrief m) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    try {
      await app.queue.run(
        () => app.backend.removeFromHub(m.id, widget.page.id),
        report: false,
      );
      app.reload();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final app = AppScope.read(context);
    final st = widget.structure;
    final page = widget.page;
    final members = st?.members ?? const <PageBrief>[];
    return LayoutBuilder(
      builder: (context, box) {
        final pad = sidePadding(box.maxWidth);
        return ListView(
          padding: EdgeInsets.fromLTRB(pad, 36, pad, 120),
          children: [
            if (st != null && st.nsPath.isNotEmpty) ...[
              Breadcrumb(path: st.nsPath),
              const SizedBox(height: 6),
            ],
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: TitleField(page: page)),
                PageMenu(page: page, structure: st),
              ],
            ),
            const SizedBox(height: 8),
            TextField(
              key: const ValueKey('hub-description'),
              controller: _desc,
              focusNode: _focus,
              minLines: 1,
              maxLines: 3,
              style: TextStyle(fontSize: 14.5, color: colors.muted),
              decoration: const InputDecoration.collapsed(
                hintText: 'Add a short description',
              ),
            ),
            const SizedBox(height: 10),
            Wrap(
              spacing: 14,
              runSpacing: 4,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                if (st != null && st.inHubs.isNotEmpty)
                  InHubs(pageId: page.id, hubs: st.inHubs),
                if (page.originJournal != null)
                  TextLink(
                    'Born ${longDate(page.originJournal!)}',
                    icon: Icons.spa_outlined,
                    onTap: () => app.openTitle(page.originJournal!),
                    style: const TextStyle(fontSize: 12),
                  ),
              ],
            ),
            const SizedBox(height: 8),
            TagBar(page: page),
            const SizedBox(height: 28),
            Row(
              children: [
                Expanded(
                  child: SectionLabel(
                    'Members',
                    trailing: Text(
                      '${members.length}',
                      style: TextStyle(fontSize: 11, color: colors.faint),
                    ),
                  ),
                ),
                TextLink(
                  'Template',
                  icon: Icons.article_outlined,
                  onTap: _openTemplate,
                  style: const TextStyle(fontSize: 12.5),
                ),
                const SizedBox(width: 14),
                TextLink(
                  'Add',
                  icon: Icons.add_rounded,
                  onTap: _add,
                  style: const TextStyle(fontSize: 12.5),
                ),
              ],
            ),
            const SizedBox(height: 8),
            if (st != null && members.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: Text(
                  'Empty. Add notes or sub-hubs to organise them here.',
                  style: TextStyle(fontSize: 13, color: colors.muted),
                ),
              ),
            for (final m in members)
              _MemberRow(
                key: ValueKey('member-${m.id}'),
                member: m,
                count: widget.hubSizes[m.id],
                onOpen: () => app.openId(m.id),
                onRemove: () => _remove(m),
              ),
            const SizedBox(height: 36),
            LocalGraphPanel(key: ValueKey('local-${page.id}'), pageId: page.id),
          ],
        );
      },
    );
  }
}

class _MemberRow extends StatefulWidget {
  const _MemberRow({
    super.key,
    required this.member,
    required this.count,
    required this.onOpen,
    required this.onRemove,
  });
  final PageBrief member;
  final int? count;
  final VoidCallback onOpen;
  final VoidCallback onRemove;

  @override
  State<_MemberRow> createState() => _MemberRowState();
}

class _MemberRowState extends State<_MemberRow> {
  bool _hover = false;

  Future<void> _menu(Offset at) async {
    final r = await showMenu<bool>(
      context: context,
      position: RelativeRect.fromLTRB(at.dx, at.dy, at.dx, at.dy),
      items: const [PopupMenuItem(value: true, child: Text('Remove from hub'))],
    );
    if (r == true) widget.onRemove();
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final m = widget.member;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onLongPressStart: (d) => _menu(d.globalPosition),
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: widget.onOpen,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
            child: Row(
              children: [
                Icon(
                  m.isHub ? Icons.folder_outlined : Icons.description_outlined,
                  size: 17,
                  color: colors.accent.withAlpha(0xCC),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    m.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 14.5, color: colors.ink),
                  ),
                ),
                if (m.isHub && widget.count != null)
                  Text(
                    '${widget.count}',
                    style: TextStyle(fontSize: 11.5, color: colors.faint),
                  ),
                SizedBox(
                  width: 28,
                  child: _hover
                      ? InkResponse(
                          key: ValueKey('remove-${m.id}'),
                          radius: 14,
                          onTap: widget.onRemove,
                          child: Icon(
                            Icons.close_rounded,
                            size: 15,
                            color: colors.faint,
                          ),
                        )
                      : null,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
