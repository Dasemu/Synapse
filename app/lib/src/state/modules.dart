import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../backend/graph_backend.dart';

/// What each permission lets a module do, for the install consent dialog.
const permissionDescriptions = {
  'storage': 'Keep its own data in this graph (synced to your other devices)',
  'crypto': 'Derive keys from passwords and encrypt or decrypt data (keys stay in the app)',
  'clipboard': 'Copy text to the clipboard (cleared after a delay)',
  'pages:read': 'Read your pages',
  'pages:write': 'Create pages and add blocks to them',
};

/// Modules bundled with the app (Flutter assets), offered for installation in Settings.
const bundledModules = {
  'synapse.secrets': (
    asset: 'assets/modules/synapse.secrets.wasm',
    name: 'Secrets',
    description:
        'An encrypted vault for passwords, unlocked with a master password.',
  ),
};

/// Installed modules and the plumbing around them: sidebar views, block macros, periodic
/// ticks, locking on suspend and the effects modules ask for (clipboard, notices).
class ModulesController extends ChangeNotifier {
  ModulesController(this.backend);

  final GraphBackend backend;

  List<ModuleDto> installed = const [];

  /// Bumped when modules ask for their block macros to be redrawn (lock, unlock, edits,
  /// a tick that changed something). Macro widgets listen to this controller.
  int macroVersion = 0;

  /// Bumped when a view should re-render (a tick changed the module's state).
  int viewVersion = 0;

  final _timers = <String, Timer>{};
  Timer? _clipboardTimer;

  /// Text a module copied with a clear delay, until it is cleared.
  String? _clipboardSecret;
  bool _disposed = false;

  Iterable<ModuleDto> get enabled => installed.where((m) => m.enabled);

  /// Module id rendering `{{name ...}}` blocks (the first enabled one claiming it).
  String? macroOwner(String name) {
    final n = name.toLowerCase();
    for (final m in enabled) {
      if (m.macros.contains(n)) return m.id;
    }
    return null;
  }

  ModuleDto? byId(String id) {
    for (final m in installed) {
      if (m.id == id) return m;
    }
    return null;
  }

  Future<void> load() async {
    try {
      installed = await backend.modules();
    } catch (_) {
      installed = const [];
    }
    if (_disposed) return;
    _restartTimers();
    macroVersion++;
    notifyListeners();
  }

  Future<ModuleDto> inspect(Uint8List wasm) => backend.inspectModule(wasm);

  Future<bool> hasData(String id) async {
    try {
      return await backend.moduleHasData(id);
    } catch (_) {
      return false;
    }
  }

  Future<ModuleDto> install(Uint8List wasm, List<String> granted) async {
    final m = await backend.installModule(wasm, granted);
    await load();
    return m;
  }

  Future<void> setEnabled(String id, bool enabled) async {
    await backend.setModuleEnabled(id, enabled);
    await load();
  }

  Future<void> uninstall(String id, {bool deleteData = false}) async {
    await backend.uninstallModule(id, deleteData: deleteData);
    await load();
  }

  /// Sends a request; errors (including a crashed module) come back as `{'error': ...}`.
  Future<Map<String, dynamic>> call(
    String id,
    Map<String, dynamic> request,
  ) async {
    try {
      return await backend.moduleCall(id, request);
    } catch (e) {
      return {'error': '$e'};
    }
  }

  void _restartTimers() {
    for (final t in _timers.values) {
      t.cancel();
    }
    _timers.clear();
    for (final m in enabled) {
      final s = m.tickSeconds;
      if (s == null || s < 5) continue;
      _timers[m.id] = Timer.periodic(Duration(seconds: s), (_) => _tick(m.id));
    }
  }

  Future<void> _tick(String id) async {
    final r = await call(id, {'op': 'tick'});
    if (_disposed || r['changed'] != true) return;
    refresh();
  }

  /// Redraws module views and macros.
  void refresh() {
    macroVersion++;
    viewVersion++;
    notifyListeners();
  }

  /// The app is going to the background: modules lock what they hold (e.g. the vault), and a
  /// secret still on the clipboard is cleared (a backgrounded app cannot rely on its timer).
  Future<void> suspend() async {
    await clearClipboard();
    var changed = false;
    for (final m in enabled.toList()) {
      final r = await call(m.id, {'op': 'suspend'});
      changed |= r['changed'] == true;
    }
    if (changed && !_disposed) refresh();
  }

  /// Applies the `effects` of a module response. Returns the notices to show and the view to
  /// open, which need a `BuildContext` (the caller shows them).
  Future<({List<String> toasts, String? openView})> applyEffects(
    Map<String, dynamic> response,
  ) async {
    final toasts = <String>[];
    String? openView;
    final effects = response['effects'];
    if (effects is! List) return (toasts: toasts, openView: openView);
    for (final e in effects) {
      if (e is! Map) continue;
      final copy = e['copy'];
      if (copy is Map && copy['text'] is String) {
        final text = copy['text'] as String;
        await Clipboard.setData(ClipboardData(text: text));
        final ms = copy['clear_after_ms'];
        _clipboardTimer?.cancel();
        _clipboardSecret = null;
        if (ms is int && ms > 0) {
          _clipboardSecret = text;
          _clipboardTimer = Timer(Duration(milliseconds: ms), clearClipboard);
        }
      }
      if (e['toast'] is String) toasts.add(e['toast'] as String);
      if (e['open_view'] is String) openView = e['open_view'] as String;
      if (e['refresh_macros'] == true) {
        macroVersion++;
        notifyListeners();
      }
    }
    return (toasts: toasts, openView: openView);
  }

  /// Clears the clipboard if it still holds the secret a module copied (or if it cannot be
  /// read, as on Android in the background: then clearing is the safe choice).
  Future<void> clearClipboard() async {
    final secret = _clipboardSecret;
    _clipboardTimer?.cancel();
    _clipboardSecret = null;
    if (secret == null) return;
    String? now;
    var readable = true;
    try {
      now = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    } catch (_) {
      readable = false;
    }
    if (!readable || now == null || now == secret) {
      await Clipboard.setData(const ClipboardData(text: ''));
    }
  }

  @override
  void dispose() {
    _disposed = true;
    for (final t in _timers.values) {
      t.cancel();
    }
    clearClipboard().ignore();
    super.dispose();
  }
}
