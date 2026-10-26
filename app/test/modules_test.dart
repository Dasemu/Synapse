import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/module_view.dart';
import 'package:synapse/src/views/modules_settings.dart';
import 'package:synapse/src/views/shell.dart';
import 'package:synapse/src/widgets/module_ui.dart';

import 'support/memory_backend.dart';

ModuleDto vault({int? tick}) => ModuleDto(
  id: 'synapse.secrets',
  name: 'Secrets',
  version: '0.1.0',
  description: 'An encrypted vault.',
  author: 'Synapse',
  permissions: const ['storage', 'crypto', 'clipboard'],
  views: const [ModuleViewDto(id: 'vault', title: 'Secrets', icon: '🔑')],
  macros: const ['secret'],
  tickSeconds: tick,
  enabled: true,
  size: BigInt.from(1000),
);

Future<AppState> pump(
  WidgetTester tester,
  MemoryBackend b, {
  AppRoute? route,
  double height = 1000,
}) async {
  tester.view.physicalSize = Size(1600, height);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final app = AppState(b);
  await app.modules.load();
  if (route != null) app.go(route);
  await tester.pumpWidget(
    AppScope(
      state: app,
      child: MaterialApp(
        theme: buildTheme(Brightness.light),
        home: const Shell(),
      ),
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

/// Captures what the app puts on the clipboard.
List<String?> mockClipboard(WidgetTester tester) {
  final writes = <String?>[];
  String? current;
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'Clipboard.setData') {
        current = (call.arguments as Map)['text'] as String?;
        writes.add(current);
      }
      if (call.method == 'Clipboard.getData') return {'text': current};
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
  return writes;
}

void main() {
  test('module routes survive a restart', () {
    const r = ModuleRoute('synapse.secrets', 'vault', title: 'Secrets');
    final back = routeFromJson(routeToJson(r));
    expect(back, r);
    expect((back! as ModuleRoute).title, 'Secrets');
    expect(routeFromJson({'t': 'module', 'id': 'x'}), isNull);
  });

  testWidgets('installing a bundled module asks for its permissions first', (
    tester,
  ) async {
    final b = MemoryBackend()..inspectResult = vault();
    loadBundledModule = (_) async => Uint8List(4);
    await pump(tester, b, route: const SettingsRoute(), height: 3000);

    await tester.ensureVisible(
      find.byKey(const ValueKey('module-install-synapse.secrets')),
    );
    await tester.tap(
      find.byKey(const ValueKey('module-install-synapse.secrets')),
    );
    await settle(tester);
    expect(find.text('Install Secrets 0.1.0?'), findsOneWidget);
    expect(find.textContaining('Derive keys from passwords'), findsOneWidget);
    // Cancel installs nothing.
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(b.installedModules, isEmpty);

    await tester.tap(
      find.byKey(const ValueKey('module-install-synapse.secrets')),
    );
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('module-consent-install')));
    await settle(tester);
    expect(b.installedModules.keys, ['synapse.secrets']);
    // Its view is in the sidebar.
    expect(
      find.widgetWithText(InkWell, 'Secrets').evaluate().isNotEmpty ||
          find.text('Secrets').evaluate().isNotEmpty,
      isTrue,
    );

    // The "installed" notice would cover the buttons.
    tester
        .state<ScaffoldMessengerState>(find.byType(ScaffoldMessenger))
        .removeCurrentSnackBar();
    await settle(tester);
    // Disabling hides it; removing keeps data unless asked.
    await tester.ensureVisible(
      find.byKey(const ValueKey('module-enabled-synapse.secrets')),
    );
    await tester.tap(
      find.byKey(const ValueKey('module-enabled-synapse.secrets')),
    );
    await settle(tester);
    expect(b.installedModules['synapse.secrets']!.enabled, isFalse);
    await tester.tap(
      find.byKey(const ValueKey('module-remove-synapse.secrets')),
    );
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('module-remove-confirm')));
    await settle(tester);
    expect(b.installedModules, isEmpty);
  });

  testWidgets(
    'a module view renders its tree, sends field values and applies effects',
    (tester) async {
      final writes = mockClipboard(tester);
      final b = MemoryBackend();
      b.installedModules['synapse.secrets'] = vault();
      var unlocked = false;
      b.moduleHandlers['synapse.secrets'] = (req) {
        if (req['op'] == 'event' && req['action'] == 'unlock') {
          unlocked = (req['values'] as Map)['password'] == 'correct horse';
          if (!unlocked) {
            return {
              'ui': {
                'type': 'column',
                'children': [
                  {
                    'type': 'field',
                    'id': 'password',
                    'label': 'Master password',
                    'value': '',
                    'secret': true,
                    'submit': 'unlock',
                  },
                  {'type': 'text', 'text': 'Wrong password.', 'style': 'error'},
                  {
                    'type': 'button',
                    'label': 'Unlock',
                    'action': 'unlock',
                    'style': 'primary',
                  },
                ],
              },
            };
          }
        }
        if (req['op'] == 'event' && req['action'] == 'copy_password') {
          return {
            'ui': {'type': 'text', 'text': 'GitHub'},
            'effects': [
              {
                'copy': {'text': 'hunter2', 'clear_after_ms': 1000},
              },
              {'toast': 'Password copied.'},
            ],
          };
        }
        if (unlocked) {
          return {
            'ui': {
              'type': 'column',
              'children': [
                {'type': 'heading', 'text': 'GitHub'},
                {
                  'type': 'button',
                  'label': 'Copy',
                  'action': 'copy_password',
                  'arg': 'id1',
                },
              ],
            },
          };
        }
        return {
          'ui': {
            'type': 'column',
            'children': [
              {
                'type': 'field',
                'id': 'password',
                'label': 'Master password',
                'value': '',
                'secret': true,
                'submit': 'unlock',
              },
              {
                'type': 'button',
                'label': 'Unlock',
                'action': 'unlock',
                'style': 'primary',
              },
            ],
          },
        };
      };
      await pump(
        tester,
        b,
        route: const ModuleRoute('synapse.secrets', 'vault', title: 'Secrets'),
      );
      expect(find.byType(ModuleView), findsOneWidget);
      final field = find.widgetWithText(TextField, 'Master password');
      expect(tester.widget<TextField>(field).obscureText, isTrue);

      await tester.enterText(field, 'wrong');
      await tester.tap(find.text('Unlock'));
      await settle(tester);
      expect(find.text('Wrong password.'), findsOneWidget);
      expect((b.moduleRequests.last.$2['values'] as Map)['password'], 'wrong');

      await tester.enterText(
        find.widgetWithText(TextField, 'Master password'),
        'correct horse',
      );
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await settle(tester);
      expect(find.text('GitHub'), findsOneWidget);

      await tester.tap(find.text('Copy'));
      await settle(tester);
      expect(writes, ['hunter2']);
      expect(find.text('Password copied.'), findsOneWidget);
      // The clipboard is cleared after the delay (only if it still holds our text).
      await tester.pump(const Duration(seconds: 2));
      expect(writes, ['hunter2', '']);
    },
  );

  testWidgets('blocks render module macros as chips with actions', (
    tester,
  ) async {
    final writes = mockClipboard(tester);
    final b = MemoryBackend();
    b.installedModules['synapse.secrets'] = vault();
    b.moduleHandlers['synapse.secrets'] = (req) => switch (req['op']) {
      'macro' => {
        'inline': {
          'text': req['args'],
          'icon': '🔑',
          'masked': '••••••••',
          'actions': [
            {'id': 'copy', 'label': 'Copy password', 'icon': 'copy'},
          ],
        },
      },
      'macro_action' => {
        'inline': {'text': req['args'], 'masked': '••••••••'},
        'effects': [
          {
            'copy': {'text': 's3cret'},
          },
        ],
      },
      _ => {},
    };
    final p = b.createPage('Accounts');
    b.seed(p.id, [(0, 'GitHub: {{secret GitHub}} and {{embed nothing}}')]);
    await pump(tester, b, route: PageTarget(id: p.id));

    expect(find.byType(ModuleMacro), findsOneWidget);
    expect(find.text('••••••••'), findsOneWidget);
    expect(
      find.textContaining('{{embed nothing}}', findRichText: true),
      findsOneWidget,
    );
    final req = b.moduleRequests.firstWhere((r) => r.$2['op'] == 'macro').$2;
    expect(req['name'], 'secret');
    expect(req['args'], 'GitHub');

    await tester.tap(find.byTooltip('Copy password'));
    await settle(tester);
    expect(writes, ['s3cret']);
    expect(b.moduleRequests.last.$2['action'], 'copy');
  });

  testWidgets('without the module, macros stay text', (tester) async {
    final b = MemoryBackend();
    final p = b.createPage('Accounts');
    b.seed(p.id, [(0, 'GitHub: {{secret GitHub}}')]);
    await pump(tester, b, route: PageTarget(id: p.id));
    expect(find.byType(ModuleMacro), findsNothing);
    expect(
      find.textContaining('{{secret GitHub}}', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets('suspending the app locks modules and redraws them', (
    tester,
  ) async {
    final b = MemoryBackend();
    b.installedModules['synapse.secrets'] = vault();
    b.moduleHandlers['synapse.secrets'] = (req) =>
        req['op'] == 'suspend' ? {'changed': true} : {};
    final app = await pump(tester, b);
    final before = app.modules.macroVersion;
    await app.modules.suspend();
    expect(b.moduleRequests.last.$2['op'], 'suspend');
    expect(app.modules.macroVersion, greaterThan(before));
  });
}
