import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../state/modules.dart';
import '../theme/theme.dart';

/// Picks a module file; replaced in tests.
Future<Uint8List?> Function() pickModuleFile = () async {
  final file = await openFile(
    acceptedTypeGroups: const [
      XTypeGroup(label: 'Synapse module', extensions: ['wasm']),
    ],
  );
  return file?.readAsBytes();
};

/// Loads a bundled module; replaced in tests.
Future<Uint8List> Function(String asset) loadBundledModule = (asset) async =>
    (await rootBundle.load(asset)).buffer.asUint8List();

/// Settings section: installed modules (enable, remove), bundled ones and install from file.
class ModulesSettings extends StatelessWidget {
  const ModulesSettings({super.key});

  @override
  Widget build(BuildContext context) {
    final modules = AppScope.read(context).modules;
    final colors = context.colors;
    return ListenableBuilder(
      listenable: modules,
      builder: (context, _) {
        final installed = modules.installed;
        final available = bundledModules.entries
            .where((e) => installed.every((m) => m.id != e.key))
            .toList();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    'Modules',
                    style: Theme.of(context).textTheme.headlineSmall,
                  ),
                ),
                TextButton.icon(
                  key: const ValueKey('module-install-file'),
                  onPressed: () => _installFromFile(context),
                  icon: const Icon(Icons.upload_file_outlined, size: 18),
                  label: const Text('Install from file…'),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              'Modules extend Synapse. Each one runs in a sandbox and can only do what you '
              'allow when you install it. Installed modules are per device; their data '
              'syncs with the graph.',
              style: TextStyle(color: colors.muted),
            ),
            const SizedBox(height: 14),
            for (final m in installed) _InstalledModule(module: m),
            for (final e in available)
              _BundledModule(
                id: e.key,
                asset: e.value.asset,
                name: e.value.name,
                description: e.value.description,
              ),
            if (installed.isEmpty && available.isEmpty)
              Text(
                'No modules installed.',
                style: TextStyle(color: colors.muted),
              ),
          ],
        );
      },
    );
  }

  static Future<void> _installFromFile(BuildContext context) async {
    final messenger = ScaffoldMessenger.of(context);
    final Uint8List? bytes;
    try {
      bytes = await pickModuleFile();
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text('Could not read the file: $e')),
      );
      return;
    }
    if (bytes == null || !context.mounted) return;
    await installModuleBytes(context, bytes);
  }
}

/// Reads the module, asks for consent to its permissions and installs it.
Future<void> installModuleBytes(BuildContext context, Uint8List bytes) async {
  final modules = AppScope.read(context).modules;
  final messenger = ScaffoldMessenger.of(context);
  final ModuleDto m;
  try {
    m = await modules.inspect(bytes);
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text('Not a valid Synapse module: ${_message(e)}')),
    );
    return;
  }
  if (!context.mounted) return;
  final previous = modules.byId(m.id);
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => _ConsentDialog(module: m, previous: previous),
  );
  if (ok != true) return;
  try {
    await modules.install(bytes, m.permissions);
    messenger.showSnackBar(
      SnackBar(content: Text('${m.name} ${m.version} installed.')),
    );
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text('Could not install ${m.name}: ${_message(e)}')),
    );
  }
}

String _message(Object e) {
  final s = '$e';
  return s.startsWith('Exception: ') ? s.substring(11) : s;
}

class _ConsentDialog extends StatelessWidget {
  const _ConsentDialog({required this.module, this.previous});
  final ModuleDto module;
  final ModuleDto? previous;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final m = module;
    final update = previous != null;
    return AlertDialog(
      title: Text(
        update
            ? 'Update ${m.name} (${previous!.version} → ${m.version})?'
            : 'Install ${m.name} ${m.version}?',
      ),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (m.author.isNotEmpty)
                Text('By ${m.author}', style: TextStyle(color: colors.muted)),
              if (m.description.isNotEmpty) ...[
                const SizedBox(height: 8),
                Text(m.description),
              ],
              const SizedBox(height: 16),
              Text(
                m.permissions.isEmpty
                    ? 'It asks for no permissions.'
                    : 'It will be allowed to:',
                style: const TextStyle(fontWeight: FontWeight.w600),
              ),
              for (final p in m.permissions)
                Padding(
                  padding: const EdgeInsets.only(top: 6),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(
                        Icons.check_circle_outline,
                        size: 16,
                        color: colors.accent,
                      ),
                      const SizedBox(width: 8),
                      Expanded(child: Text(permissionDescriptions[p] ?? p)),
                    ],
                  ),
                ),
              if (m.views.isNotEmpty || m.macros.isNotEmpty) ...[
                const SizedBox(height: 16),
                Text(
                  [
                    if (m.views.isNotEmpty)
                      'Adds ${m.views.map((v) => v.title).join(', ')} to the sidebar.',
                    if (m.macros.isNotEmpty)
                      'Renders ${m.macros.map((n) => '{{$n …}}').join(', ')} in blocks.',
                  ].join(' '),
                  style: TextStyle(color: colors.muted),
                ),
              ],
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const ValueKey('module-consent-install'),
          onPressed: () => Navigator.pop(context, true),
          child: Text(update ? 'Update' : 'Install'),
        ),
      ],
    );
  }
}

class _InstalledModule extends StatelessWidget {
  const _InstalledModule({required this.module});
  final ModuleDto module;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final m = module;
    final modules = AppScope.read(context).modules;
    final icon = m.views.isEmpty ? null : m.views.first.icon;
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.fromLTRB(14, 10, 8, 10),
      decoration: BoxDecoration(
        border: Border.all(color: colors.line),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2, right: 12),
            child: icon == null
                ? Icon(Icons.extension_outlined, color: colors.muted)
                : Text(icon, style: const TextStyle(fontSize: 20)),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${m.name}  ${m.version}',
                  style: TextStyle(
                    fontWeight: FontWeight.w600,
                    color: colors.ink,
                  ),
                ),
                if (m.description.isNotEmpty)
                  Text(m.description, style: TextStyle(color: colors.muted)),
                const SizedBox(height: 6),
                Wrap(
                  spacing: 6,
                  runSpacing: 4,
                  children: [
                    for (final p in m.permissions)
                      Tooltip(
                        message: permissionDescriptions[p] ?? p,
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 7,
                            vertical: 2,
                          ),
                          decoration: BoxDecoration(
                            color: colors.codeBg,
                            borderRadius: BorderRadius.circular(5),
                          ),
                          child: Text(
                            p,
                            style: TextStyle(fontSize: 12, color: colors.muted),
                          ),
                        ),
                      ),
                  ],
                ),
              ],
            ),
          ),
          Column(
            children: [
              Switch(
                key: ValueKey('module-enabled-${m.id}'),
                value: m.enabled,
                onChanged: (v) => modules.setEnabled(m.id, v),
              ),
              TextButton(
                key: ValueKey('module-remove-${m.id}'),
                onPressed: () => _remove(context),
                child: const Text('Remove'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Future<void> _remove(BuildContext context) async {
    final modules = AppScope.read(context).modules;
    var deleteData = false;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setState) => AlertDialog(
          title: Text('Remove ${module.name}?'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                'It is removed from this device. Its data stays in the graph, so '
                'installing it again brings it back.',
              ),
              const SizedBox(height: 8),
              CheckboxListTile(
                key: const ValueKey('module-remove-data'),
                contentPadding: EdgeInsets.zero,
                value: deleteData,
                onChanged: (v) => setState(() => deleteData = v ?? false),
                title: const Text(
                  'Also delete its data on every synced device (cannot be undone)',
                ),
              ),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              key: const ValueKey('module-remove-confirm'),
              onPressed: () => Navigator.pop(ctx, true),
              child: const Text('Remove'),
            ),
          ],
        ),
      ),
    );
    if (ok == true) {
      await modules.uninstall(module.id, deleteData: deleteData);
    }
  }
}

class _BundledModule extends StatelessWidget {
  const _BundledModule({
    required this.id,
    required this.asset,
    required this.name,
    required this.description,
  });
  final String id;
  final String asset;
  final String name;
  final String description;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.fromLTRB(14, 10, 8, 10),
      decoration: BoxDecoration(
        border: Border.all(color: colors.line),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        children: [
          Padding(
            padding: const EdgeInsets.only(right: 12),
            child: Icon(Icons.extension_outlined, color: colors.muted),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  name,
                  style: TextStyle(
                    fontWeight: FontWeight.w600,
                    color: colors.ink,
                  ),
                ),
                Text(
                  '$description Included with Synapse.',
                  style: TextStyle(color: colors.muted),
                ),
              ],
            ),
          ),
          FilledButton.tonal(
            key: ValueKey('module-install-$id'),
            onPressed: () async {
              final bytes = await loadBundledModule(asset);
              if (context.mounted) await installModuleBytes(context, bytes);
            },
            child: const Text('Install'),
          ),
        ],
      ),
    );
  }
}
