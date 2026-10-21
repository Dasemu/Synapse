import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
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
  AppState(this.backend, {WriteQueue? queue})
    : queue = queue ?? WriteQueue(onError: _logError),
      refs = BlockRefCache(backend);

  final GraphBackend backend;
  final WriteQueue queue;
  final BlockRefCache refs;

  AppRoute _route = const JournalsRoute();
  final _back = <AppRoute>[];
  final _forward = <AppRoute>[];
  ThemeMode themeMode = ThemeMode.system;
  final List<PageBrief> recent = [];

  /// Last page shown (for synapse reinforcement on navigation).
  String? _lastPageId;

  /// Bumped when an error makes views reload from the core.
  int generation = 0;

  AppRoute get route => _route;
  bool get canGoBack => _back.isNotEmpty;
  bool get canGoForward => _forward.isNotEmpty;

  static void _logError(Object e, StackTrace st) {
    debugPrint('graph write failed: $e\n$st');
  }

  void go(AppRoute r) {
    _back.add(_route);
    if (_back.length > 100) _back.removeAt(0);
    _forward.clear();
    _route = r;
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
    recent.insert(
      0,
      PageBrief(
        id: page.id,
        title: page.title,
        isJournal: page.journalDate != null,
      ),
    );
    if (recent.length > 12) recent.removeLast();
    notifyListeners();
  }

  /// Seeds the recents list with recently edited pages.
  Future<void> loadRecent() async {
    final pages = await backend.listPages(limit: 8);
    for (final p in pages) {
      if (!recent.any((r) => r.id == p.id)) {
        recent.add(PageBrief(id: p.id, title: p.title, isJournal: false));
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
      );
    }
    notifyListeners();
  }

  void toggleTheme(Brightness current) {
    themeMode = current == Brightness.dark ? ThemeMode.light : ThemeMode.dark;
    notifyListeners();
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
