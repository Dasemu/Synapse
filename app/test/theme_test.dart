import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/state/settings.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/settings_view.dart';

import 'support/memory_backend.dart';

void main() {
  group('theme CSS', () {
    test('round-trips every built-in theme', () {
      for (final t in appThemes) {
        final p = parseThemeCss(themeToCss(t), fallback: SynapseColors.light);
        expect(p.errors, isEmpty, reason: t.id);
        expect(p.brightness, t.brightness, reason: t.id);
        expect(p.colors, t.colors, reason: t.id);
      }
    });

    test('writes kebab-case names and alpha only when translucent', () {
      final css = themeToCss(appThemes[1]);
      expect(css, contains('/* Synapse theme: Paper dark */'));
      expect(css, contains('color-scheme: dark;'));
      expect(css, contains('--paper: #16181b;'));
      expect(css, contains('--accent-soft: #63c2b822;'));
    });

    test('parses every colour syntax', () {
      expect(parseCssColor('#f80'), const Color(0xFFFF8800));
      expect(parseCssColor('#f808'), const Color(0x88FF8800));
      expect(parseCssColor('#112233'), const Color(0xFF112233));
      expect(parseCssColor('#11223344'), const Color(0x44112233));
      expect(parseCssColor('rgb(255, 0, 128)'), const Color(0xFFFF0080));
      expect(parseCssColor('rgb(255 0 128 / 50%)'), const Color(0x80FF0080));
      expect(parseCssColor('rgba(0, 0, 0, 0.5)'), const Color(0x80000000));
      expect(parseCssColor('hsl(120, 100%, 50%)'), const Color(0xFF00FF00));
      expect(
        parseCssColor('hsla(240, 100%, 50%, 0.2)'),
        const Color(0x330000FF),
      );
      expect(parseCssColor('#12'), isNull);
      expect(parseCssColor('#ggg'), isNull);
      expect(parseCssColor('red'), isNull);
      expect(parseCssColor('rgb(1, 2)'), isNull);
    });

    test('accepts camelCase, ignores selectors and reports bad lines', () {
      final p = parseThemeCss('''
        .foo, :root { /* hi */
          --accentSoft: rgba(255, 0, 0, 0.5);
          --code-bg: #000;
          --nope: #fff;
          --ink: banana;
          color-scheme: dark;
          background: red;
        }
        ''', fallback: SynapseColors.light);
      expect(p.brightness, Brightness.dark);
      expect(p.colors.accentSoft, const Color(0x80FF0000));
      expect(p.colors.codeBg, const Color(0xFF000000));
      expect(p.colors.ink, SynapseColors.light.ink);
      expect(p.errors, hasLength(2));
      expect(p.errors[0], contains('--nope'));
      expect(p.errors[1], contains('--ink'));
      expect(
        parseThemeCss(
          '--paper: #fff;',
          fallback: SynapseColors.dark,
        ).brightness,
        isNull,
      );
    });
  });

  group('custom themes', () {
    test('persist across restarts and skip corrupt entries', () {
      final store = MemorySettings();
      final a = AppState(MemoryBackend(), settings: store);
      final t = a.createCustomTheme(from: 'nord');
      expect(t.label, 'Custom');
      expect(a.createCustomTheme().label, 'Custom 2');
      a.updateCustomTheme(
        t.copyWith(
          label: 'Mine',
          colors: t.colors.copyWith(accent: const Color(0xFFFF0000)),
        ),
      );
      a.setTheme(t.id);

      store.data['customThemes'] = [
        ...(store.data['customThemes'] as List),
        'junk',
        {'id': 'custom-x', 'label': 'x', 'brightness': 'purple', 'colors': {}},
        {'id': 'paper', 'label': 'x', 'brightness': 'dark', 'colors': {}},
        {
          'id': 'custom-partial',
          'label': 'Partial',
          'brightness': 'dark',
          'colors': {'accent': '#00ff00', 'ink': 'bad'},
        },
      ];
      final b = AppState(MemoryBackend(), settings: store);
      expect(b.customThemes.map((t) => t.label), [
        'Mine',
        'Custom 2',
        'Partial',
      ]);
      expect(b.themeId, t.id);
      final mine = b.resolveTheme(t.id)!;
      expect(mine.brightness, Brightness.dark);
      expect(mine.colors.accent, const Color(0xFFFF0000));
      expect(mine.colors.paper, SynapseColors.nord.paper);
      final partial = b.resolveTheme('custom-partial')!;
      expect(partial.colors.accent, const Color(0xFF00FF00));
      expect(partial.colors.ink, SynapseColors.dark.ink);
    });

    test('deleting the selected theme falls back to auto', () {
      final store = MemorySettings();
      final a = AppState(MemoryBackend(), settings: store);
      final t = a.createCustomTheme();
      expect(a.themeId, t.id);
      a.deleteCustomTheme(t.id);
      expect(a.themeId, autoThemeId);
      expect(a.customThemes, isEmpty);
      expect(AppState(MemoryBackend(), settings: store).themeId, autoThemeId);
    });
  });

  testWidgets('settings: new theme is created, selected and editable', (
    tester,
  ) async {
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
            final t = app.resolveTheme(app.themeId);
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
    await tester.tap(find.byKey(const ValueKey('theme-new')));
    await tester.pumpAndSettle();
    expect(app.customThemes, hasLength(1));
    expect(app.themeId, app.customThemes.single.id);
    expect(find.byKey(const ValueKey('theme-preview')), findsOneWidget);

    // Apply CSS from the CSS tab.
    await tester.tap(find.text('CSS'));
    await tester.pump();
    await tester.enterText(
      find.byKey(const ValueKey('theme-css')),
      ':root { color-scheme: dark; --accent: #ff0000; --bogus: #fff; }',
    );
    await tester.tap(find.byKey(const ValueKey('theme-css-apply')));
    await tester.pump();
    expect(find.byKey(const ValueKey('theme-css-errors')), findsOneWidget);
    expect(app.customThemes.single.colors.accent, const Color(0xFFFF0000));
    expect(app.customThemes.single.brightness, Brightness.dark);

    // Cancel discards a new theme and restores the previous selection.
    await tester.tap(find.byKey(const ValueKey('theme-cancel')));
    await tester.pumpAndSettle();
    expect(app.customThemes, isEmpty);
    expect(app.themeId, autoThemeId);
  });
}
