import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../graph/network_canvas.dart';
import '../theme/theme.dart';
import 'keymap.dart';
import 'settings.dart';
import 'write_queue.dart';

sealed class AppRoute {
  const AppRoute();
}

class JournalsRoute extends AppRoute {
  const JournalsRoute();
}

class TimelineRoute extends AppRoute {
  const TimelineRoute();
}

class AllPagesRoute extends AppRoute {
  const AllPagesRoute();
}

class SettingsRoute extends AppRoute {
  const SettingsRoute();
}

enum GraphMode {
  network('Network', Icons.hub_outlined),
  brain('Synapses', Icons.bolt_outlined),
  clusters('Clusters', Icons.bubble_chart_outlined),
  tree('Tree', Icons.account_tree_outlined),
  timeline('Timeline', Icons.timeline_rounded);

  const GraphMode(this.label, this.icon);
  final String label;
  final IconData icon;
}

/// The graph view in a given mode, optionally focused on a page.
class GraphRoute extends AppRoute {
  const GraphRoute(this.mode, {this.focusId});
  final GraphMode mode;
  final String? focusId;
}

/// A page, by id or (for links that may not exist yet) by title.
class PageTarget extends AppRoute {
  const PageTarget({this.id, this.title}) : assert(id != null || title != null);
  final String? id;
  final String? title;
}

/// App-wide state: the open graph, navigation history, recents and theme.
///
/// State management is plain `ChangeNotifier`s: one for the app, one per open outline
/// (`OutlineController`) and a block-ref cache. The state is small and mostly local to a
/// page, so a framework (Riverpod, Bloc) would add a dependency and indirection without
/// paying for itself; rebuilds are scoped with `ListenableBuilder`.
class AppState extends ChangeNotifier {
  AppState(this.backend, {WriteQueue? queue, SettingsStore? settings})
    : refs = BlockRefCache(backend),
      queue = queue ?? WriteQueue(),
      _settings = settings ?? MemorySettings() {
    this.queue.onError = _writeFailed;
    final saved = _settings.load();
    final id = saved['theme'];
    if (id is String && (id == autoThemeId || themeById(id) != null)) {
      themeId = id;
    }
    final keys = saved['keys'];
    keymap = Keymap(
      keys is Map
          ? {for (final e in keys.entries) '${e.key}': '${e.value}'}
          : {},
    );
  }

  final SettingsStore _settings;

  /// Key bindings (defaults plus the user's overrides).
  late final Keymap keymap;

  /// Selected theme id (`auto` follows the system).
  String themeId = autoThemeId;

  void _saveSettings() {
    _settings.save({'theme': themeId, 'keys': keymap.toJson()});
  }

  void setTheme(String id) {
    themeId = id;
    _saveSettings();
    notifyListeners();
  }

  /// Binds `combo` to `action`; returns the action already using it (nothing changes then).
  AppAction? bindKey(AppAction action, Combo combo) {
    final owner = keymap.bind(action, combo);
    if (owner == null) {
      _saveSettings();
      notifyListeners();
    }
    return owner;
  }

  void resetKey(AppAction? action) {
    action == null ? keymap.resetAll() : keymap.reset(action);
    _saveSettings();
    notifyListeners();
  }

  final GraphBackend backend;
  final WriteQueue queue;
  final BlockRefCache refs;

  AppRoute _route = const JournalsRoute();
  final _back = <AppRoute>[];
  final _forward = <AppRoute>[];
  final List<PageBrief> recent = [];

  /// Node positions shared by the graph views (stable layouts across modes and filters).
  final graphPositions = PositionCache();

  /// Last page shown (for synapse reinforcement on navigation).
  String? _lastPageId;

  /// Bumped to make the current view reload from the core (failed writes, renames).
  int generation = 0;

  /// Hubs expanded in the sidebar tree (kept here so it survives sidebar rebuilds).
  final expandedHubs = <String>{};

  /// Right sidebar of the page editor (local graph + related pages); collapsed by default.
  bool pageSidebar = false;

  void togglePageSidebar() {
    pageSidebar = !pageSidebar;
    notifyListeners();
  }

  /// Last write error, shown once by the shell.
  String? writeError;

  AppRoute get route => _route;
  bool get canGoBack => _back.isNotEmpty;
  bool get canGoForward => _forward.isNotEmpty;

  /// A failed write leaves the optimistic UI out of sync: reload the view from the core.
  void _writeFailed(Object e, StackTrace st) {
    debugPrint('graph write failed: $e\n$st');
    writeError = '$e';
    reload();
  }

  /// Rebuilds the current view from the core (after queued writes have run).
  void reload() {
    generation++;
    notifyListeners();
  }

  void go(AppRoute r) {
    _back.add(_route);
    if (_back.length > 100) _back.removeAt(0);
    _forward.clear();
    _route = r;
    notifyListeners();
  }

  /// Points the current route and any history entry that opens `oldTitle` at the page `id`,
  /// without pushing history (the page was renamed or converted, so its old title no longer
  /// exists and opening it would create an empty duplicate).
  void replaceWithId(String id, String oldTitle) {
    final key = oldTitle.toLowerCase();
    AppRoute fix(AppRoute r) =>
        r is PageTarget && r.id == null && r.title!.toLowerCase() == key
        ? PageTarget(id: id)
        : r;
    if (_route is PageTarget) _route = PageTarget(id: id);
    for (final l in [_back, _forward]) {
      for (var i = 0; i < l.length; i++) {
        l[i] = fix(l[i]);
      }
    }
    generation++;
    notifyListeners();
  }

  void back() {
    if (_back.isEmpty) return;
    _forward.add(_route);
    _route = _back.removeLast();
    notifyListeners();
  }

  void forward() {
    if (_forward.isEmpty) return;
    _back.add(_route);
    _route = _forward.removeLast();
    notifyListeners();
  }

  void openTitle(String title) => go(PageTarget(title: title));
  void openId(String id) => go(PageTarget(id: id));

  /// Called by the page view once it knows which page it shows.
  void pageShown(PageDto page) {
    final from = _lastPageId;
    _lastPageId = page.id;
    if (from != page.id) {
      queue.run(() => backend.recordVisit(from, page.id)).ignore();
    }
    recent.removeWhere((p) => p.id == page.id);
    // Hubs are folders, not pages: they never show up in recents.
    if (!page.isHub) {
      recent.insert(
        0,
        PageBrief(
          id: page.id,
          title: page.title,
          isJournal: page.journalDate != null,
          isHub: false,
        ),
      );
      if (recent.length > 12) recent.removeLast();
    }
    notifyListeners();
  }

  /// Seeds the recents list with recently edited pages.
  Future<void> loadRecent() async {
    final pages = await backend.listPages(limit: 8);
    for (final p in pages) {
      if (!p.isHub && !recent.any((r) => r.id == p.id)) {
        recent.add(
          PageBrief(id: p.id, title: p.title, isJournal: false, isHub: p.isHub),
        );
      }
    }
    notifyListeners();
  }

  void renamed(String id, String title) {
    final i = recent.indexWhere((p) => p.id == id);
    if (i >= 0) {
      recent[i] = PageBrief(
        id: id,
        title: title,
        isJournal: recent[i].isJournal,
        isHub: recent[i].isHub,
      );
    }
    notifyListeners();
  }

  /// Top-bar shortcut: flips between the light and dark Paper themes.
  void toggleTheme(Brightness current) {
    setTheme(current == Brightness.dark ? 'paper' : 'paper-dark');
  }
}

/// Resolved `((block refs))`, loaded lazily and shared by every rendered block.
class BlockRefCache extends ChangeNotifier {
  BlockRefCache(this._backend);

  final GraphBackend _backend;
  final _cache = <String, BlockRefDto?>{};
  final _loading = <String>{};

  /// Cached target, or null while loading / when missing ([isMissing] tells them apart).
  BlockRefDto? get(String id) {
    if (_cache.containsKey(id)) return _cache[id];
    if (_loading.add(id)) {
      _backend
          .blockRef(id)
          .then(
            (r) {
              _cache[id] = r;
              _loading.remove(id);
              notifyListeners();
            },
            onError: (Object _) {
              _cache[id] = null;
              _loading.remove(id);
              notifyListeners();
            },
          );
    }
    return null;
  }

  bool isMissing(String id) => _cache.containsKey(id) && _cache[id] == null;

  void invalidate(String id) {
    if (_cache.remove(id) != null) notifyListeners();
  }
}

/// Gives widgets access to the [AppState].
class AppScope extends InheritedNotifier<AppState> {
  const AppScope({super.key, required AppState state, required super.child})
    : super(notifier: state);

  static AppState of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AppScope>()!.notifier!;

  /// Access without subscribing to changes.
  static AppState read(BuildContext context) =>
      context.getInheritedWidgetOfExactType<AppScope>()!.notifier!;
}
