import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart'
    show kBackMouseButton, kForwardMouseButton;
import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../state/keymap.dart';
import '../theme/theme.dart';
import '../widgets/dates.dart';
import 'all_pages_view.dart';
import 'journals_view.dart';
import 'module_view.dart';
import 'page_screen.dart';
import 'hubs_tree.dart';
import 'quick_switcher.dart';
import 'graph_view.dart';
import 'settings_view.dart';
import 'tab_strip.dart';
import 'tag_view.dart';
import 'tags_types_view.dart';
import '../widgets/type_icon.dart';

/// App frame: sidebar (navigation, hubs, recents), top bar (history, search, theme,
/// settings) and the current view.
class Shell extends StatefulWidget {
  const Shell({super.key});

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  bool _sidebar = true;
  final _scaffold = GlobalKey<ScaffoldState>();

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    final route = app.route;
    final error = app.writeError;
    if (error != null) {
      app.writeError = null;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!context.mounted) return;
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Could not save a change; reloaded. ($error)'),
          ),
        );
      });
    }
    final actions = <AppAction, VoidCallback>{
      AppAction.quickSwitcher: () => showQuickSwitcher(context),
      AppAction.journals: () => app.go(const JournalsRoute()),
      AppAction.graph: () => app.go(const GraphRoute(GraphMode.network)),
      AppAction.timeline: () => app.go(const TimelineRoute()),
      AppAction.sidebar: () => setState(() => _sidebar = !_sidebar),
      AppAction.pageSidebar: app.togglePageSidebar,
      AppAction.settings: () => app.go(const SettingsRoute()),
      AppAction.back: app.back,
      AppAction.forward: app.forward,
      AppAction.newTab: app.newTab,
      AppAction.closeTab: app.closeTab,
      AppAction.nextTab: app.nextTab,
      AppAction.previousTab: app.previousTab,
    };
    return CallbackShortcuts(
      bindings: {
        for (final e in actions.entries)
          ...Map.fromEntries(activators(app.keymap.of(e.key), e.value)),
      },
      // The mouse's back/forward side buttons work like the arrows, as in a browser.
      child: Listener(
        onPointerDown: (e) {
          if (e.buttons & kBackMouseButton != 0) app.back();
          if (e.buttons & kForwardMouseButton != 0) app.forward();
        },
        child: Focus(
          autofocus: true,
          child: LayoutBuilder(
            builder: (context, box) {
              final narrow = box.maxWidth < 760;
              final sidebar = _Sidebar(
                onNavigate: () {
                  if (narrow) _scaffold.currentState?.closeDrawer();
                },
              );
              return Scaffold(
                key: _scaffold,
                drawer: narrow
                    ? Drawer(backgroundColor: colors.surface, child: sidebar)
                    : null,
                body: Row(
                  children: [
                    if (!narrow)
                      AnimatedContainer(
                        duration: const Duration(milliseconds: 180),
                        curve: Curves.easeOutCubic,
                        width: _sidebar ? 248 : 0,
                        child: ClipRect(
                          child: OverflowBox(
                            alignment: Alignment.topLeft,
                            minWidth: 248,
                            maxWidth: 248,
                            child: sidebar,
                          ),
                        ),
                      ),
                    Expanded(
                      child: Column(
                        children: [
                          _TopBar(
                            narrow: narrow,
                            onMenu: () => narrow
                                ? _scaffold.currentState?.openDrawer()
                                : setState(() => _sidebar = !_sidebar),
                          ),
                          if (!narrow) const TabStrip(),
                          Expanded(
                            child: KeyedSubtree(
                              key: ValueKey((
                                app.tab.uid,
                                route,
                                app.generation,
                              )),
                              child: switch (route) {
                                JournalsRoute() => const JournalsView(),
                                TimelineRoute() => const GraphView(
                                  mode: GraphMode.timeline,
                                ),
                                GraphRoute() => GraphView(
                                  mode: route.mode,
                                  focusId: route.focusId,
                                ),
                                AllPagesRoute() => const AllPagesView(),
                                SettingsRoute() => const SettingsView(),
                                TagsTypesRoute() => const TagsTypesView(),
                                ModuleRoute() => ModuleView(
                                  moduleId: route.moduleId,
                                  viewId: route.viewId,
                                ),
                                TagRoute() => TagView(name: route.name),
                                PageTarget() => PageScreen(target: route),
                              },
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              );
            },
          ),
        ),
      ),
    );
  }
}

class _TopBar extends StatelessWidget {
  const _TopBar({required this.onMenu, required this.narrow});
  final VoidCallback onMenu;
  final bool narrow;

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    final brightness = Theme.of(context).brightness;
    Widget icon(IconData i, String tip, VoidCallback? f) => IconButton(
      tooltip: tip,
      onPressed: f,
      iconSize: 18,
      color: colors.muted,
      disabledColor: colors.line,
      icon: Icon(i),
    );
    return Container(
      height: 48,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      child: Row(
        children: [
          icon(
            Icons.menu_rounded,
            'Toggle sidebar (${app.keymap.of(AppAction.sidebar).label})',
            onMenu,
          ),
          icon(
            Icons.arrow_back_rounded,
            'Back (${app.keymap.of(AppAction.back).label})',
            app.canGoBack ? app.back : null,
          ),
          icon(
            Icons.arrow_forward_rounded,
            'Forward (${app.keymap.of(AppAction.forward).label})',
            app.canGoForward ? app.forward : null,
          ),
          const Spacer(),
          if (narrow) const TabCountButton(),
          // Phones: an icon instead of the pill keeps the bar from overflowing.
          if (MediaQuery.sizeOf(context).width < 600)
            icon(
              Icons.search_rounded,
              'Search (${app.keymap.of(AppAction.quickSwitcher).label})',
              () => showQuickSwitcher(context),
            )
          else
            _SearchPill(onTap: () => showQuickSwitcher(context)),
          const SizedBox(width: 4),
          icon(
            brightness == Brightness.dark
                ? Icons.light_mode_outlined
                : Icons.dark_mode_outlined,
            'Toggle theme',
            () => app.toggleTheme(brightness),
          ),
          icon(
            app.route is SettingsRoute
                ? Icons.settings_rounded
                : Icons.settings_outlined,
            'Settings (${app.keymap.of(AppAction.settings).label})',
            () => app.go(const SettingsRoute()),
          ),
        ],
      ),
    );
  }
}

class _SearchPill extends StatelessWidget {
  const _SearchPill({required this.onTap});
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(8),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        decoration: BoxDecoration(
          border: Border.all(color: colors.line),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.search_rounded, size: 16, color: colors.muted),
            const SizedBox(width: 8),
            Text('Search', style: TextStyle(fontSize: 13, color: colors.muted)),
            const SizedBox(width: 18),
            Text(
              AppScope.of(context).keymap.of(AppAction.quickSwitcher).label,
              style: TextStyle(fontSize: 11, color: colors.faint),
            ),
          ],
        ),
      ),
    );
  }
}

class _Sidebar extends StatelessWidget {
  const _Sidebar({required this.onNavigate});
  final VoidCallback onNavigate;

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    final route = app.route;
    // Ctrl/Cmd-click opens in a new tab, middle-click always does.
    void go(AppRoute r) {
      app.navigate(r);
      onNavigate();
    }

    void newTab(AppRoute r) {
      app.openInNewTab(r);
      onNavigate();
    }

    return Container(
      color: colors.surface,
      child: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 18, 20, 22),
              child: Row(
                children: [
                  CustomPaint(
                    size: const Size(20, 20),
                    painter: _Logo(colors.accent),
                  ),
                  const SizedBox(width: 10),
                  Text(
                    'Synapse',
                    style: TextStyle(
                      fontFamily: titleFont,
                      fontSize: 18,
                      fontWeight: FontWeight.w600,
                      color: colors.ink,
                    ),
                  ),
                ],
              ),
            ),
            _NavItem(
              icon: Icons.calendar_today_outlined,
              label: 'Journals',
              selected: route is JournalsRoute,
              onTap: () => go(const JournalsRoute()),
              onMiddle: () => newTab(const JournalsRoute()),
            ),
            _NavItem(
              icon: Icons.hub_outlined,
              label: 'Graph',
              selected: route is GraphRoute,
              onTap: () => go(const GraphRoute(GraphMode.network)),
              onMiddle: () => newTab(const GraphRoute(GraphMode.network)),
            ),
            _NavItem(
              icon: Icons.description_outlined,
              label: 'All pages',
              selected: route is AllPagesRoute,
              onTap: () => go(const AllPagesRoute()),
              onMiddle: () => newTab(const AllPagesRoute()),
            ),
            _NavItem(
              icon: Icons.style_outlined,
              label: 'Tags & types',
              selected: route is TagsTypesRoute,
              onTap: () => go(const TagsTypesRoute()),
              onMiddle: () => newTab(const TagsTypesRoute()),
            ),
            // Views of enabled modules.
            ListenableBuilder(
              listenable: app.modules,
              builder: (context, _) => Column(
                children: [
                  for (final m in app.modules.enabled)
                    for (final v in m.views)
                      _NavItem(
                        icon: v.icon == null ? Icons.extension_outlined : null,
                        leading: v.icon == null
                            ? null
                            : Padding(
                                padding: const EdgeInsets.only(right: 10),
                                child: Text(
                                  v.icon!,
                                  style: const TextStyle(fontSize: 14),
                                ),
                              ),
                        label: v.title,
                        selected:
                            route is ModuleRoute &&
                            route.moduleId == m.id &&
                            route.viewId == v.id,
                        onTap: () =>
                            go(ModuleRoute(m.id, v.id, title: v.title)),
                        onMiddle: () =>
                            newTab(ModuleRoute(m.id, v.id, title: v.title)),
                      ),
                ],
              ),
            ),
            const SizedBox(height: 22),
            Expanded(
              child: ListView(
                padding: EdgeInsets.zero,
                children: [
                  HubsSection(
                    selectedId: route is PageTarget ? route.id : null,
                    onOpen: (id) => go(PageTarget(id: id)),
                  ),
                  Padding(
                    padding: const EdgeInsets.fromLTRB(20, 0, 20, 6),
                    child: Text(
                      'RECENT',
                      style: Theme.of(context).textTheme.labelSmall,
                    ),
                  ),
                  for (final p in app.recent)
                    _NavItem(
                      label: p.isJournal ? longDate(p.title) : p.title,
                      leading: TypeIcon(p.pageType),
                      selected: route is PageTarget && route.id == p.id,
                      dense: true,
                      onTap: () => go(PageTarget(id: p.id)),
                      onMiddle: () => newTab(PageTarget(id: p.id)),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _NavItem extends StatefulWidget {
  const _NavItem({
    this.icon,
    this.leading,
    required this.label,
    required this.selected,
    required this.onTap,
    this.onMiddle,
    this.dense = false,
  });
  final IconData? icon;

  /// Shown before the label (a page's type emoji).
  final Widget? leading;
  final String label;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback? onMiddle;
  final bool dense;

  @override
  State<_NavItem> createState() => _NavItemState();
}

class _NavItemState extends State<_NavItem> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final active = widget.selected;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        onTertiaryTapUp: widget.onMiddle == null
            ? null
            : (_) => widget.onMiddle!(),
        child: Container(
          margin: const EdgeInsets.symmetric(horizontal: 10, vertical: 1),
          padding: EdgeInsets.symmetric(
            horizontal: 10,
            vertical: widget.dense ? 6 : 8,
          ),
          decoration: BoxDecoration(
            color: active
                ? colors.accentSoft
                : _hover
                ? colors.line.withAlpha(0x66)
                : null,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              if (widget.icon != null) ...[
                Icon(
                  widget.icon,
                  size: 16,
                  color: active ? colors.accent : colors.muted,
                ),
                const SizedBox(width: 10),
              ],
              ?widget.leading,
              Expanded(
                child: Text(
                  widget.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: widget.dense ? 13 : 14,
                    fontWeight: active ? FontWeight.w600 : FontWeight.w500,
                    color: active
                        ? colors.ink
                        : (widget.dense ? colors.muted : colors.ink),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Two linked nodes: the mark.
class _Logo extends CustomPainter {
  _Logo(this.color);
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()..color = color;
    final a = Offset(size.width * 0.25, size.height * 0.68);
    final b = Offset(size.width * 0.75, size.height * 0.32);
    canvas.drawLine(
      a,
      b,
      Paint()
        ..color = color.withAlpha(0x99)
        ..strokeWidth = 2,
    );
    canvas.drawCircle(a, size.width * 0.18, p);
    canvas.drawCircle(b, size.width * 0.14, p);
  }

  @override
  bool shouldRepaint(_Logo old) => old.color != color;
}

/// The shortcut activators for `combo` (Ctrl also answers to Cmd on macOS).
List<MapEntry<ShortcutActivator, VoidCallback>> activators(
  Combo combo,
  VoidCallback run,
) => [
  MapEntry(
    SingleActivator(
      combo.key,
      control: combo.ctrl,
      shift: combo.shift,
      alt: combo.alt,
    ),
    run,
  ),
  if (combo.ctrl && defaultTargetPlatform == TargetPlatform.macOS)
    MapEntry(
      SingleActivator(
        combo.key,
        meta: true,
        shift: combo.shift,
        alt: combo.alt,
      ),
      run,
    ),
];
