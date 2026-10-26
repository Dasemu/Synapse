import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../editor/outline_view.dart';
import '../state/app_state.dart';
import '../state/outline_controller.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/dates.dart';
import '../widgets/pickers.dart';
import '../widgets/tags.dart';
import 'all_pages_view.dart' show DeletePagesDialog;
import 'convert_dialog.dart';
import 'graph_view.dart';
import 'hub_screen.dart';
import 'journals_view.dart';

/// A single page: title, aliases, outline, linked references and related notes.
class PageScreen extends StatefulWidget {
  const PageScreen({super.key, required this.target});
  final PageTarget target;

  @override
  State<PageScreen> createState() => _PageScreenState();
}

class _PageScreenState extends State<PageScreen> {
  PageData? _view;
  OutlineController? _outline;
  List<BacklinkGroup>? _backlinks;
  List<RelatedDto>? _related;
  PageStructure? _structure;
  Map<String, int> _hubSizes = const {};
  Object? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _outline?.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    final app = AppScope.read(context);
    try {
      final t = widget.target;
      // Read after the previous view's pending saves (flushed when it is disposed at the
      // end of this frame) have reached the core.
      await WidgetsBinding.instance.endOfFrame;
      final v = await app.queue.run(
        () => t.id != null
            ? app.backend.page(t.id!)
            : app.backend.openPage(t.title!).then<PageData?>((p) => p),
        report: false,
      );
      if (!mounted) return;
      if (v == null) {
        setState(() => _error = 'This page no longer exists.');
        return;
      }
      final view = v;
      setState(() {
        _view = view;
        _outline = OutlineController(
          backend: app.backend,
          queue: app.queue,
          pageId: view.page.id,
          blocks: view.blocks,
        );
      });
      app.pageShown(view.page);
      // Make sure pending writes (e.g. a link just typed elsewhere) are visible.
      await app.queue.idle;
      if (view.page.journalDate == null) await _loadStructure();
      if (view.page.isHub) return;
      final results = await Future.wait([
        app.backend.backlinks(view.page.id),
        app.backend.related(view.page.id),
      ]);
      if (!mounted) return;
      setState(() {
        _backlinks = results[0] as List<BacklinkGroup>;
        _related = results[1] as List<RelatedDto>;
      });
    } catch (e) {
      if (mounted) setState(() => _error = e);
    }
  }

  /// Hub and namespace context of the page (not shown for journals).
  Future<void> _loadStructure() async {
    final app = AppScope.read(context);
    final id = _view!.page.id;
    try {
      final st = await app.backend.pageStructure(id);
      var sizes = const <String, int>{};
      if (st.isHub && st.members.any((m) => m.isHub)) {
        final hubs = await app.backend.hubs();
        sizes = {for (final h in hubs.hubs) h.id: h.members.length};
      }
      if (!mounted) return;
      setState(() {
        _structure = st;
        _hubSizes = sizes;
      });
    } catch (e) {
      debugPrint('page structure failed: $e');
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return Center(
        child: Text('$_error', style: TextStyle(color: context.colors.muted)),
      );
    }
    final view = _view;
    if (view == null) return const SizedBox.shrink();
    if (view.page.isHub) {
      return HubScreen(
        page: view.page,
        aliases: view.aliases,
        structure: _structure,
        hubSizes: _hubSizes,
      );
    }
    return LayoutBuilder(
      builder: (context, box) {
        final open = AppScope.of(context).pageSidebar;
        final wide = box.maxWidth >= 900;
        final side = open && wide;
        final mainWidth = side ? box.maxWidth - 300 : box.maxWidth;
        final pad = sidePadding(mainWidth);
        final scroll = CustomScrollView(
          slivers: [
            const SliverToBoxAdapter(child: SizedBox(height: 36)),
            SliverPadding(
              padding: EdgeInsets.symmetric(horizontal: pad),
              sliver: SliverMainAxisGroup(
                slivers: [
                  SliverToBoxAdapter(
                    child: _Header(view: view, structure: _structure),
                  ),
                  if (_structure != null)
                    SliverToBoxAdapter(
                      child: _NamespaceSection(structure: _structure!),
                    ),
                  OutlineSliver(controller: _outline!),
                  SliverToBoxAdapter(
                    child: _LinkedReferences(groups: _backlinks),
                  ),
                  if (open && !wide) ...[
                    SliverToBoxAdapter(child: RelatedPanel(related: _related)),
                    SliverToBoxAdapter(
                      child: Padding(
                        padding: const EdgeInsets.only(top: 40),
                        child: LocalGraphPanel(pageId: view.page.id),
                      ),
                    ),
                  ],
                  const SliverToBoxAdapter(child: SizedBox(height: 120)),
                ],
              ),
            ),
          ],
        );
        if (!side) return scroll;
        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(child: scroll),
            Container(
              width: 300,
              decoration: BoxDecoration(
                border: Border(left: BorderSide(color: context.colors.line)),
              ),
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(20, 44, 20, 40),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    LocalGraphPanel(pageId: view.page.id),
                    const SizedBox(height: 32),
                    RelatedPanel(related: _related, compact: true),
                  ],
                ),
              ),
            ),
          ],
        );
      },
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.view, required this.structure});
  final PageData view;
  final PageStructure? structure;

  @override
  Widget build(BuildContext context) {
    final page = view.page;
    final colors = context.colors;
    final journal = page.journalDate;
    final app = AppScope.read(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (journal != null) ...[
            Text(
              longDate(journal),
              style: Theme.of(context).textTheme.headlineLarge,
            ),
            const SizedBox(height: 4),
            Row(
              children: [
                Text(
                  weekday(journal),
                  style: TextStyle(color: colors.muted, fontSize: 13),
                ),
                const SizedBox(width: 14),
                JournalChain(view: view),
              ],
            ),
          ] else ...[
            if (structure != null && structure!.nsPath.isNotEmpty) ...[
              Breadcrumb(path: structure!.nsPath),
              const SizedBox(height: 6),
            ],
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: TitleField(page: page)),
                IconButton(
                  key: const ValueKey('page-sidebar-toggle'),
                  tooltip: AppScope.of(context).pageSidebar
                      ? 'Hide graph sidebar'
                      : 'Show graph sidebar',
                  visualDensity: VisualDensity.compact,
                  iconSize: 20,
                  color: AppScope.of(context).pageSidebar
                      ? colors.accent
                      : colors.muted,
                  icon: const Icon(Icons.view_sidebar_outlined),
                  onPressed: AppScope.read(context).togglePageSidebar,
                ),
                PageMenu(page: page, structure: structure),
              ],
            ),
            const SizedBox(height: 6),
            Wrap(
              spacing: 14,
              runSpacing: 4,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                if (structure != null && structure!.inHubs.isNotEmpty)
                  InHubs(pageId: page.id, hubs: structure!.inHubs),
                if (page.originJournal != null)
                  TextLink(
                    'Born ${longDate(page.originJournal!)}',
                    icon: Icons.spa_outlined,
                    onTap: () => app.openTitle(page.originJournal!),
                    style: const TextStyle(fontSize: 12),
                  ),
                _Aliases(page: page, initial: view.aliases),
              ],
            ),
            const SizedBox(height: 8),
            TagBar(page: page),
          ],
        ],
      ),
    );
  }
}

/// Page title; editing it renames the page (and rewrites every link to it).
class TitleField extends StatefulWidget {
  const TitleField({super.key, required this.page});
  final PageDto page;

  @override
  State<TitleField> createState() => TitleFieldState();
}

class TitleFieldState extends State<TitleField> {
  late final _text = TextEditingController(text: widget.page.title);
  late final _focus = FocusNode()..addListener(_blur);
  late String _committed = widget.page.title;

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _blur() {
    if (!_focus.hasFocus) _commit();
  }

  Future<void> _commit() async {
    final title = _text.text.trim();
    if (title.isEmpty || title == _committed) {
      _text.text = _committed;
      return;
    }
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    try {
      await app.queue.run(() => app.backend.renamePage(widget.page.id, title));
      final old = _committed;
      _committed = title;
      app.renamed(widget.page.id, title);
      // The core rewrote links in other blocks, possibly on this page: reload them. The
      // route must not keep opening the old title (that would create a duplicate page).
      app.replaceWithId(widget.page.id, old);
    } catch (e) {
      _text.text = _committed;
      messenger.showSnackBar(SnackBar(content: Text('Rename failed: $e')));
    }
  }

  @override
  Widget build(BuildContext context) => TextField(
    controller: _text,
    focusNode: _focus,
    style: Theme.of(context).textTheme.headlineLarge,
    decoration: const InputDecoration.collapsed(hintText: 'Untitled'),
    maxLines: null,
    onSubmitted: (_) => _focus.unfocus(),
    textInputAction: TextInputAction.done,
  );
}

class _Aliases extends StatefulWidget {
  const _Aliases({required this.page, required this.initial});
  final PageDto page;
  final List<String> initial;

  @override
  State<_Aliases> createState() => _AliasesState();
}

class _AliasesState extends State<_Aliases> {
  late final List<String> _aliases = List.of(widget.initial);
  bool _adding = false;
  final _text = TextEditingController();

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _add() async {
    final alias = _text.text.trim();
    setState(() => _adding = false);
    _text.clear();
    if (alias.isEmpty || _aliases.contains(alias)) return;
    final app = AppScope.read(context);
    await app.queue.run(() => app.backend.addAlias(widget.page.id, alias));
    if (mounted) setState(() => _aliases.add(alias));
  }

  Future<void> _remove(String alias) async {
    final app = AppScope.read(context);
    await app.queue.run(() => app.backend.removeAlias(alias));
    if (mounted) setState(() => _aliases.remove(alias));
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Wrap(
      spacing: 6,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (final a in _aliases)
          Container(
            padding: const EdgeInsets.only(left: 8, right: 2),
            decoration: BoxDecoration(
              color: colors.surface,
              borderRadius: BorderRadius.circular(20),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(a, style: TextStyle(fontSize: 12, color: colors.muted)),
                InkResponse(
                  radius: 10,
                  onTap: () => _remove(a),
                  child: Padding(
                    padding: const EdgeInsets.all(3),
                    child: Icon(
                      Icons.close_rounded,
                      size: 12,
                      color: colors.faint,
                    ),
                  ),
                ),
              ],
            ),
          ),
        if (_adding)
          SizedBox(
            width: 140,
            child: TextField(
              controller: _text,
              autofocus: true,
              style: TextStyle(fontSize: 12, color: colors.ink),
              decoration: const InputDecoration.collapsed(hintText: 'alias'),
              onSubmitted: (_) => _add(),
              onTapOutside: (_) => _add(),
            ),
          )
        else
          TextLink(
            _aliases.isEmpty ? 'Add alias' : 'Alias',
            icon: Icons.add_rounded,
            onTap: () => setState(() => _adding = true),
            style: const TextStyle(fontSize: 12),
          ),
      ],
    );
  }
}

class _LinkedReferences extends StatelessWidget {
  const _LinkedReferences({required this.groups});
  final List<BacklinkGroup>? groups;

  @override
  Widget build(BuildContext context) {
    final g = groups;
    if (g == null || g.isEmpty) return const SizedBox(height: 40);
    final app = AppScope.read(context);
    final colors = context.colors;
    final count = g.fold<int>(0, (n, x) => n + x.blocks.length);
    return Padding(
      padding: const EdgeInsets.only(top: 56),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SectionLabel(
            'Linked references',
            trailing: Text(
              '$count',
              style: TextStyle(fontSize: 11, color: colors.faint),
            ),
          ),
          const SizedBox(height: 12),
          for (final group in g)
            Container(
              margin: const EdgeInsets.only(bottom: 12),
              padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
              decoration: BoxDecoration(
                color: colors.surface,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  TextLink(
                    group.isJournal
                        ? longDate(group.pageTitle)
                        : group.pageTitle,
                    onTap: () => app.openId(group.pageId),
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: colors.ink,
                    ),
                  ),
                  const SizedBox(height: 4),
                  for (final b in group.blocks)
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        const Dot(),
                        const SizedBox(width: 4),
                        Expanded(
                          child: StaticBlock(
                            content: b.content,
                            onTap: () => app.openId(group.pageId),
                          ),
                        ),
                      ],
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// Notes related by spreading activation over links, synapses and time.
class RelatedPanel extends StatelessWidget {
  const RelatedPanel({super.key, required this.related, this.compact = false});
  final List<RelatedDto>? related;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final r = related;
    final colors = context.colors;
    final app = AppScope.read(context);
    final top = (r == null || r.isEmpty) ? 1.0 : r.first.score;
    return Padding(
      padding: EdgeInsets.only(top: compact ? 0 : 40),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const SectionLabel('Related'),
          const SizedBox(height: 4),
          Text(
            'Strengthened by links, visits and shared days',
            style: TextStyle(fontSize: 12, color: colors.faint),
          ),
          const SizedBox(height: 14),
          if (r == null)
            const SizedBox.shrink()
          else if (r.isEmpty)
            Text(
              'Nothing yet. Link this note to others.',
              style: TextStyle(fontSize: 13, color: colors.muted),
            )
          else
            for (final x in r)
              Padding(
                padding: const EdgeInsets.only(bottom: 10),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    TextLink(
                      x.page.isJournal ? longDate(x.page.title) : x.page.title,
                      icon: x.page.isJournal
                          ? Icons.calendar_today_outlined
                          : Icons.description_outlined,
                      onTap: () => app.openId(x.page.id),
                      style: TextStyle(fontSize: 13.5, color: colors.ink),
                    ),
                    const SizedBox(height: 4),
                    ClipRRect(
                      borderRadius: BorderRadius.circular(2),
                      child: LinearProgressIndicator(
                        value: (x.score / top).clamp(0.05, 1.0),
                        minHeight: 3,
                        backgroundColor: colors.line,
                        color: colors.accent.withAlpha(0x99),
                      ),
                    ),
                  ],
                ),
              ),
        ],
      ),
    );
  }
}

/// Namespace ancestors of the page, `A / B` above the title. Segments open their page
/// (virtual ones are created on open, like links).
class Breadcrumb extends StatelessWidget {
  const Breadcrumb({super.key, required this.path});
  final List<NamespaceEntry> path;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final app = AppScope.read(context);
    return Wrap(
      spacing: 6,
      runSpacing: 2,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Icon(Icons.account_tree_outlined, size: 14, color: colors.faint),
        for (var i = 0; i < path.length; i++) ...[
          if (i > 0)
            Text('/', style: TextStyle(fontSize: 12, color: colors.faint)),
          TextLink(
            path[i].title.split('/').last,
            onTap: () => path[i].id != null
                ? app.openId(path[i].id!)
                : app.openTitle(path[i].title),
            style: const TextStyle(fontSize: 12.5),
          ),
        ],
      ],
    );
  }
}

/// "In: hub, hub" chips for the hubs containing the page; each has a remove button.
class InHubs extends StatelessWidget {
  const InHubs({super.key, required this.pageId, required this.hubs});
  final String pageId;
  final List<PageBrief> hubs;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final app = AppScope.read(context);
    return Wrap(
      spacing: 8,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Text('In', style: TextStyle(fontSize: 12, color: colors.faint)),
        for (final h in hubs)
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextLink(
                h.title,
                icon: Icons.folder_outlined,
                onTap: () => app.openId(h.id),
                style: const TextStyle(fontSize: 12),
              ),
              InkResponse(
                key: ValueKey('leave-${h.id}'),
                radius: 10,
                onTap: () async {
                  final messenger = ScaffoldMessenger.of(context);
                  try {
                    await app.queue.run(
                      () => app.backend.removeFromHub(pageId, h.id),
                      report: false,
                    );
                    app.reload();
                  } catch (e) {
                    messenger.showSnackBar(
                      SnackBar(content: Text(errorText(e))),
                    );
                  }
                },
                child: Padding(
                  padding: const EdgeInsets.all(3),
                  child: Icon(
                    Icons.close_rounded,
                    size: 12,
                    color: colors.faint,
                  ),
                ),
              ),
            ],
          ),
      ],
    );
  }
}

/// The page's "⋯" menu: hub conversion, hub membership and deletion.
class PageMenu extends StatelessWidget {
  const PageMenu({super.key, required this.page, required this.structure});
  final PageDto page;
  final PageStructure? structure;

  Future<void> _addToHub(BuildContext context) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    final taken = {page.id, ...?structure?.inHubs.map((h) => h.id)};
    final r = await showPagePicker(
      context,
      heading: 'Add to hub',
      canCreateHub: true,
      search: (q) async {
        final all = await app.backend.hubs();
        return [
          for (final h in all.hubs)
            if (!taken.contains(h.id) &&
                h.title.toLowerCase().contains(q.toLowerCase()))
              PickItem(id: h.id, title: h.title, isHub: true),
        ];
      },
    );
    if (r == null) return;
    try {
      await app.queue.run(() async {
        final id =
            r.item?.id ?? (await app.backend.createHub(r.newTitle!, '')).id;
        await app.backend.addToHub(page.id, id);
      }, report: false);
      app.reload();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  Future<void> _toHub(BuildContext context) async {
    final app = AppScope.read(context);
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => ConvertToHubDialog(page: page),
    );
    if (ok == true) {
      app.recent.removeWhere((p) => p.id == page.id);
      app.replaceWithId(page.id, page.title);
    }
  }

  Future<void> _toNote(BuildContext context) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    try {
      await app.queue.run(
        () => app.backend.convertToNote(page.id),
        report: false,
      );
      app.reload();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
    }
  }

  Future<void> _delete(BuildContext context) async {
    final app = AppScope.read(context);
    final messenger = ScaffoldMessenger.of(context);
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => DeletePagesDialog(pages: [page]),
    );
    if (ok != true) return;
    try {
      await app.queue.run(() => app.backend.deletePage(page.id));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(errorText(e))));
      app.reload();
      return;
    }
    // Tabs showing the page step back (or fall back to Journals); recents drop it.
    app.pagesDeleted([page.id]);
    messenger.showSnackBar(SnackBar(content: Text('Deleted "${page.title}"')));
  }

  @override
  Widget build(BuildContext context) => PopupMenuButton<String>(
    key: const ValueKey('page-menu'),
    icon: Icon(Icons.more_horiz_rounded, color: context.colors.faint),
    tooltip: 'More',
    onSelected: (v) => switch (v) {
      'hub' => _toHub(context),
      'note' => _toNote(context),
      'delete' => _delete(context),
      _ => _addToHub(context),
    },
    itemBuilder: (_) => [
      if (!page.isHub)
        const PopupMenuItem(value: 'hub', child: Text('Convert to hub…')),
      if (page.isHub)
        const PopupMenuItem(value: 'note', child: Text('Convert to note')),
      const PopupMenuItem(value: 'add', child: Text('Add to hub…')),
      const PopupMenuDivider(),
      PopupMenuItem(
        key: const ValueKey('page-delete'),
        value: 'delete',
        child: Text(
          'Delete page…',
          style: TextStyle(color: Theme.of(context).colorScheme.error),
        ),
      ),
    ],
  );
}

/// "Namespace" listing (title-kin children) under the header.
class _NamespaceSection extends StatelessWidget {
  const _NamespaceSection({required this.structure});
  final PageStructure structure;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final app = AppScope.read(context);
    final st = structure;
    if (st.nsChildren.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(bottom: 18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SectionLabel(
            'Namespace',
            trailing: Text(
              '${st.nsChildren.length}',
              style: TextStyle(fontSize: 11, color: colors.faint),
            ),
          ),
          const SizedBox(height: 8),
          Wrap(
            children: [
              for (final c in st.nsChildren)
                SizedBox(
                  width: 230,
                  child: InkWell(
                    borderRadius: BorderRadius.circular(8),
                    onTap: () => c.id != null
                        ? app.openId(c.id!)
                        : app.openTitle(c.title),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 8,
                        vertical: 6,
                      ),
                      child: Row(
                        children: [
                          Icon(
                            Icons.account_tree_outlined,
                            size: 16,
                            color: colors.accent.withAlpha(0xCC),
                          ),
                          const SizedBox(width: 8),
                          Expanded(
                            child: Text(
                              c.title.split('/').last,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 13.5,
                                color: colors.ink,
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
        ],
      ),
    );
  }
}
