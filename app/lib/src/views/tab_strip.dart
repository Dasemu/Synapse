import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../state/keymap.dart';
import '../theme/theme.dart';

const _tabWidth = 168.0;
const _stripHeight = 34.0;

/// Browser-like tab strip: one compact item per tab, close button on hover or when active,
/// a "+" button, horizontal scrolling on overflow and drag to reorder.
class TabStrip extends StatelessWidget {
  const TabStrip({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    final tabs = app.tabs;
    return Container(
      key: const ValueKey('tab-strip'),
      height: _stripHeight,
      decoration: BoxDecoration(
        color: colors.surface,
        border: Border(
          top: BorderSide(color: colors.line),
          bottom: BorderSide(color: colors.line),
        ),
      ),
      child: Row(
        children: [
          Expanded(
            child: ReorderableListView.builder(
              scrollDirection: Axis.horizontal,
              buildDefaultDragHandles: false,
              padding: const EdgeInsets.only(left: 6, top: 4),
              itemCount: tabs.length,
              onReorderItem: app.moveTab,
              proxyDecorator: (child, _, _) =>
                  Material(color: Colors.transparent, child: child),
              itemBuilder: (context, i) => ReorderableDragStartListener(
                key: ValueKey('tab-${tabs[i].uid}'),
                index: i,
                child: _TabItem(
                  tab: tabs[i],
                  index: i,
                  active: i == app.activeTab,
                ),
              ),
            ),
          ),
          IconButton(
            key: const ValueKey('tab-new'),
            tooltip: 'New tab (${app.keymap.of(AppAction.newTab).label})',
            iconSize: 16,
            visualDensity: VisualDensity.compact,
            color: colors.muted,
            icon: const Icon(Icons.add_rounded),
            onPressed: app.newTab,
          ),
        ],
      ),
    );
  }
}

class _TabItem extends StatefulWidget {
  const _TabItem({
    required this.tab,
    required this.index,
    required this.active,
  });
  final AppTab tab;
  final int index;
  final bool active;

  @override
  State<_TabItem> createState() => _TabItemState();
}

class _TabItemState extends State<_TabItem> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final colors = context.colors;
    final active = widget.active;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      cursor: SystemMouseCursors.click,
      child: Listener(
        onPointerDown: (e) {
          if (e.buttons == kMiddleMouseButton) app.closeTab(widget.index);
        },
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => app.activateTab(widget.index),
          child: Container(
            width: _tabWidth,
            margin: const EdgeInsets.only(right: 2),
            padding: const EdgeInsets.only(left: 10, right: 4),
            decoration: BoxDecoration(
              color: active
                  ? colors.paper
                  : (_hover ? colors.line.withAlpha(0x66) : null),
              border: Border.all(
                color: active ? colors.line : Colors.transparent,
              ),
              borderRadius: const BorderRadius.vertical(
                top: Radius.circular(8),
              ),
            ),
            child: Row(
              children: [
                Icon(
                  widget.tab.icon,
                  size: 13,
                  color: active ? colors.accent : colors.faint,
                ),
                const SizedBox(width: 7),
                Expanded(
                  child: Text(
                    widget.tab.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: active ? FontWeight.w600 : FontWeight.w500,
                      color: active ? colors.ink : colors.muted,
                    ),
                  ),
                ),
                SizedBox(
                  width: 22,
                  child: active || _hover
                      ? InkResponse(
                          key: ValueKey('tab-close-${widget.index}'),
                          radius: 11,
                          onTap: () => app.closeTab(widget.index),
                          child: Icon(
                            Icons.close_rounded,
                            size: 14,
                            color: colors.muted,
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

/// Narrow layouts: a button with the tab count that opens the tab list.
class TabCountButton extends StatelessWidget {
  const TabCountButton({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    return IconButton(
      key: const ValueKey('tab-count'),
      tooltip: 'Tabs',
      onPressed: () => showTabsSheet(context),
      icon: Container(
        width: 20,
        height: 20,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          border: Border.all(color: colors.muted, width: 1.5),
          borderRadius: BorderRadius.circular(5),
        ),
        child: Text(
          '${app.tabs.length}',
          style: TextStyle(
            fontSize: 11,
            fontWeight: FontWeight.w700,
            color: colors.muted,
          ),
        ),
      ),
    );
  }
}

Future<void> showTabsSheet(BuildContext context) {
  final app = AppScope.read(context);
  return showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    isScrollControlled: true,
    builder: (sheet) => ListenableBuilder(
      listenable: app,
      builder: (context, _) {
        final colors = context.colors;
        final tabs = app.tabs;
        return SafeArea(
          child: ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.sizeOf(context).height * 0.7,
            ),
            child: ListView(
              shrinkWrap: true,
              children: [
                for (var i = 0; i < tabs.length; i++)
                  ListTile(
                    key: ValueKey('sheet-tab-$i'),
                    dense: true,
                    selected: i == app.activeTab,
                    selectedColor: colors.accent,
                    leading: Icon(tabs[i].icon, size: 18),
                    title: Text(
                      tabs[i].title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    trailing: IconButton(
                      key: ValueKey('sheet-tab-close-$i'),
                      tooltip: 'Close tab',
                      icon: const Icon(Icons.close_rounded, size: 18),
                      onPressed: () => app.closeTab(i),
                    ),
                    onTap: () {
                      app.activateTab(i);
                      Navigator.pop(sheet);
                    },
                  ),
                ListTile(
                  key: const ValueKey('sheet-tab-new'),
                  dense: true,
                  leading: const Icon(Icons.add_rounded, size: 18),
                  title: const Text('New tab'),
                  onTap: () {
                    app.newTab();
                    Navigator.pop(sheet);
                  },
                ),
              ],
            ),
          ),
        );
      },
    ),
  );
}
