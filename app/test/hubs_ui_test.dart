import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/all_pages_view.dart';
import 'package:synapse/src/views/graph_view.dart';
import 'package:synapse/src/views/shell.dart';

import 'support/memory_backend.dart';

/// Fun (hub) -> Fun/TV (sub-hub) -> Anime, Movies, Shows; Fun also holds Games and Books.
MemoryBackend demo() {
  final b = MemoryBackend();
  final fun = b.addHub('Fun', 'Things for the weekend');
  final tv = b.addHub('Fun/TV');
  b.hubsOfPage[tv.id] = {fun.id};
  for (final t in ['Games', 'Books']) {
    b.hubsOfPage[b.createPage(t).id] = {fun.id};
  }
  for (final t in ['Anime', 'Movies', 'Shows']) {
    b.hubsOfPage[b.createPage(t).id] = {tv.id};
  }
  return b;
}

PageDto byTitle(MemoryBackend b, String t) =>
    b.pages.values.firstWhere((p) => p.title == t);

Future<AppState> pump(
  WidgetTester tester,
  MemoryBackend b, {
  Widget home = const Shell(),
  AppRoute? route,
}) async {
  tester.view.physicalSize = const Size(1600, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final app = AppState(b);
  if (route != null) app.go(route);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(theme: buildTheme(Brightness.light), home: home),
    ),
  );
  await settle(tester);
  return app;
}

Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 6; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

void main() {
  tagTests();
  testWidgets('all pages hides hubs and sorts by title', (tester) async {
    await pump(tester, demo(), home: const Scaffold(body: AllPagesView()));
    expect(find.text('Fun'), findsNothing);
    expect(find.text('Games'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('page-sort')));
    await settle(tester);
    await tester.tap(find.text('Title').last);
    await settle(tester);
    // Title sort is A-Z by default... descending arrow means newest/Z-A; flip to A-Z first.
    await tester.tap(find.byKey(const ValueKey('page-sort-direction')));
    await settle(tester);
    final ys = [
      for (final t in ['Anime', 'Books', 'Games', 'Movies', 'Shows'])
        tester.getTopLeft(find.text(t)).dy,
    ];
    expect(ys, [...ys]..sort());
  });

  testWidgets('page sidebar with the local graph toggles', (tester) async {
    final b = demo();
    final app = await pump(tester, b);
    app.openId(byTitle(b, 'Games').id);
    await settle(tester);
    expect(find.byType(LocalGraphPanel), findsNothing);
    await tester.tap(find.byKey(const ValueKey('page-sidebar-toggle')));
    await settle(tester);
    expect(find.byType(LocalGraphPanel), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('page-sidebar-toggle')));
    await settle(tester);
    expect(find.byType(LocalGraphPanel), findsNothing);
  });

  testWidgets('sidebar hubs start collapsed and only list sub-hubs', (
    tester,
  ) async {
    await pump(tester, demo());
    expect(find.text('HUBS'), findsOneWidget);
    expect(find.text('Fun'), findsOneWidget);
    expect(find.text('Fun/TV'), findsNothing);
    await tester.tap(find.byIcon(Icons.chevron_right_rounded));
    await settle(tester);
    expect(find.text('Fun/TV'), findsOneWidget);
    // Notes are never children in the sidebar.
    expect(find.text('Games'), findsNothing);
    expect(find.text('Anime'), findsNothing);
  });

  testWidgets('new note from a hub starts from its template', (tester) async {
    final b = demo();
    final hub = byTitle(b, 'Fun');
    final t = await b.hubTemplate(hub.id);
    b.seed(t, [(0, 'Rating'), (1, 'Notes')]);
    final p = await b.createPageFromTemplate(hub.id, 'Chess');
    expect(b.outlines[p.id]!.map((n) => n.content), ['Rating', 'Notes']);
    expect(b.hubsOfPage[p.id], contains(hub.id));
  });

  testWidgets(
    'sidebar shows HUBS with a plus even without hubs; plus creates',
    (tester) async {
      final b = MemoryBackend()..createPage('Solo');
      await pump(tester, b);
      expect(find.text('HUBS'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('new-hub')));
      await settle(tester);
      await tester.enterText(find.byType(TextField), 'Projects');
      await tester.tap(find.text('Create'));
      await settle(tester);
      final hub = byTitle(b, 'Projects');
      expect(hub.isHub, isTrue);
      // The new hub opens as a hub screen.
      expect(find.byKey(const ValueKey('hub-description')), findsOneWidget);
    },
  );

  testWidgets('new hub with a taken title shows the error inline', (
    tester,
  ) async {
    await pump(tester, demo());
    await tester.tap(find.byKey(const ValueKey('new-hub')));
    await settle(tester);
    await tester.enterText(find.byType(TextField), 'Games');
    await tester.tap(find.text('Create'));
    await settle(tester);
    expect(find.textContaining('already exists'), findsOneWidget);
  });

  testWidgets(
    'hub screen lists sub-hubs first, removes members, saves description',
    (tester) async {
      final b = demo();
      final fun = byTitle(b, 'Fun');
      await pump(tester, b, route: PageTarget(id: fun.id));
      expect(find.byKey(const ValueKey('hub-description')), findsOneWidget);
      expect(find.text('Things for the weekend'), findsOneWidget);
      expect(find.text('MEMBERS'), findsOneWidget);
      // Sub-hub row comes before the notes.
      final rows = find.byWidgetPredicate(
        (w) =>
            w.key is ValueKey &&
            '${(w.key as ValueKey).value}'.startsWith('member-'),
      );
      expect(rows, findsNWidgets(3));
      final ys = [
        for (final t in ['Fun/TV', 'Books', 'Games'])
          tester
              .getTopLeft(find.descendant(of: rows, matching: find.text(t)))
              .dy,
      ];
      expect(ys, orderedEquals([...ys]..sort()));
      // Remove Books (the × shows on hover).
      final books = byTitle(b, 'Books');
      final gesture = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await gesture.addPointer(location: Offset.zero);
      addTearDown(gesture.removePointer);
      await gesture.moveTo(
        tester.getCenter(
          find.descendant(
            of: find.byKey(ValueKey('member-${books.id}')),
            matching: find.text('Books'),
          ),
        ),
      );
      await settle(tester);
      await tester.tap(find.byKey(ValueKey('remove-${books.id}')));
      await settle(tester);
      expect(b.hubsOfPage[books.id], isEmpty);
      expect(find.text('Books'), findsNothing);
      // Description is saved on blur.
      await tester.enterText(
        find.byKey(const ValueKey('hub-description')),
        'New text',
      );
      await tester.testTextInput.receiveAction(TextInputAction.done);
      FocusManager.instance.primaryFocus?.unfocus();
      await settle(tester);
      expect(b.pages[fun.id]!.description, 'New text');
    },
  );

  testWidgets('hub screen adds an existing note and creates a sub-hub', (
    tester,
  ) async {
    final b = demo();
    final lone = b.createPage('Lonely');
    final fun = byTitle(b, 'Fun');
    await pump(tester, b, route: PageTarget(id: fun.id));
    await tester.tap(find.text('Add'));
    await settle(tester);
    await tester.enterText(find.byType(TextField).last, 'Lone');
    await settle(tester);
    await tester.tap(find.byKey(ValueKey('pick-${lone.id}')));
    await settle(tester);
    expect(b.hubsOfPage[lone.id], {fun.id});
    await tester.tap(find.text('Add'));
    await settle(tester);
    await tester.enterText(find.byType(TextField).last, 'Music');
    await settle(tester);
    await tester.tap(find.text('Create hub "Music"'));
    await settle(tester);
    final music = byTitle(b, 'Music');
    expect(music.isHub, isTrue);
    expect(b.hubsOfPage[music.id], {fun.id});
  });

  testWidgets('a hub cannot be added below itself (cycle error shown)', (
    tester,
  ) async {
    final b = demo();
    final fun = byTitle(b, 'Fun');
    final tv = byTitle(b, 'Fun/TV');
    await pump(tester, b, route: PageTarget(id: tv.id));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.text('Add to hub…'));
    await settle(tester);
    // Fun is already a parent, so it is not offered; the hub itself neither.
    expect(find.byKey(ValueKey('pick-${fun.id}')), findsNothing);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    // Adding Fun under Fun/TV is rejected by the backend.
    expect(() => b.addToHub(fun.id, tv.id), throwsException);
  });

  testWidgets('a page can be deleted from its menu, after confirming', (
    tester,
  ) async {
    final b = demo();
    final games = byTitle(b, 'Games');
    final app = await pump(tester, b, route: PageTarget(id: games.id));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('page-delete')));
    await settle(tester);
    // Cancel keeps it.
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(b.pages.containsKey(games.id), isTrue);
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('page-delete')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('confirm-delete')));
    await settle(tester);
    expect(b.pages.containsKey(games.id), isFalse);
    expect(app.route, isNot(PageTarget(id: games.id)));
  });

  testWidgets('note page: In chips can be removed, no hub pill', (
    tester,
  ) async {
    final b = demo();
    final games = byTitle(b, 'Games');
    final fun = byTitle(b, 'Fun');
    await pump(tester, b, route: PageTarget(id: games.id));
    expect(find.text('Hub'), findsNothing);
    expect(find.text('In'), findsOneWidget);
    await tester.tap(find.byKey(ValueKey('leave-${fun.id}')));
    await settle(tester);
    expect(b.hubsOfPage[games.id], isEmpty);
    expect(find.text('In'), findsNothing);
  });

  testWidgets('note page: Add to hub picks a hub', (tester) async {
    final b = demo();
    final lone = b.createPage('Lonely');
    final tv = byTitle(b, 'Fun/TV');
    await pump(tester, b, route: PageTarget(id: lone.id));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.text('Add to hub…'));
    await settle(tester);
    await tester.tap(find.byKey(ValueKey('pick-${tv.id}')));
    await settle(tester);
    expect(b.hubsOfPage[lone.id], {tv.id});
    expect(find.text('Fun/TV'), findsWidgets);
  });

  testWidgets('convert to hub: shortened title, members, keep outline', (
    tester,
  ) async {
    final b = demo();
    final anime = b.createPage('Entretenimiento/TV/Anime');
    b.seed(anime.id, [
      (0, 'A long list of shows I like'),
      (0, '[[Naruto]]'),
      (0, '[[Bleach]]'),
    ]);
    final naruto = b.createPage('Naruto');
    b.createPage('Bleach');
    await pump(tester, b, route: PageTarget(id: anime.id));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.text('Convert to hub…'));
    await settle(tester);
    expect(find.text('Convert to hub'), findsOneWidget);
    final title = tester.widget<TextField>(
      find.byKey(const ValueKey('convert-title')),
    );
    expect(title.controller!.text, 'Anime');
    expect(find.text('MEMBERS (2/2)'), findsOneWidget);
    // Uncheck Bleach; the outline has prose, so "keep" is on by default.
    await tester.tap(find.byKey(ValueKey('member-${byTitle(b, 'Bleach').id}')));
    await settle(tester);
    expect(find.text('MEMBERS (1/2)'), findsOneWidget);
    expect(
      tester
          .widget<CheckboxListTile>(find.byKey(const ValueKey('keep-outline')))
          .value,
      isTrue,
    );
    await tester.tap(find.text('Convert'));
    await settle(tester);
    // "Anime" already exists in the demo: the error is shown inline.
    expect(find.byKey(const ValueKey('convert-error')), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('convert-title')),
      'Anime (TV)',
    );
    await tester.tap(find.text('Convert'));
    await settle(tester);
    final hub = b.pages[anime.id]!;
    expect(hub.isHub, isTrue);
    expect(hub.title, 'Anime (TV)');
    expect(hub.description, 'A long list of shows I like');
    expect(b.outlines[anime.id], isEmpty);
    expect(b.hubsOfPage[naruto.id], {anime.id});
    final note = byTitle(b, 'Anime (notes)');
    expect(b.outlines[note.id], hasLength(3));
    expect(b.hubsOfPage[note.id], {anime.id});
    // The page now renders as a hub.
    expect(find.byKey(const ValueKey('hub-description')), findsOneWidget);
  });

  testWidgets('convert is disabled while "keep outline" has no note title', (
    tester,
  ) async {
    final b = demo();
    final p = b.createPage('Area of Work');
    b.seed(p.id, [(0, 'Some prose')]);
    await pump(tester, b, route: PageTarget(id: p.id));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.text('Convert to hub…'));
    await settle(tester);
    FilledButton convert() => tester.widget<FilledButton>(
      find.widgetWithText(FilledButton, 'Convert'),
    );
    expect(convert().onPressed, isNotNull);
    await tester.enterText(
      find.byKey(const ValueKey('convert-note-title')),
      '',
    );
    await settle(tester);
    expect(convert().onPressed, isNull);
    expect(
      find.text('A title is required to keep the outline'),
      findsOneWidget,
    );
    // Unticking "keep" re-enables it (the outline is then dropped on purpose).
    await tester.tap(find.byKey(const ValueKey('keep-outline')));
    await settle(tester);
    expect(convert().onPressed, isNotNull);
    expect(b.outlines[p.id], isNotEmpty);
  });

  testWidgets('converting a page opened by title does not recreate the title', (
    tester,
  ) async {
    final b = demo();
    final p = b.createPage('Area of Work');
    b.seed(p.id, [(0, 'Some prose')]);
    final app = await pump(tester, b, route: PageTarget(title: 'Area of Work'));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.text('Convert to hub…'));
    await settle(tester);
    await tester.enterText(find.byKey(const ValueKey('convert-title')), 'Area');
    await tester.tap(find.text('Convert'));
    await settle(tester);
    expect(find.byKey(const ValueKey('hub-description')), findsOneWidget);
    expect(b.pages[p.id]!.title, 'Area');
    expect(b.pages.values.where((q) => q.title == 'Area of Work'), isEmpty);
    expect((app.route as PageTarget).id, p.id);
  });

  testWidgets('renaming rewrites history entries that open the old title', (
    tester,
  ) async {
    final b = demo();
    final p = b.createPage('Old name');
    final app = await pump(tester, b, route: PageTarget(title: 'Old name'));
    app.go(PageTarget(id: byTitle(b, 'Games').id));
    await settle(tester);
    app.back();
    await settle(tester);
    await tester.enterText(find.byType(TextField).first, 'New name');
    FocusManager.instance.primaryFocus?.unfocus();
    await settle(tester);
    expect(b.pages[p.id]!.title, 'New name');
    expect(b.pages.values.where((q) => q.title == 'Old name'), isEmpty);
    expect((app.route as PageTarget).id, p.id);
    app.go(PageTarget(id: byTitle(b, 'Games').id));
    app.back();
    await settle(tester);
    expect((app.route as PageTarget).id, p.id);
  });

  testWidgets('adding a note to another hub moves it', (tester) async {
    final b = demo();
    final other = b.addHub('Other');
    final tv = byTitle(b, 'Fun/TV');
    await b.addToHub(tv.id, other.id);
    expect(b.hubsOfPage[tv.id], {other.id});
  });

  testWidgets('cleanup never preselects hubs or pages with content', (
    tester,
  ) async {
    final b = demo();
    final todo = b.createPage('todo');
    b.seed(todo.id, [(0, 'real tasks')]);
    final hub = b.addHub('later');
    final empty = b.createPage('now');
    final contents = b.createPage('contents');
    b.seed(contents.id, [(0, 'x')]);
    for (final p in [todo, hub, empty, contents]) {
      b.legacy[p.id] = 'Logseq built-in';
    }
    final c = {for (final x in await b.legacyCandidates()) x.title: x};
    expect(c['todo']!.preselected, isFalse);
    expect(c['later']!.preselected, isFalse);
    expect(c['now']!.preselected, isTrue);
    expect(c['contents']!.preselected, isTrue);
  });

  testWidgets('memory backend: inHubs only lists hubs, keep title is checked', (
    tester,
  ) async {
    final b = demo();
    final p = b.createPage('Plain');
    final notHub = b.createPage('NotHub');
    b.hubsOfPage[p.id] = {byTitle(b, 'Fun').id, notHub.id};
    final st = await b.pageStructure(p.id);
    expect(st.inHubs.map((h) => h.title), ['Fun']);
    b.seed(p.id, [(0, 'x')]);
    await expectLater(
      b.convertToHub(p.id, 'Same', '', [], keepOutlineAs: 'same'),
      throwsException,
    );
    await expectLater(
      b.convertToHub(p.id, 'Same', '', [], keepOutlineAs: '  '),
      throwsException,
    );
    expect(b.pages[p.id]!.isHub, isFalse);
  });

  testWidgets('convert to note turns members into links', (tester) async {
    final b = demo();
    final tv = byTitle(b, 'Fun/TV');
    await pump(tester, b, route: PageTarget(id: tv.id));
    await tester.tap(find.byKey(const ValueKey('page-menu')));
    await settle(tester);
    await tester.tap(find.text('Convert to note'));
    await settle(tester);
    expect(b.pages[tv.id]!.isHub, isFalse);
    expect(b.outlines[tv.id]!.map((n) => n.content), [
      '[[Anime]]',
      '[[Movies]]',
      '[[Shows]]',
    ]);
  });

  testWidgets('page screen shows the namespace breadcrumb and children', (
    tester,
  ) async {
    final b = demo();
    final a = b.createPage('Claude/Code');
    b.createPage('Claude');
    await pump(tester, b, route: PageTarget(id: a.id));
    expect(find.byIcon(Icons.account_tree_outlined), findsWidgets);
    final c = byTitle(b, 'Claude');
    await pump(tester, b, route: PageTarget(id: c.id));
    expect(find.text('NAMESPACE'), findsOneWidget);
  });

  testWidgets('cleanup deletes only the checked pages', (tester) async {
    final b = demo();
    final a = b.createPage('contents');
    final c = b.createPage('Keep me');
    b.seed(c.id, [(0, 'real content')]);
    final d = b.createPage('card');
    b.legacy[a.id] = 'Logseq built-in';
    b.legacy[c.id] = 'type:: system';
    b.legacy[d.id] = 'Logseq built-in';
    await pump(tester, b, home: const Scaffold(body: AllPagesView()));
    await tester.tap(find.text('Clean up'));
    await settle(tester);
    expect(find.text('Remove legacy Logseq pages'), findsOneWidget);
    expect(find.text('Delete (2)'), findsOneWidget);
    // Uncheck "card".
    await tester.tap(
      find.descendant(
        of: find.byType(AlertDialog),
        matching: find.text('card'),
      ),
    );
    await settle(tester);
    expect(find.text('Delete (1)'), findsOneWidget);
    await tester.tap(find.text('Delete (1)'));
    await settle(tester);
    expect(b.purged, [a.id]);
    expect(b.pages.containsKey(a.id), isFalse);
    expect(b.pages.containsKey(c.id), isTrue);
    expect(b.pages.containsKey(d.id), isTrue);
  });

  testWidgets('cleanup with nothing to remove says so', (tester) async {
    await pump(tester, demo(), home: const Scaffold(body: AllPagesView()));
    await tester.tap(find.text('Clean up'));
    await settle(tester);
    expect(find.text('No legacy pages found.'), findsOneWidget);
  });
}

void tagTests() {
  testWidgets('tags are names: pick one, create one, remove, sorted', (
    tester,
  ) async {
    final b = demo();
    final note = byTitle(b, 'Games');
    await b.setPageTags(note.id, ['tema']);
    await pump(tester, b, route: PageTarget(id: note.id));
    expect(find.byKey(const ValueKey('tag-tema')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('add-tag')));
    await settle(tester);
    // Tags already on the page are not offered again; a new name just creates a tag.
    expect(find.byKey(const ValueKey('tag-opt-tema')), findsNothing);
    await tester.enterText(find.byKey(const ValueKey('tag-input')), '#ocio');
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('tag-create')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('tag-done')));
    await settle(tester);
    // Stored and shown sorted.
    expect(byTitle(b, 'Games').tags, ['ocio', 'tema']);
    expect(
      tester.getTopLeft(find.byKey(const ValueKey('tag-ocio'))).dx,
      lessThan(tester.getTopLeft(find.byKey(const ValueKey('tag-tema'))).dx),
    );
    // Tags are names, never pages.
    expect(b.pages.values.any((p) => p.title == 'ocio'), isFalse);

    final gesture = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await gesture.addPointer(location: Offset.zero);
    await gesture.moveTo(
      tester.getCenter(find.byKey(const ValueKey('tag-tema'))),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    await tester.tap(find.byKey(const ValueKey('untag-tema')));
    await settle(tester);
    expect(byTitle(b, 'Games').tags, ['ocio']);
  });

  testWidgets('a page has one type: set, change, remove', (tester) async {
    final b = demo();
    final note = byTitle(b, 'Games');
    await pump(tester, b, route: PageTarget(id: note.id));
    expect(find.byKey(const ValueKey('type-pill')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('add-type')));
    await settle(tester);
    expect(find.byKey(const ValueKey('type-opt-persona')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('type-opt-lugar')));
    await settle(tester);
    expect(byTitle(b, 'Games').pageType, 'lugar');
    expect(find.byKey(const ValueKey('type-pill')), findsOneWidget);

    // Change it: the single-select picker replaces the type, with a new name too.
    await tester.tap(find.byKey(const ValueKey('type-pill')));
    await settle(tester);
    await tester.enterText(find.byKey(const ValueKey('type-input')), 'ocio');
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('type-create')));
    await settle(tester);
    expect(byTitle(b, 'Games').pageType, 'ocio');
    expect(b.pages.values.any((p) => p.title == 'ocio'), isFalse);

    await tester.tap(find.byKey(const ValueKey('type-pill')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('type-remove')));
    await settle(tester);
    expect(byTitle(b, 'Games').pageType, isNull);
    expect(find.byKey(const ValueKey('add-type')), findsOneWidget);
  });

  testWidgets('a tag chip opens the tag view with typed and tagged pages', (
    tester,
  ) async {
    final b = demo();
    final games = byTitle(b, 'Games');
    final other = b.pages.values.firstWhere((p) => p.id != games.id);
    await b.setPageTags(games.id, ['ocio']);
    await b.setPageType(other.id, 'ocio');
    await pump(tester, b, route: PageTarget(id: games.id));
    await tester.tap(find.byKey(const ValueKey('tag-ocio')));
    await settle(tester);
    expect(find.byKey(const ValueKey('tag-title')), findsOneWidget);
    expect(find.byKey(ValueKey('tag-page-${games.id}')), findsOneWidget);
    expect(find.byKey(ValueKey('tag-page-${other.id}')), findsOneWidget);
  });

  testWidgets('renaming and removing a tag from its view', (tester) async {
    final b = demo();
    final games = byTitle(b, 'Games');
    await b.setPageTags(games.id, ['ocio']);
    await pump(tester, b, route: const TagRoute('ocio'));
    await tester.tap(find.byKey(const ValueKey('tag-rename')));
    await settle(tester);
    await tester.enterText(
      find.byKey(const ValueKey('tag-rename-input')),
      'tiempo libre',
    );
    await tester.tap(find.byKey(const ValueKey('tag-rename-confirm')));
    await settle(tester);
    expect(byTitle(b, 'Games').tags, ['tiempo libre']);
    expect(find.byKey(ValueKey('tag-page-${games.id}')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('tag-remove')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('tag-remove-confirm')));
    await settle(tester);
    expect(byTitle(b, 'Games').tags, isEmpty);
  });
}
