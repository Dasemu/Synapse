import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/graph/graph_model.dart';
import 'package:synapse/src/graph/tree_canvas.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/all_pages_view.dart';
import 'package:synapse/src/views/graph_view.dart';
import 'package:synapse/src/views/quick_switcher.dart';
import 'package:synapse/src/views/shell.dart';
import 'package:synapse/src/views/tags_types_view.dart';
import 'package:synapse/src/widgets/emoji_picker.dart';
import 'package:synapse/src/widgets/type_icon.dart';

import 'support/memory_backend.dart';

/// Ada (persona, tag "math"), Bob (persona), Rome (lugar), Notes (untyped).
Future<MemoryBackend> demo() async {
  final b = MemoryBackend();
  final ada = b.createPage('Ada');
  await b.setPageType(ada.id, 'persona');
  await b.addPageTag(ada.id, 'math');
  await b.setPageType(b.createPage('Bob').id, 'persona');
  await b.setPageType(b.createPage('Rome').id, 'lugar');
  b.createPage('Notes');
  await b.setTypeIcon('persona', '🧑');
  return b;
}

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
  await app.loadTypeIcons();
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

PageDto byTitle(MemoryBackend b, String t) =>
    b.pages.values.firstWhere((p) => p.title == t);

void main() {
  test('single emoji validation', () {
    expect(isSingleEmoji('🧑'), isTrue);
    expect(isSingleEmoji(' 👨‍👩‍👧 '), isTrue);
    expect(isSingleEmoji('🇪🇸'), isTrue);
    expect(isSingleEmoji('a'), isFalse);
    expect(isSingleEmoji('🧑🧑'), isFalse);
    expect(isSingleEmoji(''), isFalse);
  });

  test(
    'memory backend keeps icons through rename and drops them on delete',
    () async {
      final b = await demo();
      // Like the core: clearing an undefined type's icon defines nothing, names are capped,
      // and renaming a plain tag defines no type.
      await b.setTypeIcon('nueva', null);
      expect(b.typeDefs.containsKey('nueva'), isFalse);
      expect(() => b.renameTag('math', 'x' * 49), throwsException);
      expect(() => b.createType('x' * 49), throwsException);
      await b.renameTag('math', 'idea');
      expect(b.typeDefs.containsKey('math'), isFalse);
      expect(await b.renameType('persona', 'Gente'), 2);
      var cat = await b.typeCatalog();
      expect(cat.firstWhere((t) => t.name == 'Gente').icon, '🧑');
      expect(cat.any((t) => t.name == 'persona'), isFalse);
      expect(await b.removeType('gente'), 2);
      cat = await b.typeCatalog();
      expect(cat.any((t) => t.name.toLowerCase() == 'gente'), isFalse);
      expect(await b.createType('Libro'), 'Libro');
      cat = await b.typeCatalog();
      expect(cat.firstWhere((t) => t.name == 'Libro').count, 0);
    },
  );

  testWidgets('sidebar opens Tags & types, also in a tab', (tester) async {
    final app = await pump(tester, await demo());
    await tester.tap(find.text('Tags & types'));
    await settle(tester);
    expect(app.route, isA<TagsTypesRoute>());
    expect(find.byType(TagsTypesView), findsOneWidget);
    expect(app.tab.title, 'Tags & types');
    expect(
      routeFromJson(routeToJson(const TagsTypesRoute())),
      isA<TagsTypesRoute>(),
    );
  });

  testWidgets('types list emoji, counts and unused suggestions', (
    tester,
  ) async {
    final b = await demo();
    final app = await pump(
      tester,
      b,
      home: const Scaffold(body: TagsTypesView()),
    );
    final persona = find.byKey(const ValueKey('type-row-persona'));
    expect(
      find.descendant(of: persona, matching: find.text('🧑')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: persona, matching: find.text('2 pages')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('type-row-empresa')), findsOneWidget);

    // Pick an emoji for "lugar" from the grid.
    await tester.tap(find.byKey(const ValueKey('type-icon-btn-lugar')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('emoji-Places-📍')));
    await settle(tester);
    expect(app.typeIcon('Lugar'), '📍');

    // A pasted emoji is validated.
    await tester.tap(find.byKey(const ValueKey('type-icon-btn-lugar')));
    await settle(tester);
    await tester.enterText(find.byKey(const ValueKey('emoji-input')), 'xy');
    await tester.tap(find.byKey(const ValueKey('emoji-use')));
    await settle(tester);
    expect(find.text('Enter a single emoji'), findsOneWidget);
    await tester.enterText(find.byKey(const ValueKey('emoji-input')), '🗺️');
    await tester.tap(find.byKey(const ValueKey('emoji-use')));
    await settle(tester);
    expect(app.typeIcon('lugar'), '🗺️');

    // Clearing it.
    await tester.tap(find.byKey(const ValueKey('type-icon-btn-lugar')));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('emoji-clear')));
    await settle(tester);
    expect(app.typeIcon('lugar'), isNull);

    // New type with no pages.
    await tester.tap(find.byKey(const ValueKey('new-type')));
    await settle(tester);
    await tester.enterText(find.byType(TextField).last, 'Libro');
    await tester.tap(find.text('Create'));
    await settle(tester);
    expect(find.byKey(const ValueKey('type-row-Libro')), findsOneWidget);
  });

  testWidgets('renaming a type keeps its emoji; deleting says what is lost', (
    tester,
  ) async {
    final b = await demo();
    final app = await pump(
      tester,
      b,
      home: const Scaffold(body: TagsTypesView()),
    );
    await tester.tap(find.byKey(const ValueKey('type-menu-persona')));
    await settle(tester);
    await tester.tap(find.text('Rename…'));
    await settle(tester);
    await tester.enterText(find.byType(TextField).last, 'Gente');
    await tester.tap(find.text('Rename').last);
    await settle(tester);
    expect(byTitle(b, 'Ada').pageType, 'Gente');
    expect(app.typeIcon('gente'), '🧑');

    await tester.tap(find.byKey(const ValueKey('type-menu-Gente')));
    await settle(tester);
    await tester.tap(find.text('Delete…'));
    await settle(tester);
    expect(find.textContaining('2 pages lose this type'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('tt-confirm')));
    await settle(tester);
    expect(byTitle(b, 'Ada').pageType, isNull);
    expect(byTitle(b, 'Ada').tags, ['math']);
    expect(app.typeIcon('gente'), isNull);
    expect(find.byKey(const ValueKey('type-row-Gente')), findsNothing);
  });

  testWidgets('tags: explicit ones can be renamed, hub tags are locked', (
    tester,
  ) async {
    final b = await demo();
    final hub = b.addHub('Anime');
    b.hubsOfPage[byTitle(b, 'Notes').id] = {hub.id};
    await pump(tester, b, home: const Scaffold(body: TagsTypesView()));
    await tester.tap(find.byKey(const ValueKey('tt-tab-tags')));
    await settle(tester);
    expect(find.byKey(const ValueKey('tag-menu-math')), findsOneWidget);
    expect(find.byKey(const ValueKey('tag-locked-Anime')), findsOneWidget);
    expect(find.byKey(const ValueKey('tag-locked-persona')), findsOneWidget);
    final tip = tester.widget<Tooltip>(
      find.byKey(const ValueKey('tag-locked-Anime')),
    );
    expect(tip.message, contains('hub'));

    await tester.tap(find.byKey(const ValueKey('tag-menu-math')));
    await settle(tester);
    await tester.tap(find.text('Rename…'));
    await settle(tester);
    await tester.enterText(find.byType(TextField).last, 'maths');
    await tester.tap(find.text('Rename').last);
    await settle(tester);
    expect(byTitle(b, 'Ada').tags, ['maths']);
    // Still on the Tags section after the reload.
    expect(find.byKey(const ValueKey('tag-row-maths')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('tag-menu-maths')));
    await settle(tester);
    await tester.tap(find.text('Delete…'));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('tt-confirm')));
    await settle(tester);
    expect(byTitle(b, 'Ada').tags, isEmpty);
  });

  testWidgets('type emoji sits before page names across the app', (
    tester,
  ) async {
    final b = await demo();
    final app = await pump(tester, b, route: const AllPagesRoute());
    // All pages: Ada and Bob are personas.
    expect(
      find.descendant(of: find.byType(AllPagesView), matching: find.text('🧑')),
      findsNWidgets(2),
    );
    // Page header, tab strip and recents.
    app.go(PageTarget(id: byTitle(b, 'Ada').id));
    await settle(tester);
    expect(app.tab.pageType, 'persona');
    expect(app.tabTypeIcon(app.tab), '🧑');
    expect(find.text('🧑'), findsAtLeastNWidgets(3));
    // Tag view lists.
    app.go(const TagRoute('persona'));
    await settle(tester);
    expect(find.text('🧑'), findsAtLeastNWidgets(3));
  });

  testWidgets('quick switcher hits show the emoji', (tester) async {
    final b = await demo();
    await pump(tester, b, home: const Scaffold(body: QuickSwitcher()));
    await tester.enterText(find.byType(TextField), 'Ad');
    await settle(tester);
    expect(find.text('🧑'), findsOneWidget);
  });

  testWidgets('network view draws no emoji; the tree does', (tester) async {
    final b = await demo();
    final app = await pump(
      tester,
      b,
      route: const GraphRoute(GraphMode.network),
    );
    expect(find.byType(GraphView), findsOneWidget);
    expect(find.byKey(const ValueKey('type-icon')), findsNothing);
    expect(find.text('🧑'), findsNothing);
    final data = await b.graphData();
    final ada = data.nodes.firstWhere((n) => n.title == 'Ada');
    expect(ada.title, 'Ada', reason: 'network labels are plain titles');

    final root = buildNamespaceTree(GraphModel(data), typeIcon: app.typeIcon);
    final items = <TreeItem>[];
    void walk(TreeItem t) {
      items.add(t);
      t.children.forEach(walk);
    }

    walk(root);
    expect(items.firstWhere((t) => t.label == 'Ada').icon, '🧑');
    expect(items.firstWhere((t) => t.label == 'Rome').icon, isNull);
    expect(withTypeIcon('🧑', 'Ada'), '🧑 Ada');
  });

  test('graph nodes from the memory backend carry the page type', () async {
    final b = await demo();
    final data = await b.graphData();
    expect(data.nodes.firstWhere((n) => n.title == 'Rome').pageType, 'lugar');
  });

  testWidgets('the tree picks up icons loaded or edited after it was built', (
    tester,
  ) async {
    final b = await demo();
    tester.view.physicalSize = const Size(1600, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    // Cold start: the icon cache is still empty when the tree is built.
    final app = AppState(b)..go(const GraphRoute(GraphMode.tree));
    await tester.pumpWidget(
      AppScope(
        state: app,
        child: MaterialApp(
          theme: buildTheme(Brightness.light),
          home: const Scaffold(body: GraphView(mode: GraphMode.tree)),
        ),
      ),
    );
    await settle(tester);
    TreeItem ada() {
      final items = <TreeItem>[];
      void walk(TreeItem t) {
        items.add(t);
        t.children.forEach(walk);
      }

      walk(tester.state<TreeCanvasState>(find.byType(TreeCanvas)).root);
      return items.firstWhere((t) => t.label == 'Ada');
    }

    expect(ada().icon, isNull);
    await app.loadTypeIcons();
    await settle(tester);
    expect(ada().icon, '🧑');
    await b.setTypeIcon('persona', '👤');
    await app.loadTypeIcons();
    await settle(tester);
    expect(ada().icon, '👤');
  });

  test('reload refreshes the icon cache; failures are logged', () async {
    final b = await demo();
    final app = AppState(b);
    await app.loadTypeIcons();
    expect(app.typeIcon('persona'), '🧑');
    final v = app.iconsVersion;
    await app.loadTypeIcons();
    expect(app.iconsVersion, v, reason: 'unchanged icons do not notify');
    // Changed elsewhere (sync, restore): a reload picks it up.
    await b.setTypeIcon('persona', '👤');
    app.reload();
    await Future<void>.delayed(Duration.zero);
    expect(app.typeIcon('persona'), '👤');
    await AppState(_FailingBackend()).loadTypeIcons();
  });

  testWidgets(
    'renaming a tag from its view moves the type emoji in the cache',
    (tester) async {
      final b = await demo();
      final app = await pump(tester, b, route: const TagRoute('persona'));
      await tester.tap(find.byKey(const ValueKey('tag-rename')));
      await settle(tester);
      await tester.enterText(
        find.byKey(const ValueKey('tag-rename-input')),
        'gente',
      );
      await tester.tap(find.byKey(const ValueKey('tag-rename-confirm')));
      await settle(tester);
      expect(app.typeIcon('gente'), '🧑');
      expect(app.typeIcon('persona'), isNull);
    },
  );
}

class _FailingBackend extends MemoryBackend {
  @override
  Future<List<TagDto>> typeCatalog() async => throw Exception('boom');
}
