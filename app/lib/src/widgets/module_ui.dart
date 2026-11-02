import 'dart:async';

import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';

/// Icon names modules may use in buttons and macro actions.
IconData? moduleIcon(Object? name) => switch (name) {
  'add' => Icons.add,
  'lock' => Icons.lock_outline,
  'unlock' => Icons.lock_open_outlined,
  'copy' => Icons.copy_outlined,
  'eye' => Icons.visibility_outlined,
  'eye_off' => Icons.visibility_off_outlined,
  'edit' => Icons.edit_outlined,
  'delete' => Icons.delete_outline,
  'key' => Icons.key_outlined,
  'refresh' => Icons.refresh,
  'save' => Icons.check,
  'close' => Icons.close,
  'search' => Icons.search,
  'link' => Icons.link,
  'person' => Icons.person_outline,
  _ => null,
};

/// Applies a module response's effects: clipboard (through the controller), notices as snack
/// bars, and opening one of the module's views.
Future<void> applyModuleEffects(
  BuildContext context,
  String moduleId,
  Map<String, dynamic> response,
) async {
  final app = AppScope.read(context);
  final messenger = ScaffoldMessenger.maybeOf(context);
  final out = await app.modules.applyEffects(response);
  for (final t in out.toasts) {
    messenger?.showSnackBar(SnackBar(content: Text(t)));
  }
  final view = out.openView;
  if (view != null) {
    final m = app.modules.byId(moduleId);
    final decl = m?.views.where((v) => v.id == view).firstOrNull;
    if (decl != null) {
      app.go(ModuleRoute(moduleId, view, title: decl.title));
    }
  }
}

/// A `{{name args}}` macro rendered by a module inside a block: its label, a value (masked or
/// not) and action buttons. Redrawn when the module asks (`ModulesController.macroVersion`).
class ModuleMacro extends StatefulWidget {
  const ModuleMacro({
    super.key,
    required this.moduleId,
    required this.name,
    required this.args,
  });

  final String moduleId;
  final String name;
  final String args;

  @override
  State<ModuleMacro> createState() => _ModuleMacroState();
}

class _ModuleMacroState extends State<ModuleMacro> {
  Map<String, dynamic>? _inline;
  int _version = -1;
  bool _busy = false;

  Map<String, dynamic> get _request => {
    'name': widget.name,
    'args': widget.args,
  };

  @override
  void didUpdateWidget(ModuleMacro old) {
    super.didUpdateWidget(old);
    if (old.args != widget.args || old.moduleId != widget.moduleId) {
      _version = -1;
    }
  }

  Future<void> _load(int version) async {
    _version = version;
    final r = await AppScope.read(context).modules
        .call(widget.moduleId, {'op': 'macro', ..._request});
    if (!mounted) return;
    setState(() => _inline = _inlineOf(r));
  }

  Map<String, dynamic> _inlineOf(Map<String, dynamic> r) {
    final i = r['inline'];
    if (i is Map<String, dynamic>) return i;
    return {'text': widget.args, 'error': '${r['error'] ?? 'No answer'}'};
  }

  Future<void> _action(String id) async {
    if (_busy) return;
    setState(() => _busy = true);
    final r = await AppScope.read(context).modules.call(widget.moduleId, {
      'op': 'macro_action',
      'action': id,
      ..._request,
    });
    if (!mounted) return;
    setState(() {
      _busy = false;
      _inline = _inlineOf(r);
    });
    await applyModuleEffects(context, widget.moduleId, r);
  }

  @override
  Widget build(BuildContext context) {
    final modules = AppScope.read(context).modules;
    return ListenableBuilder(
      listenable: modules,
      builder: (context, _) {
        if (_version != modules.macroVersion) {
          final v = modules.macroVersion;
          scheduleMicrotask(() {
            if (mounted && _version != v) _load(v);
          });
        }
        return _chip(context);
      },
    );
  }

  Widget _chip(BuildContext context) {
    final colors = context.colors;
    final i = _inline;
    final small = TextStyle(fontSize: 13, color: colors.ink);
    final children = <Widget>[];
    if (i == null) {
      children.add(Text('{{${widget.name} ${widget.args}}}', style: small));
    } else {
      final icon = i['icon'];
      if (icon is String) {
        children.add(Text(icon, style: const TextStyle(fontSize: 12)));
        children.add(const SizedBox(width: 5));
      }
      children.add(
        Text(
          '${i['text'] ?? widget.args}',
          style: small.copyWith(fontWeight: FontWeight.w600),
        ),
      );
      final error = i['error'];
      final masked = i['masked'];
      if (error is String) {
        children.add(const SizedBox(width: 8));
        children.add(
          Text(
            error,
            style: small.copyWith(color: Theme.of(context).colorScheme.error),
          ),
        );
      } else if (masked is String) {
        children.add(const SizedBox(width: 8));
        children.add(
          SelectableText(
            masked,
            style: small.copyWith(
              fontFamily: i['revealed'] == true ? monoFont : null,
              color: i['revealed'] == true ? colors.ink : colors.muted,
            ),
          ),
        );
      }
      final actions = i['actions'];
      if (actions is List) {
        for (final a in actions) {
          if (a is! Map || a['id'] is! String) continue;
          children.add(
            IconButton(
              tooltip: a['label'] is String ? a['label'] as String : null,
              visualDensity: VisualDensity.compact,
              iconSize: 15,
              padding: EdgeInsets.zero,
              constraints: const BoxConstraints.tightFor(width: 26, height: 22),
              onPressed: _busy ? null : () => _action(a['id'] as String),
              icon: Icon(moduleIcon(a['icon']) ?? Icons.bolt_outlined),
            ),
          );
        }
      }
    }
    // Taps on the chip must not start editing the block.
    return GestureDetector(
      onTapUp: (_) {},
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.symmetric(horizontal: 1),
        padding: const EdgeInsets.only(left: 7, right: 3, top: 1, bottom: 1),
        decoration: BoxDecoration(
          color: colors.codeBg,
          border: Border.all(color: colors.line),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Row(mainAxisSize: MainAxisSize.min, children: children),
      ),
    );
  }
}
