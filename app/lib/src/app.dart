import 'package:flutter/material.dart';

import 'backend/graph_backend.dart';
import 'state/app_state.dart';
import 'state/settings.dart';
import 'theme/theme.dart';
import 'views/shell.dart';

class SynapseApp extends StatefulWidget {
  const SynapseApp({super.key, required this.backend, this.settings});
  final GraphBackend backend;
  final SettingsStore? settings;

  @override
  State<SynapseApp> createState() => _SynapseAppState();
}

class _SynapseAppState extends State<SynapseApp> {
  late final AppState _state = AppState(
    widget.backend,
    settings: widget.settings,
  )..loadRecent();

  @override
  void dispose() {
    _state.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AppScope(
      state: _state,
      child: ListenableBuilder(
        listenable: _state,
        builder: (context, _) {
          final t = themeById(_state.themeId);
          final built = t == null
              ? null
              : buildTheme(t.brightness, colors: t.colors);
          return MaterialApp(
            title: 'Synapse',
            debugShowCheckedModeBanner: false,
            theme: built ?? buildTheme(Brightness.light),
            darkTheme: built ?? buildTheme(Brightness.dark),
            themeMode: t == null
                ? ThemeMode.system
                : (t.brightness == Brightness.dark
                      ? ThemeMode.dark
                      : ThemeMode.light),
            home: const Shell(),
          );
        },
      ),
    );
  }
}
