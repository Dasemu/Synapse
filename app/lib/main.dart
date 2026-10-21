import 'dart:io';

import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:url_launcher/url_launcher.dart';

import 'src/app.dart';
import 'src/backend/graph_backend.dart';
import 'src/editor/block_row.dart' as editor;
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  editor.openUrl = (url) {
    final uri = Uri.tryParse(url);
    if (uri != null) launchUrl(uri, mode: LaunchMode.externalApplication);
  };
  final dir =
      Platform.environment['SYNAPSE_GRAPH'] ??
      '${(await getApplicationSupportDirectory()).path}${Platform.pathSeparator}graph';
  final backend = await FfiGraphBackend.open(dir);
  runApp(SynapseApp(backend: backend));
}
