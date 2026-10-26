import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// A key plus modifiers. Ctrl also matches Cmd on macOS.
@immutable
class Combo {
  const Combo(
    this.key, {
    this.ctrl = false,
    this.shift = false,
    this.alt = false,
  });

  final LogicalKeyboardKey key;
  final bool ctrl;
  final bool shift;
  final bool alt;

  /// `ctrl+shift+<keyId>`: stable across platforms and keyboard layouts.
  String encode() =>
      '${ctrl ? 'ctrl+' : ''}${shift ? 'shift+' : ''}${alt ? 'alt+' : ''}${key.keyId}';

  static Combo? decode(String s) {
    final parts = s.split('+');
    final id = int.tryParse(parts.last);
    if (id == null) return null;
    final mods = parts.sublist(0, parts.length - 1);
    if (mods.any((m) => m != 'ctrl' && m != 'shift' && m != 'alt')) return null;
    return Combo(
      LogicalKeyboardKey.findKeyByKeyId(id) ?? LogicalKeyboardKey(id),
      ctrl: mods.contains('ctrl'),
      shift: mods.contains('shift'),
      alt: mods.contains('alt'),
    );
  }

  /// A key that is only a modifier cannot be bound on its own.
  bool get isModifierOnly => const {
    'Control Left',
    'Control Right',
    'Shift Left',
    'Shift Right',
    'Alt Left',
    'Alt Right',
    'Meta Left',
    'Meta Right',
  }.contains(key.keyLabel);

  String get label {
    final k = key.keyLabel.isEmpty ? 'Key ${key.keyId}' : key.keyLabel;
    return [if (ctrl) 'Ctrl', if (shift) 'Shift', if (alt) 'Alt', k].join('+');
  }

  @override
  bool operator ==(Object other) =>
      other is Combo &&
      other.key == key &&
      other.ctrl == ctrl &&
      other.shift == shift &&
      other.alt == alt;

  @override
  int get hashCode => Object.hash(key, ctrl, shift, alt);
}

/// Everything that can be bound to a key. `editor` actions apply inside a focused block;
/// the others anywhere in the app.
enum AppAction {
  quickSwitcher('Quick switcher', false),
  journals('Journals', false),
  graph('Graph view', false),
  timeline('Timeline', false),
  sidebar('Toggle sidebar', false),
  pageSidebar('Toggle graph sidebar', false),
  settings('Settings', false),
  back('Back', false),
  forward('Forward', false),
  bold('Bold', true),
  italic('Italic', true),
  strike('Strikethrough', true),
  highlight('Highlight', true),
  code('Inline code', true),
  link('Page link', true);

  const AppAction(this.label, this.editor);
  final String label;
  final bool editor;

  Combo get defaultCombo => switch (this) {
    quickSwitcher => Combo(LogicalKeyboardKey.keyK, ctrl: true),
    journals => Combo(LogicalKeyboardKey.keyJ, ctrl: true, shift: true),
    graph => Combo(LogicalKeyboardKey.keyG, ctrl: true, shift: true),
    timeline => Combo(LogicalKeyboardKey.keyT, ctrl: true, shift: true),
    sidebar => Combo(LogicalKeyboardKey.backslash, ctrl: true),
    pageSidebar => Combo(LogicalKeyboardKey.backslash, ctrl: true, shift: true),
    settings => Combo(LogicalKeyboardKey.comma, ctrl: true),
    back => Combo(LogicalKeyboardKey.arrowLeft, alt: true),
    forward => Combo(LogicalKeyboardKey.arrowRight, alt: true),
    bold => Combo(LogicalKeyboardKey.keyB, ctrl: true),
    italic => Combo(LogicalKeyboardKey.keyI, ctrl: true),
    strike => Combo(LogicalKeyboardKey.keyS, ctrl: true, shift: true),
    highlight => Combo(LogicalKeyboardKey.keyH, ctrl: true, shift: true),
    code => Combo(LogicalKeyboardKey.keyE, ctrl: true),
    link => Combo(LogicalKeyboardKey.keyL, ctrl: true),
  };
}

/// Default bindings plus the user's overrides. Combos are unique across all actions; binding
/// one that another action holds is refused (the caller shows which one).
class Keymap {
  Keymap([Map<String, String> overrides = const {}]) {
    for (final e in overrides.entries) {
      final action = AppAction.values.asNameMap()[e.key];
      final combo = Combo.decode(e.value);
      if (action != null && combo != null) _custom[action] = combo;
    }
  }

  final _custom = <AppAction, Combo>{};

  Combo of(AppAction a) => _custom[a] ?? a.defaultCombo;
  bool isCustom(AppAction a) => _custom.containsKey(a);

  /// The action bound to `c`, if any.
  AppAction? actionFor(Combo c, {bool? editor}) {
    for (final a in AppAction.values) {
      if ((editor == null || a.editor == editor) && of(a) == c) return a;
    }
    return null;
  }

  /// Binds `c` to `a`. Returns the action that already holds `c`, leaving the map unchanged.
  AppAction? bind(AppAction a, Combo c) {
    final owner = actionFor(c);
    if (owner != null && owner != a) return owner;
    if (c == a.defaultCombo) {
      _custom.remove(a);
    } else {
      _custom[a] = c;
    }
    return null;
  }

  void reset(AppAction a) => _custom.remove(a);

  void resetAll() => _custom.clear();

  Map<String, String> toJson() => {
    for (final e in _custom.entries) e.key.name: e.value.encode(),
  };
}
