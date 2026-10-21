import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/dates.dart';
import 'all_pages_view.dart';
import 'journals_view.dart';
import 'page_screen.dart';
import 'quick_switcher.dart';
import 'timeline_view.dart';

/// App frame: sidebar (navigation, recents, theme), top bar and the current view.
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
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyK, control: true): () =>
            showQuickSwitcher(context),
        const SingleActivator(LogicalKeyboardKey.keyK, meta: true): () =>
            showQuickSwitcher(context),
        const SingleActivator(
          LogicalKeyboardKey.keyJ,
          control: true,
          shift: true,
        ): () =>
            app.go(const JournalsRoute()),
        const SingleActivator(
          LogicalKeyboardKey.keyT,
          control: true,
          shift: true,
        ): () =>
            app.go(const TimelineRoute()),
        const SingleActivator(
          LogicalKeyboardKey.backslash,
          control: true,
        ): () =>
            setState(() => _sidebar = !_sidebar),
        const SingleActivator(LogicalKeyboardKey.arrowLeft, alt: true):
            app.back,
        const SingleActivator(LogicalKeyboardKey.arrowRight, alt: true):
            app.forward,
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
                          onMenu: () => narrow
                              ? _scaffold.currentState?.openDrawer()
                              : setState(() => _sidebar = !_sidebar),
                        ),
                        Expanded(
                          child: KeyedSubtree(
                            key: ValueKey(route),
                            child: switch (route) {
                              JournalsRoute() => const JournalsView(),
                              TimelineRoute() => const TimelineView(),
                              AllPagesRoute() => const AllPagesView(),
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
    );
  }
}

class _TopBar extends StatelessWidget {
  const _TopBar({required this.onMenu});
  final VoidCallback onMenu;

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
          icon(Icons.menu_rounded, 'Toggle sidebar (Ctrl+\\)', onMenu),
          icon(
            Icons.arrow_back_rounded,
            'Back (Alt+←)',
            app.canGoBack ? app.back : null,
          ),
          icon(
            Icons.arrow_forward_rounded,
            'Forward (Alt+→)',
            app.canGoForward ? app.forward : null,
          ),
          const Spacer(),
          _SearchPill(onTap: () => showQuickSwitcher(context)),
          const SizedBox(width: 4),
          icon(
            brightness == Brightness.dark
                ? Icons.light_mode_outlined
                : Icons.dark_mode_outlined,
            'Toggle theme',
            () => app.toggleTheme(brightness),
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
            Text('Ctrl K', style: TextStyle(fontSize: 11, color: colors.faint)),
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
    void go(AppRoute r) {
      app.go(r);
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
            ),
            _NavItem(
              icon: Icons.timeline_rounded,
              label: 'Timeline',
              selected: route is TimelineRoute,
              onTap: () => go(const TimelineRoute()),
            ),
            _NavItem(
              icon: Icons.description_outlined,
              label: 'All pages',
              selected: route is AllPagesRoute,
              onTap: () => go(const AllPagesRoute()),
            ),
            const SizedBox(height: 22),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20),
              child: Text(
                'RECENT',
                style: Theme.of(context).textTheme.labelSmall,
              ),
            ),
            const SizedBox(height: 6),
            Expanded(
              child: ListView(
                padding: EdgeInsets.zero,
                children: [
                  for (final p in app.recent)
                    _NavItem(
                      label: p.isJournal ? longDate(p.title) : p.title,
                      selected: route is PageTarget && route.id == p.id,
                      dense: true,
                      onTap: () => go(PageTarget(id: p.id)),
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
    required this.label,
    required this.selected,
    required this.onTap,
    this.dense = false,
  });
  final IconData? icon;
  final String label;
  final bool selected;
  final VoidCallback onTap;
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
