import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/editor/outline_view.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/state/outline_controller.dart';
import 'package:synapse/src/theme/theme.dart';

import 'support/memory_backend.dart';

class Harness {
  Harness(this.backend, this.app, this.controller, this.pageId);
  final MemoryBackend backend;
  final AppState app;
  final OutlineController controller;
  final String pageId;

  List<(int, String)> get local => [
    for (final n in controller.nodes) (n.depth, n.content),
  ];

  List<(int, String)> get stored => [
    for (final n in backend.outlines[pageId]!) (n.depth, n.content),
  ];
}

Future<Harness> pumpOutline(
  WidgetTester tester,
  List<(int, String)> rows, {
  List<String> otherPages = const [],
}) async {
  final backend = MemoryBackend();
  final page = backend.createPage('Test');
  for (final t in otherPages) {
    backend.createPage(t);
  }
  backend.seed(page.id, rows);
  final app = AppState(backend);
  final view = await backend.page(page.id);
  final controller = OutlineController(
    backend: backend,
    queue: app.queue,
    pageId: page.id,
    blocks: view!.blocks,
    saveDelay: const Duration(milliseconds: 50),
  );
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(
        theme: buildTheme(Brightness.light),
        home: Scaffold(
          body: CustomScrollView(
            slivers: [OutlineSliver(controller: controller)],
          ),
        ),
      ),
    ),
  );
  return Harness(backend, app, controller, page.id);
}

/// Focuses the block with `content` and puts the caret at `caret` (end by default).
Future<void> edit(
  WidgetTester tester,
  Harness h,
  String content, {
  int? caret,
}) async {
  final n = h.controller.nodes.firstWhere((n) => n.content == content);
  h.controller.focus(n.id, caret: caret ?? content.length);
  await tester.pump();
  await tester.pump();
}

TextEditingController field(WidgetTester tester) =>
    tester.widget<TextField>(find.byType(TextField)).controller!;

String? focusedContent(Harness h) {
  final id = h.controller.focusedId;
  return id == null ? null : h.controller.byId(id)?.content;
}

Future<void> settle(WidgetTester tester, Harness h) async {
  await tester.pump(const Duration(milliseconds: 100));
  await h.app.queue.idle;
  await tester.pump(const Duration(seconds: 1));
  await h.app.queue.idle;
  await tester.pump();
}

Future<void> withShift(WidgetTester tester, LogicalKeyboardKey key) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyEvent(key);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  await tester.pump();
}

void main() {
  testWidgets('Enter splits the block at the caret and focuses the new one', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [(0, 'hello world')]);
    await edit(tester, h, 'hello world', caret: 5);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    await tester.pump();

    expect(h.local, [(0, 'hello'), (0, ' world')]);
    expect(focusedContent(h), ' world');
    expect(field(tester).selection.baseOffset, 0);

    await settle(tester, h);
    expect(h.stored, [(0, 'hello'), (0, ' world')]);
    expect(h.controller.nodes.every((n) => !n.id.startsWith('_tmp')), isTrue);
  });

  testWidgets('Enter on a block with children inserts its first child', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [(0, 'parent'), (1, 'child')]);
    await edit(tester, h, 'parent');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester, h);
    expect(h.stored, [(0, 'parent'), (1, ''), (1, 'child')]);
  });

  testWidgets('Shift+Enter inserts a newline inside the block', (tester) async {
    final h = await pumpOutline(tester, [(0, 'a')]);
    await edit(tester, h, 'a');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    // The platform inserts the newline as text input.
    await tester.enterText(find.byType(TextField), 'a\n');
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await settle(tester, h);
    expect(h.stored, [(0, 'a\n')]);
  });

  testWidgets('a newline typed by a soft keyboard splits the block', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [(0, 'ab')]);
    await edit(tester, h, 'ab', caret: 1);
    tester.testTextInput.updateEditingValue(
      const TextEditingValue(
        text: 'a\nb',
        selection: TextSelection.collapsed(offset: 2),
      ),
    );
    await settle(tester, h);
    expect(h.stored, [(0, 'a'), (0, 'b')]);
  });

  testWidgets('Tab indents and Shift+Tab outdents, keeping focus', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [(0, 'a'), (0, 'b'), (0, 'c')]);
    await edit(tester, h, 'b', caret: 1);
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    expect(h.local, [(0, 'a'), (1, 'b'), (0, 'c')]);
    expect(focusedContent(h), 'b');
    expect(field(tester).selection.baseOffset, 1);

    await edit(tester, h, 'c');
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    expect(h.local, [(0, 'a'), (1, 'b'), (1, 'c')]);

    // Outdenting `b` adopts `c` (Logseq's direct outdenting).
    await edit(tester, h, 'b');
    await withShift(tester, LogicalKeyboardKey.tab);
    expect(h.local, [(0, 'a'), (0, 'b'), (1, 'c')]);

    await settle(tester, h);
    expect(h.stored, h.local);
    expect(h.backend.calls.where((c) => c == 'indent').length, 2);
    expect(h.backend.calls.where((c) => c == 'outdent').length, 1);
  });

  testWidgets('Tab on a first child does nothing', (tester) async {
    final h = await pumpOutline(tester, [(0, 'a'), (1, 'b')]);
    await edit(tester, h, 'b');
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await settle(tester, h);
    expect(h.stored, [(0, 'a'), (1, 'b')]);
    expect(h.backend.calls, isNot(contains('indent')));
  });

  testWidgets('Backspace at the start merges into the previous block', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [(0, 'foo'), (0, 'bar'), (1, 'kid')]);
    await edit(tester, h, 'bar', caret: 0);
    await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
    await tester.pump();
    await tester.pump();
    expect(h.local, [(0, 'foobar'), (1, 'kid')]);
    expect(focusedContent(h), 'foobar');
    expect(field(tester).selection.baseOffset, 3);
    await settle(tester, h);
    expect(h.stored, [(0, 'foobar'), (1, 'kid')]);
  });

  testWidgets('Backspace in the middle of text edits normally', (tester) async {
    final h = await pumpOutline(tester, [(0, 'foo'), (0, 'bar')]);
    await edit(tester, h, 'bar', caret: 2);
    await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
    await settle(tester, h);
    expect(h.backend.calls, isNot(contains('merge')));
  });

  testWidgets('Delete at the end pulls the next block in', (tester) async {
    final h = await pumpOutline(tester, [(0, 'foo'), (0, 'bar')]);
    await edit(tester, h, 'foo');
    await tester.sendKeyEvent(LogicalKeyboardKey.delete);
    await settle(tester, h);
    expect(h.stored, [(0, 'foobar')]);
    expect(focusedContent(h), 'foobar');
  });

  testWidgets('arrows move between blocks, skipping collapsed children', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [
      (0, 'one'),
      (1, 'hidden'),
      (0, 'two'),
    ]);
    h.controller.nodes.first.collapsed = true;
    await edit(tester, h, 'one', caret: 1);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    await tester.pump();
    expect(focusedContent(h), 'two');
    expect(field(tester).selection.baseOffset, 1);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    await tester.pump();
    expect(focusedContent(h), 'one');

    // Right at the end moves to the start of the next block.
    field(tester).selection = const TextSelection.collapsed(offset: 3);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    await tester.pump();
    expect(focusedContent(h), 'two');
    expect(field(tester).selection.baseOffset, 0);
    await settle(tester, h);
  });

  testWidgets('Alt+Shift+Up swaps a block with its previous sibling', (
    tester,
  ) async {
    final h = await pumpOutline(tester, [(0, 'a'), (1, 'a1'), (0, 'b')]);
    await edit(tester, h, 'b');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
    await settle(tester, h);
    expect(h.stored, [(0, 'b'), (0, 'a'), (1, 'a1')]);
  });

  testWidgets('Ctrl+Enter cycles TODO and DONE', (tester) async {
    final h = await pumpOutline(tester, [(0, 'buy milk')]);
    await edit(tester, h, 'buy milk');
    Future<void> ctrlEnter() async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pump();
    }

    await ctrlEnter();
    expect(field(tester).text, 'TODO buy milk');
    await ctrlEnter();
    expect(field(tester).text, 'DONE buy milk');
    await ctrlEnter();
    expect(field(tester).text, 'buy milk');
    await settle(tester, h);
    expect(h.stored, [(0, 'buy milk')]);
  });

  testWidgets('[[ autocompletes page titles', (tester) async {
    final h = await pumpOutline(
      tester,
      [(0, '')],
      otherPages: ['Rust', 'Ruby'],
    );
    await edit(tester, h, '');
    final finder = find.byType(TextField);
    // Typing the second bracket auto-pairs the closing ones.
    await tester.enterText(finder, '[');
    await tester.enterText(finder, '[[');
    expect(field(tester).text, '[[]]');
    expect(field(tester).selection.baseOffset, 2);

    tester.testTextInput.updateEditingValue(
      const TextEditingValue(
        text: '[[Rus]]',
        selection: TextSelection.collapsed(offset: 5),
      ),
    );
    await tester.pump();
    await tester.pump();
    expect(find.text('Rust'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(field(tester).text, '[[Rust]]');
    expect(field(tester).selection.baseOffset, 8);
    expect(
      h.controller.nodes,
      hasLength(1),
      reason: 'Enter was taken by the popup',
    );
    await settle(tester, h);
    expect(h.stored, [(0, '[[Rust]]')]);
  });

  testWidgets('Escape leaves editing and the block renders', (tester) async {
    final h = await pumpOutline(tester, [(0, 'some **bold** text')]);
    await edit(tester, h, 'some **bold** text');
    expect(find.byType(TextField), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester, h);
    expect(find.byType(TextField), findsNothing);
    expect(h.controller.focusedId, isNull);
    expect(find.textContaining('bold', findRichText: true), findsOneWidget);
  });

  testWidgets('tapping an empty page creates its first block', (tester) async {
    final h = await pumpOutline(tester, []);
    await tester.tap(find.text('Click to start writing…'));
    await tester.pump();
    await tester.pump();
    expect(find.byType(TextField), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'first');
    await settle(tester, h);
    expect(h.stored, [(0, 'first')]);
  });
}
