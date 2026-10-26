import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../state/app_state.dart';
import '../state/keymap.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';

/// Theme picker and key bindings.
class SettingsView extends StatelessWidget {
  const SettingsView({super.key});

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    return LayoutBuilder(
      builder: (context, box) => ListView(
        padding: EdgeInsets.fromLTRB(
          sidePadding(box.maxWidth),
          36,
          sidePadding(box.maxWidth),
          80,
        ),
        children: [
          Text('Settings', style: Theme.of(context).textTheme.headlineLarge),
          const SizedBox(height: 28),
          _Heading('Theme'),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: [
              _ThemeCard(
                id: autoThemeId,
                label: 'System',
                swatch: [
                  SynapseColors.light.paper,
                  SynapseColors.dark.paper,
                  SynapseColors.light.accent,
                  SynapseColors.dark.accent,
                ],
                selected: app.themeId == autoThemeId,
              ),
              for (final t in appThemes)
                _ThemeCard(
                  id: t.id,
                  label: t.label,
                  swatch: [
                    t.colors.paper,
                    t.colors.surface,
                    t.colors.accent,
                    t.colors.ref,
                  ],
                  selected: app.themeId == t.id,
                ),
            ],
          ),
          const SizedBox(height: 36),
          Row(
            children: [
              Expanded(child: _Heading('Keyboard shortcuts')),
              TextLink(
                'Reset all',
                icon: Icons.restart_alt_rounded,
                onTap: () => app.resetKey(null),
              ),
            ],
          ),
          Text(
            'Click a shortcut, then press the new combination. Esc cancels.',
            style: TextStyle(fontSize: 13, color: colors.muted),
          ),
          const SizedBox(height: 10),
          for (final editor in [false, true]) ...[
            Padding(
              padding: const EdgeInsets.only(top: 14, bottom: 4),
              child: Text(
                editor ? 'EDITOR' : 'APP',
                style: Theme.of(context).textTheme.labelSmall,
              ),
            ),
            for (final a in AppAction.values.where((a) => a.editor == editor))
              _BindingRow(action: a),
          ],
          const SizedBox(height: 28),
          Text(
            'Type "/" in a block for commands (link, bold, headings, todo, ...). '
            'Prefix a character with a backslash to show it literally: \\*not italic\\*.',
            style: TextStyle(fontSize: 13, color: colors.muted),
          ),
        ],
      ),
    );
  }
}

class _Heading extends StatelessWidget {
  const _Heading(this.text);
  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 12),
    child: Text(text, style: Theme.of(context).textTheme.headlineSmall),
  );
}

class _ThemeCard extends StatelessWidget {
  const _ThemeCard({
    required this.id,
    required this.label,
    required this.swatch,
    required this.selected,
  });

  final String id;
  final String label;
  final List<Color> swatch;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return InkWell(
      key: ValueKey('theme-$id'),
      borderRadius: BorderRadius.circular(10),
      onTap: () => AppScope.read(context).setTheme(id),
      child: Container(
        width: 132,
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(10),
          border: Border.all(
            color: selected ? colors.accent : colors.line,
            width: selected ? 2 : 1,
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(6),
              child: Row(
                children: [
                  for (final c in swatch)
                    Expanded(child: Container(height: 28, color: c)),
                ],
              ),
            ),
            const SizedBox(height: 8),
            Text(
              label,
              style: TextStyle(
                fontSize: 13,
                fontWeight: selected ? FontWeight.w600 : FontWeight.w400,
                color: colors.ink,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _BindingRow extends StatefulWidget {
  const _BindingRow({required this.action});
  final AppAction action;

  @override
  State<_BindingRow> createState() => _BindingRowState();
}

class _BindingRowState extends State<_BindingRow> {
  final _focus = FocusNode();
  bool _listening = false;
  String? _conflict;

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode _, KeyEvent e) {
    if (!_listening || e is! KeyDownEvent) return KeyEventResult.ignored;
    if (e.logicalKey == LogicalKeyboardKey.escape) {
      setState(() => _listening = false);
      return KeyEventResult.handled;
    }
    final kb = HardwareKeyboard.instance;
    final combo = Combo(
      e.logicalKey,
      ctrl: kb.isControlPressed || kb.isMetaPressed,
      shift: kb.isShiftPressed,
      alt: kb.isAltPressed,
    );
    if (combo.isModifierOnly) return KeyEventResult.handled;
    final owner = AppScope.read(context).bindKey(widget.action, combo);
    setState(() {
      _listening = false;
      _conflict = owner == null ? null : 'Already used by "${owner.label}"';
    });
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.of(context);
    final colors = context.colors;
    final a = widget.action;
    return Focus(
      focusNode: _focus,
      onKeyEvent: _onKey,
      onFocusChange: (f) {
        if (!f && _listening) setState(() => _listening = false);
      },
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 3),
        child: Row(
          children: [
            Expanded(
              child: Text(
                a.label,
                style: TextStyle(fontSize: 14, color: colors.ink),
              ),
            ),
            if (_conflict != null)
              Padding(
                padding: const EdgeInsets.only(right: 12),
                child: Text(
                  _conflict!,
                  style: TextStyle(fontSize: 12, color: colors.ref),
                ),
              ),
            OutlinedButton(
              key: ValueKey('bind-${a.name}'),
              style: OutlinedButton.styleFrom(
                minimumSize: const Size(120, 32),
                side: BorderSide(
                  color: _listening ? colors.accent : colors.line,
                ),
                foregroundColor: colors.ink,
              ),
              onPressed: () {
                setState(() {
                  _listening = true;
                  _conflict = null;
                });
                _focus.requestFocus();
              },
              child: Text(
                _listening ? 'Press keys…' : app.keymap.of(a).label,
                style: TextStyle(
                  fontSize: 12.5,
                  color: app.keymap.isCustom(a) ? colors.accent : colors.ink,
                ),
              ),
            ),
            IconButton(
              tooltip: 'Reset',
              iconSize: 16,
              visualDensity: VisualDensity.compact,
              color: colors.faint,
              onPressed: app.keymap.isCustom(a) ? () => app.resetKey(a) : null,
              icon: const Icon(Icons.restart_alt_rounded),
            ),
          ],
        ),
      ),
    );
  }
}
