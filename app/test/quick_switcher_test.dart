import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/theme/theme.dart';
import 'package:synapse/src/views/quick_switcher.dart';

import 'support/memory_backend.dart';

void main() {
  testWidgets('the quick switcher finds pages by their content', (
    tester,
  ) async {
    final b = MemoryBackend();
    final recipes = b.createPage('Recipes');
    b.seed(recipes.id, [(0, 'Tortilla de patatas con cebolla')]);
    final soups = b.createPage('Soups');
    b.seed(soups.id, [(0, 'Gazpacho'), (0, 'Salmorejo con cebolla')]);
    final cebolla = b.createPage('Cebolla');
    b.seed(cebolla.id, [(0, 'cebolla morada')]);
    final app = AppState(b);
    await tester.pumpWidget(
      AppScope(
        state: app,
        child: MaterialApp(
          theme: buildTheme(Brightness.light),
          home: const Scaffold(body: QuickSwitcher()),
        ),
      ),
    );
    await tester.enterText(find.byType(TextField), 'cebolla');
    await tester.pumpAndSettle();
    // The title hit first, then one row per other page whose blocks match.
    expect(find.text('IN NOTES'), findsOneWidget);
    expect(find.text('Recipes'), findsOneWidget);
    expect(find.text('Soups'), findsOneWidget);
    expect(find.text('Cebolla'), findsOneWidget);
    await tester.tap(find.text('Soups'));
    await tester.pumpAndSettle();
    expect((app.route as PageTarget).id, soups.id);
  });
}
