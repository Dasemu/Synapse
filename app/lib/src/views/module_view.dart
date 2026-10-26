import 'dart:async';

import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';
import '../editor/block_row.dart' show openUrl;
import '../widgets/common.dart';
import '../widgets/module_ui.dart';

/// A view contributed by a module. The module returns a UI tree (JSON, see
/// `docs/modules.md`); buttons, submitted fields and `change` fields send an `event` with the
/// current text of every field, and the module answers with the next tree plus effects.
class ModuleView extends StatefulWidget {
  const ModuleView({super.key, required this.moduleId, required this.viewId});

  final String moduleId;
  final String viewId;

  @override
  State<ModuleView> createState() => _ModuleViewState();
}

class _ModuleViewState extends State<ModuleView> {
  Map<String, dynamic>? _tree;
  String? _error;
  bool _busy = false;
  int _viewVersion = -1;

  /// One controller per field id, kept across trees so typing and focus survive re-renders.
  final _fields = <String, TextEditingController>{};

  /// The value each field had in the last tree (a new value from the module replaces the
  /// text; an unchanged one keeps what the user typed).
  final _sent = <String, String>{};
  final _hidden = <String, bool>{};
  Timer? _debounce;

  @override
  void initState() {
    super.initState();
    _render();
  }

  @override
  void dispose() {
    _debounce?.cancel();
    for (final c in _fields.values) {
      c.dispose();
    }
    super.dispose();
  }

  Future<void> _render() async {
    final r = await AppScope.read(context).modules
        .call(widget.moduleId, {'op': 'render', 'view': widget.viewId});
    if (mounted) _apply(r);
  }

  Future<void> _event(String action, {String? arg}) async {
    if (_busy) return;
    _debounce?.cancel();
    setState(() => _busy = true);
    final r = await AppScope.read(context).modules.call(widget.moduleId, {
      'op': 'event',
      'view': widget.viewId,
      'action': action,
      'arg': ?arg,
      'values': {for (final e in _fields.entries) e.key: e.value.text},
    });
    if (!mounted) return;
    setState(() => _busy = false);
    _apply(r);
    await applyModuleEffects(context, widget.moduleId, r);
  }

  void _apply(Map<String, dynamic> r) {
    setState(() {
      final ui = r['ui'];
      if (ui is Map<String, dynamic>) {
        _tree = ui;
        _error = null;
        _syncFields(ui);
      } else if (r['error'] != null) {
        _error = '${r['error']}';
      }
    });
  }

  /// The `key` of the last tree: a different one is another screen.
  Object? _screen;

  /// Applies field values from the new tree and drops controllers of fields that are gone.
  /// On a new screen every field starts from the tree, so text typed on one screen (a
  /// password) never carries over to a field with the same id on another.
  void _syncFields(Map<String, dynamic> tree) {
    if (tree['key'] != _screen) {
      _screen = tree['key'];
      _sent.clear();
      _hidden.clear();
    }
    final seen = <String>{};
    void walk(Object? w) {
      if (w is List) {
        w.forEach(walk);
        return;
      }
      if (w is! Map) return;
      if (w['type'] == 'field' && w['id'] is String) {
        final id = w['id'] as String;
        final value = w['value'] is String ? w['value'] as String : '';
        seen.add(id);
        final c = _fields.putIfAbsent(id, TextEditingController.new);
        if (_sent[id] != value) {
          c.text = value;
          _sent[id] = value;
        }
      }
      for (final k in const ['children', 'left', 'right', 'items']) {
        walk(w[k]);
      }
    }

    walk(tree);
    for (final id in _fields.keys.toList()) {
      if (!seen.contains(id)) {
        _fields.remove(id)!.dispose();
        _sent.remove(id);
        _hidden.remove(id);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final modules = AppScope.read(context).modules;
    return ListenableBuilder(
      listenable: modules,
      builder: (context, _) {
        if (_viewVersion != modules.viewVersion) {
          final first = _viewVersion < 0;
          _viewVersion = modules.viewVersion;
          // A tick changed the module's state (auto-lock, a password hidden again).
          if (!first) scheduleMicrotask(() => mounted ? _render() : null);
        }
        final m = modules.byId(widget.moduleId);
        if (m == null || !m.enabled) {
          return const Center(
            child: Text('This module is not installed or is disabled.'),
          );
        }
        return LayoutBuilder(
          builder: (context, box) {
            final pad = sidePadding(box.maxWidth);
            return Stack(
              children: [
                SingleChildScrollView(
                  padding: EdgeInsets.fromLTRB(pad, 28, pad, 60),
                  child: _error != null
                      ? Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              _error!,
                              style: TextStyle(
                                color: Theme.of(context).colorScheme.error,
                              ),
                            ),
                            const SizedBox(height: 12),
                            OutlinedButton(
                              onPressed: _render,
                              child: const Text('Retry'),
                            ),
                          ],
                        )
                      : _tree == null
                      ? const SizedBox.shrink()
                      : _widget(context, _tree!),
                ),
                if (_busy)
                  const Positioned(
                    top: 0,
                    left: 0,
                    right: 0,
                    child: LinearProgressIndicator(minHeight: 2),
                  ),
              ],
            );
          },
        );
      },
    );
  }

  List<Widget> _children(BuildContext context, Object? list) => [
    if (list is List)
      for (final c in list)
        if (c is Map<String, dynamic>) _widget(context, c),
  ];

  Widget _widget(BuildContext context, Map<String, dynamic> w) {
    final colors = context.colors;
    String str(String k) => w[k] is String ? w[k] as String : '';
    switch (w['type']) {
      case 'column':
        final kids = _children(context, w['children']);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < kids.length; i++) ...[
              if (i > 0) const SizedBox(height: 12),
              kids[i],
            ],
          ],
        );
      case 'row':
        final raw = (w['children'] is List ? w['children'] as List : const [])
            .whereType<Map<String, dynamic>>()
            .toList();
        final grows = raw.any(
          (c) => c['type'] == 'field' || c['type'] == 'pair',
        );
        if (!grows) {
          return Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [for (final c in raw) _widget(context, c)],
          );
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            for (var i = 0; i < raw.length; i++) ...[
              if (i > 0) const SizedBox(width: 8),
              raw[i]['type'] == 'field' || raw[i]['type'] == 'pair'
                  ? Expanded(child: _widget(context, raw[i]))
                  : _widget(context, raw[i]),
            ],
          ],
        );
      case 'split':
        final left = w['left'] is Map<String, dynamic>
            ? _widget(context, w['left'] as Map<String, dynamic>)
            : const SizedBox.shrink();
        final right = w['right'] is Map<String, dynamic>
            ? _widget(context, w['right'] as Map<String, dynamic>)
            : const SizedBox.shrink();
        return LayoutBuilder(
          builder: (context, box) => box.maxWidth < 640
              ? Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [left, const SizedBox(height: 24), right],
                )
              : Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(flex: 2, child: left),
                    const SizedBox(width: 28),
                    Expanded(flex: 3, child: right),
                  ],
                ),
        );
      case 'heading':
        return Text(
          str('text'),
          style: TextStyle(
            fontFamily: titleFont,
            fontSize: 22,
            fontWeight: FontWeight.w600,
            color: colors.ink,
          ),
        );
      case 'text':
        final style = switch (w['style']) {
          'muted' => TextStyle(color: colors.muted),
          'error' => TextStyle(color: Theme.of(context).colorScheme.error),
          'mono' => TextStyle(fontFamily: monoFont, color: colors.ink),
          _ => TextStyle(color: colors.ink),
        };
        return SelectableText(str('text'), style: style.copyWith(height: 1.4));
      case 'divider':
        return Divider(color: colors.line, height: 1);
      case 'pair':
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              str('label'),
              style: TextStyle(fontSize: 12, color: colors.muted),
            ),
            const SizedBox(height: 2),
            w['link'] == true && _isWebUrl(str('value'))
                ? TextLink(str('value'), onTap: () => openUrl(str('value')))
                : SelectableText(
                    str('value'),
                    style: TextStyle(
                      color: colors.ink,
                      fontFamily: w['mono'] == true ? monoFont : null,
                    ),
                  ),
          ],
        );
      case 'field':
        final id = str('id');
        final c = _fields.putIfAbsent(id, TextEditingController.new);
        final secret = w['secret'] == true;
        final hidden = _hidden[id] ?? true;
        final submit = w['submit'] is String ? w['submit'] as String : null;
        final change = w['change'] is String ? w['change'] as String : null;
        return TextField(
          key: ValueKey('module-field-$id'),
          controller: c,
          autofocus: w['autofocus'] == true,
          obscureText: secret && hidden,
          enableSuggestions: !secret,
          autocorrect: !secret,
          minLines: w['multiline'] == true ? 3 : 1,
          maxLines: w['multiline'] == true ? 8 : 1,
          style: TextStyle(fontFamily: secret && !hidden ? monoFont : null),
          decoration: InputDecoration(
            labelText: str('label'),
            hintText: w['hint'] is String ? w['hint'] as String : null,
            prefixIcon: moduleIcon(w['icon']) == null
                ? null
                : Icon(moduleIcon(w['icon']), size: 18),
            suffixIcon: secret
                ? IconButton(
                    tooltip: hidden ? 'Show' : 'Hide',
                    icon: Icon(
                      hidden
                          ? Icons.visibility_outlined
                          : Icons.visibility_off_outlined,
                      size: 18,
                    ),
                    onPressed: () => setState(() => _hidden[id] = !hidden),
                  )
                : null,
            isDense: true,
            border: const OutlineInputBorder(),
          ),
          onSubmitted: submit == null ? null : (_) => _event(submit),
          onChanged: change == null
              ? null
              : (_) {
                  _debounce?.cancel();
                  _debounce = Timer(
                    const Duration(milliseconds: 200),
                    () => _event(change),
                  );
                },
        );
      case 'button':
        final action = str('action');
        final arg = w['arg'] is String ? w['arg'] as String : null;
        final icon = moduleIcon(w['icon']);
        final label = Text(str('label'));
        final onPressed = _busy ? null : () => _event(action, arg: arg);
        return switch (w['style']) {
          'primary' =>
            icon == null
                ? FilledButton(onPressed: onPressed, child: label)
                : FilledButton.icon(
                    onPressed: onPressed,
                    icon: Icon(icon, size: 18),
                    label: label,
                  ),
          'danger' => TextButton.icon(
            style: TextButton.styleFrom(
              foregroundColor: Theme.of(context).colorScheme.error,
            ),
            onPressed: onPressed,
            icon: Icon(icon ?? Icons.warning_amber_outlined, size: 18),
            label: label,
          ),
          _ =>
            icon == null
                ? TextButton(onPressed: onPressed, child: label)
                : TextButton.icon(
                    onPressed: onPressed,
                    icon: Icon(icon, size: 18),
                    label: label,
                  ),
        };
      case 'list':
        final items = _children(context, w['items']);
        if (items.isEmpty) {
          return Text(str('empty'), style: TextStyle(color: colors.muted));
        }
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: items,
        );
      case 'item':
        final action = w['action'] is String ? w['action'] as String : null;
        final selected = w['selected'] == true;
        return Material(
          color: selected ? colors.accentSoft : Colors.transparent,
          borderRadius: BorderRadius.circular(8),
          child: InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: action == null || _busy
                ? null
                : () => _event(
                    action,
                    arg: w['arg'] is String ? w['arg'] as String : null,
                  ),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    str('title'),
                    style: TextStyle(
                      color: colors.ink,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (str('subtitle').isNotEmpty)
                    Text(
                      str('subtitle'),
                      style: TextStyle(fontSize: 12, color: colors.muted),
                    ),
                ],
              ),
            ),
          ),
        );
    }
    return const SizedBox.shrink();
  }
}

/// Modules may only link to web pages.
bool _isWebUrl(String s) {
  final u = Uri.tryParse(s.trim());
  return u != null &&
      (u.scheme == 'http' || u.scheme == 'https') &&
      u.host.isNotEmpty;
}
