import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/state/settings.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/all_pages_view.dart';
import 'package:synapse/src/views/shell.dart';

import 'support/memory_backend.dart';

MemoryBackend demo() {
  final b = MemoryBackend();
  for (final t in ['Alpha', 'Beta', 'Gamma']) {
    b.createPage(t);
  }
  return b;
}

PageDto byTitle(MemoryBackend b, String t) =>
    b.pages.values.firstWhere((p) => p.title == t);

Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 6; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

Future<AppState> pump(
  WidgetTester tester,
  MemoryBackend b, {
  Widget home = const Shell(),
  AppState? state,
  Size size = const Size(1600, 1000),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final app = state ?? AppState(b);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(theme: buildTheme(Brightness.light), home: home),
    ),
  );
  await settle(tester);
  return app;
}

Future<void> chord(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool shift = false,
}) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  if (shift) await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyEvent(key);
  if (shift) await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await settle(tester);
}

void main() {
  group('tab state', () {
    test('each tab keeps its own history', () {
      final app = AppState(demo());
      app.go(const AllPagesRoute());
      app.openInNewTab(const SettingsRoute());
      expect(app.tabs.length, 2);
      expect(app.activeTab, 1);
      expect(app.canGoBack, isFalse);
      app.go(const TimelineRoute());
      expect(app.canGoBack, isTrue);
      app.activateTab(0);
      expect(app.route, isA<AllPagesRoute>());
      app.back();
      expect(app.route, isA<JournalsRoute>());
      app.activateTab(1);
      expect(app.route, isA<TimelineRoute>());
      app.back();
      expect(app.route, isA<SettingsRoute>());
      app.forward();
      expect(app.route, isA<TimelineRoute>());
    });

    test(
      'closing tabs keeps the active one sensible; the last leaves Journals',
      () {
        final app = AppState(demo());
        app.openInNewTab(const AllPagesRoute());
        app.openInNewTab(const SettingsRoute(), activate: false);
        expect(app.activeTab, 1);
        app.closeTab(0);
        expect(app.activeTab, 0);
        expect(app.route, isA<AllPagesRoute>());
        app.closeTab();
        expect(app.route, isA<SettingsRoute>());
        app.closeTab();
        expect(app.tabs.length, 1);
        expect(app.route, isA<JournalsRoute>());
      },
    );

    test('tabs persist across a restart; broken entries are skipped', () {
      final store = MemorySettings();
      final b = demo();
      final alpha = byTitle(b, 'Alpha');
      final app = AppState(b, settings: store);
      app.go(PageTarget(id: alpha.id));
      app.openInNewTab(const GraphRoute(GraphMode.tree, focusId: 'x'));
      app.openInNewTab(const AllPagesRoute());
      app.activateTab(1);
      final saved = store.load();
      (saved['tabs']['items'] as List).insert(1, {'t': 'nonsense'});
      (saved['tabs']['items'] as List).add('garbage');
      saved['tabs']['active'] = 2;
      store.save(saved);

      final again = AppState(b, settings: store);
      expect(again.tabs.length, 3);
      expect(again.activeTab, 2);
      expect((again.tabs[0].route as PageTarget).id, alpha.id);
      final g = again.tabs[1].route as GraphRoute;
      expect(g.mode, GraphMode.tree);
      expect(g.focusId, 'x');
      expect(again.tabs[2].route, isA<AllPagesRoute>());
      expect(again.canGoBack, isFalse);
    });

    test('deleting a page falls back in tabs and drops recents', () {
      final b = demo();
      final alpha = byTitle(b, 'Alpha');
      final beta = byTitle(b, 'Beta');
      final app = AppState(b);
      app.go(PageTarget(id: beta.id));
      app.go(PageTarget(id: alpha.id));
      app.openInNewTab(PageTarget(id: alpha.id));
      app.recent.add(
        PageBrief(id: alpha.id, title: 'Alpha', isJournal: false, isHub: false),
      );
      app.pagesDeleted([alpha.id]);
      expect(app.recent, isEmpty);
      expect(app.route, isA<JournalsRoute>());
      app.activateTab(0);
      expect((app.route as PageTarget).id, beta.id);
    });

    test('a page opened by title is left too, so it is not recreated', () {
      final b = demo();
      final alpha = byTitle(b, 'Alpha');
      final app = AppState(b);
      app.go(const PageTarget(title: 'alpha'));
      app.pagesDeleted([alpha.id], titles: ['Alpha']);
      expect(app.route, isA<JournalsRoute>());
    });
  });

  group('tabs UI', () {
    testWidgets('keyboard shortcuts open, switch and close tabs', (
      tester,
    ) async {
      final app = await pump(tester, demo());
      expect(app.tabs.length, 1);
      await chord(tester, LogicalKeyboardKey.keyT);
      expect(app.tabs.length, 2);
      expect(app.activeTab, 1);
      await chord(tester, LogicalKeyboardKey.tab);
      expect(app.activeTab, 0);
      await chord(tester, LogicalKeyboardKey.tab, shift: true);
      expect(app.activeTab, 1);
      await chord(tester, LogicalKeyboardKey.keyW);
      expect(app.tabs.length, 1);
      await chord(tester, LogicalKeyboardKey.keyW);
      expect(app.tabs.length, 1);
      expect(app.route, isA<JournalsRoute>());
    });

    testWidgets('mouse back/forward buttons walk the history', (tester) async {
      final app = await pump(tester, demo());
      app.go(const AllPagesRoute());
      await settle(tester);
      Future<void> press(int button) async {
        final mouse = await tester.createGesture(
          kind: PointerDeviceKind.mouse,
          buttons: button,
        );
        await mouse.down(const Offset(800, 500));
        await mouse.up();
        await settle(tester);
      }

      await press(kBackMouseButton);
      expect(app.route, isA<JournalsRoute>());
      await press(kForwardMouseButton);
      expect(app.route, isA<AllPagesRoute>());
    });

    testWidgets('strip shows titles; middle-click closes, plus opens', (
      tester,
    ) async {
      final app = await pump(tester, demo());
      app.go(const AllPagesRoute());
      await settle(tester);
      await tester.tap(find.byKey(const ValueKey('tab-new')));
      await settle(tester);
      expect(app.tabs.length, 2);
      final strip = find.byKey(const ValueKey('tab-strip'));
      expect(
        find.descendant(of: strip, matching: find.text('All pages')),
        findsOneWidget,
      );
      final at = tester.getCenter(
        find.descendant(of: strip, matching: find.text('All pages')),
      );
      final mouse = await tester.createGesture(
        kind: PointerDeviceKind.mouse,
        buttons: kMiddleMouseButton,
      );
      await mouse.down(at);
      await mouse.up();
      await settle(tester);
      expect(app.tabs.length, 1);
      expect(app.route, isA<JournalsRoute>());
    });

    testWidgets('narrow layout lists tabs in a sheet', (tester) async {
      final app = await pump(tester, demo(), size: const Size(500, 900));
      app.openInNewTab(const AllPagesRoute());
      await settle(tester);
      expect(find.byKey(const ValueKey('tab-strip')), findsNothing);
      await tester.tap(find.byKey(const ValueKey('tab-count')));
      await settle(tester);
      expect(find.byKey(const ValueKey('sheet-tab-1')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('sheet-tab-0')));
      await settle(tester);
      expect(app.activeTab, 0);
      await tester.tap(find.byKey(const ValueKey('tab-count')));
      await settle(tester);
      await tester.tap(find.byKey(const ValueKey('sheet-tab-close-1')));
      await settle(tester);
      expect(app.tabs.length, 1);
    });

    testWidgets('ctrl+click on an All pages row opens a new tab', (
      tester,
    ) async {
      final b = demo();
      final app = await pump(tester, b, state: AppState(b));
      app.go(const AllPagesRoute());
      await settle(tester);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.tap(find.text('Beta'));
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await settle(tester);
      expect(app.tabs.length, 2);
      expect(app.activeTab, 1);
      expect((app.route as PageTarget).id, byTitle(b, 'Beta').id);
      expect(app.tabs[0].route, isA<AllPagesRoute>());
    });
  });

  group('All pages actions', () {
    testWidgets('new page: validation, duplicate error and open it', (
      tester,
    ) async {
      final b = demo();
      final app = await pump(
        tester,
        b,
        home: const Scaffold(body: AllPagesView()),
      );
      await tester.tap(find.byKey(const ValueKey('new-page')));
      await settle(tester);
      await tester.tap(find.byKey(const ValueKey('create-page')));
      await settle(tester);
      expect(find.text('Enter a title'), findsOneWidget);

      await tester.enterText(
        find.byKey(const ValueKey('new-page-title')),
        'alpha',
      );
      await tester.tap(find.byKey(const ValueKey('create-page')));
      await settle(tester);
      expect(find.textContaining('already exists'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('open-existing')));
      await settle(tester);
      expect((app.route as PageTarget).id, byTitle(b, 'Alpha').id);

      await tester.tap(find.byKey(const ValueKey('new-page')));
      await settle(tester);
      await tester.enterText(
        find.byKey(const ValueKey('new-page-title')),
        'Delta',
      );
      await tester.tap(find.byKey(const ValueKey('create-page')));
      await settle(tester);
      expect(b.pages.values.where((p) => p.title == 'Delta').length, 1);
      expect((app.route as PageTarget).id, byTitle(b, 'Delta').id);
    });

    testWidgets('single delete asks first and says what happens', (
      tester,
    ) async {
      final b = demo();
      final beta = byTitle(b, 'Beta');
      await pump(tester, b, home: const Scaffold(body: AllPagesView()));
      await tester.tap(find.byKey(ValueKey('page-menu-${beta.id}')));
      await settle(tester);
      await tester.tap(find.text('Delete…'));
      await settle(tester);
      expect(find.text('Delete 1 page?'), findsOneWidget);
      expect(find.textContaining('plain references'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      expect(b.pages.containsKey(beta.id), isTrue);

      await tester.tap(find.byKey(ValueKey('page-menu-${beta.id}')));
      await settle(tester);
      await tester.tap(find.text('Delete…'));
      await settle(tester);
      await tester.tap(find.byKey(const ValueKey('confirm-delete')));
      await settle(tester);
      expect(b.pages.containsKey(beta.id), isFalse);
      expect(find.text('Beta'), findsNothing);
      expect(find.text('Deleted 1 page'), findsOneWidget);
    });

    testWidgets('bulk delete in selection mode; open tab falls back', (
      tester,
    ) async {
      final b = demo();
      final alpha = byTitle(b, 'Alpha');
      final beta = byTitle(b, 'Beta');
      final app = await pump(tester, b, home: const Shell());
      app.go(PageTarget(id: alpha.id));
      app.openInNewTab(const AllPagesRoute());
      await settle(tester);

      await tester.tap(find.byKey(const ValueKey('pages-select')));
      await settle(tester);
      expect(
        tester
            .widget<FilledButton>(
              find.byKey(const ValueKey('pages-delete-selected')),
            )
            .onPressed,
        isNull,
      );
      await tester.tap(find.byKey(ValueKey('page-row-${alpha.id}')));
      await tester.tap(find.byKey(ValueKey('page-row-${beta.id}')));
      await settle(tester);
      await tester.tap(find.byKey(const ValueKey('pages-delete-selected')));
      await settle(tester);
      expect(find.text('Delete 2 pages?'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('confirm-delete')));
      await settle(tester);

      expect(b.pages.values.map((p) => p.title), isNot(contains('Alpha')));
      expect(b.pages.values.map((p) => p.title), isNot(contains('Beta')));
      expect(find.text('Deleted 2 pages'), findsOneWidget);
      app.activateTab(0);
      await settle(tester);
      expect(app.route, isA<JournalsRoute>());
    });
  });
}
