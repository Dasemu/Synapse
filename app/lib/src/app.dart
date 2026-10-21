import 'package:flutter/material.dart';

import 'backend/graph_backend.dart';
import 'state/app_state.dart';
import 'theme/theme.dart';
import 'views/shell.dart';

class SynapseApp extends StatefulWidget {
  const SynapseApp({super.key, required this.backend});
  final GraphBackend backend;

  @override
  State<SynapseApp> createState() => _SynapseAppState();
}

class _SynapseAppState extends State<SynapseApp> {
  late final AppState _state = AppState(widget.backend)..loadRecent();

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
        builder: (context, _) => MaterialApp(
          title: 'Synapse',
          debugShowCheckedModeBanner: false,
          theme: buildTheme(Brightness.light),
          darkTheme: buildTheme(Brightness.dark),
          themeMode: _state.themeMode,
          home: const Shell(),
        ),
      ),
    );
  }
}
