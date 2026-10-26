import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/editor/completion.dart';
import 'package:synapse/src/editor/markdown.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/state/keymap.dart';
import 'package:synapse/src/state/settings.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/settings_view.dart';

import 'support/memory_backend.dart';

void main() {
  group('keymap', () {
    test('round-trips combos and refuses duplicates', () {
      final c = Combo(LogicalKeyboardKey.keyB, ctrl: true, shift: true);
      expect(Combo.decode(c.encode()), c);
      expect(c.label, 'Ctrl+Shift+B');
      final k = Keymap();
      expect(k.of(AppAction.bold), Combo(LogicalKeyboardKey.keyB, ctrl: true));
      // Ctrl+K belongs to the quick switcher.
      expect(
        k.bind(AppAction.bold, AppAction.quickSwitcher.defaultCombo),
        AppAction.quickSwitcher,
      );
      expect(k.bind(AppAction.bold, c), isNull);
      expect(k.of(AppAction.bold), c);
      expect(Keymap(k.toJson()).of(AppAction.bold), c);
      k.reset(AppAction.bold);
      expect(k.isCustom(AppAction.bold), isFalse);
    });

    test('default bindings are unique', () {
      final seen = <Combo>{};
      for (final a in AppAction.values) {
        expect(seen.add(a.defaultCombo), isTrue, reason: a.name);
      }
    });

    test('theme and bindings survive a restart', () {
      final store = MemorySettings();
      final a = AppState(MemoryBackend(), settings: store);
      a.setTheme('sweet');
      a.bindKey(AppAction.bold, Combo(LogicalKeyboardKey.keyY, ctrl: true));
      final b = AppState(MemoryBackend(), settings: store);
      expect(b.themeId, 'sweet');
      expect(
        b.keymap.of(AppAction.bold),
        Combo(LogicalKeyboardKey.keyY, ctrl: true),
      );
      store.data['theme'] = 'nonsense';
      expect(AppState(MemoryBackend(), settings: store).themeId, autoThemeId);
    });
  });

  group('slash commands', () {
    test('detects the trigger only at a word start', () {
      expect(detectCompletion('/li', 3)!.kind, CompletionKind.command);
      expect(detectCompletion('say /li', 7)!.query, 'li');
      expect(detectCompletion('see http://x', 12), isNull);
      expect(detectCompletion('a/b', 3), isNull);
    });

    test('/link leaves the caret inside the brackets', () {
      final q = detectCompletion('x /li', 5)!;
      final v = applyCompletion(
        'x /li',
        q,
        const CompletionItem('/link', 'link'),
      );
      expect(v.text, 'x [[]]');
      expect(v.selection.baseOffset, 4);
    });

    test('commands match by name, label and keyword', () {
      expect(matchCommands('li').first.name, 'link');
      expect(matchCommands('strong').map((c) => c.name), contains('bold'));
      expect(matchCommands('zzz'), isEmpty);
    });

    test('wrapSelection wraps, unwraps and pairs at the caret', () {
      const sel = TextSelection(baseOffset: 2, extentOffset: 5);
      final w = wrapSelection(
        const TextEditingValue(text: 'a bcd e', selection: sel),
        '**',
        '**',
      );
      expect(w.text, 'a **bcd** e');
      expect(w.selection.textInside(w.text), 'bcd');
      expect(wrapSelection(w, '**', '**').text, 'a bcd e');
      final p = wrapSelection(
        const TextEditingValue(
          text: 'ab',
          selection: TextSelection.collapsed(offset: 1),
        ),
        '==',
        '==',
      );
      expect(p.text, 'a====b');
      expect(p.selection.baseOffset, 3);
    });
  });

  test('backslash escapes show the character literally', () {
    final r = BlockRenderer(
      RenderContext(
        colors: SynapseColors.light,
        base: const TextStyle(fontSize: 15),
        refs: BlockRefCache(MemoryBackend()),
        assetPath: (_) => null,
        onPage: (_) {},
        onBlockRef: (_) {},
        onUrl: (_) {},
      ),
    ).render(r'\*not italic\* and \[[no link]]');
    final t = r.span.toPlainText(includePlaceholders: true);
    expect(t, '*not italic* and [[no link]]');
    r.dispose();
  });

  testWidgets('settings: pick a theme and rebind a key', (tester) async {
    tester.view.physicalSize = const Size(1200, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final app = AppState(MemoryBackend());
    await tester.pumpWidget(
      AppScope(
        state: app,
        child: ListenableBuilder(
          listenable: app,
          builder: (_, _) {
            final t = themeById(app.themeId);
            return MaterialApp(
              theme: buildTheme(
                t?.brightness ?? Brightness.light,
                colors: t?.colors,
              ),
              home: const Scaffold(body: SettingsView()),
            );
          },
        ),
      ),
    );
    await tester.tap(find.byKey(const ValueKey('theme-sweet')));
    await tester.pump();
    expect(app.themeId, 'sweet');

    final bind = find.byKey(const ValueKey('bind-bold'));
    await tester.ensureVisible(bind);
    await tester.tap(bind);
    await tester.pump();
    expect(find.text('Press keys…'), findsOneWidget);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.keyY);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.keyY);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();
    expect(
      app.keymap.of(AppAction.bold),
      Combo(LogicalKeyboardKey.keyY, ctrl: true),
    );
  });

  test('hubs never enter recents', () {
    final b = MemoryBackend();
    final hub = b.addHub('Fun');
    final note = b.createPage('Note');
    final app = AppState(b);
    app.pageShown(note);
    app.pageShown(hub);
    expect(app.recent.map((p) => p.title), ['Note']);
  });
}
