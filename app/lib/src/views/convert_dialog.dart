import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/pickers.dart';

/// "Convert to hub": title, description, member checklist and what to do with the outline.
/// Pops `true` once the page has been converted.
class ConvertToHubDialog extends StatefulWidget {
  const ConvertToHubDialog({super.key, required this.page});
  final PageDto page;

  @override
  State<ConvertToHubDialog> createState() => _ConvertToHubDialogState();
}

class _ConvertToHubDialogState extends State<ConvertToHubDialog> {
  HubPreviewDto? _preview;
  final _title = TextEditingController();
  final _description = TextEditingController();
  final _noteTitle = TextEditingController();
  final _checked = <String>{};
  bool _keep = false;
  bool _busy = false;
  String? _error;
  Object? _loadError;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _title.dispose();
    _description.dispose();
    _noteTitle.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    final app = AppScope.read(context);
    try {
      final p = await app.backend.hubConversionPreview(widget.page.id);
      if (!mounted) return;
      setState(() {
        _preview = p;
        _title.text = p.suggestedTitle;
        _description.text = p.suggestedDescription;
        _noteTitle.text = p.suggestedNoteTitle;
        _checked.addAll(p.members.map((m) => m.id));
        _keep = p.proseBlocks > 0;
      });
    } catch (e) {
      if (mounted) setState(() => _loadError = e);
    }
  }

  Future<void> _convert() async {
    final p = _preview!;
    final app = AppScope.read(context);
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await app.queue.run(
        () => app.backend.convertToHub(
          widget.page.id,
          _title.text.trim(),
          _description.text.trim(),
          [
            for (final m in p.members)
              if (_checked.contains(m.id)) m.id,
          ],
          keepOutlineAs: _keep ? _noteTitle.text.trim() : null,
        ),
        report: false,
      );
      if (mounted) Navigator.pop(context, true);
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = errorText(e);
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final p = _preview;
    return AlertDialog(
      title: const Text('Convert to hub'),
      content: SizedBox(
        width: 480,
        child: _loadError != null
            ? Text(errorText(_loadError!))
            : p == null
            ? const SizedBox(
                height: 80,
                child: Center(child: CircularProgressIndicator()),
              )
            : SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    TextField(
                      key: const ValueKey('convert-title'),
                      controller: _title,
                      onChanged: (_) => setState(() {}),
                      decoration: const InputDecoration(labelText: 'Title'),
                    ),
                    const SizedBox(height: 12),
                    TextField(
                      key: const ValueKey('convert-description'),
                      controller: _description,
                      minLines: 1,
                      maxLines: 3,
                      decoration: const InputDecoration(
                        labelText: 'Description',
                      ),
                    ),
                    const SizedBox(height: 16),
                    Text(
                      'MEMBERS (${_checked.length}/${p.members.length})',
                      style: Theme.of(context).textTheme.labelSmall,
                    ),
                    if (p.members.isEmpty)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 8),
                        child: Text(
                          'No candidates found. Add notes from the hub afterwards.',
                          style: TextStyle(fontSize: 13, color: colors.muted),
                        ),
                      ),
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxHeight: 220),
                      child: ListView(
                        shrinkWrap: true,
                        children: [
                          for (final m in p.members)
                            CheckboxListTile(
                              key: ValueKey('member-${m.id}'),
                              dense: true,
                              contentPadding: EdgeInsets.zero,
                              controlAffinity: ListTileControlAffinity.leading,
                              value: _checked.contains(m.id),
                              title: Text(m.title),
                              onChanged: (v) => setState(
                                () => v == true
                                    ? _checked.add(m.id)
                                    : _checked.remove(m.id),
                              ),
                            ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 8),
                    CheckboxListTile(
                      key: const ValueKey('keep-outline'),
                      dense: true,
                      contentPadding: EdgeInsets.zero,
                      controlAffinity: ListTileControlAffinity.leading,
                      value: _keep,
                      title: const Text('Keep the original outline in a note'),
                      subtitle: p.blocks == 0
                          ? null
                          : Text(
                              _keep
                                  ? '${p.blocks} blocks move to the note'
                                  : '${p.blocks} blocks will be deleted',
                              style: TextStyle(color: colors.faint),
                            ),
                      onChanged: (v) => setState(() => _keep = v == true),
                    ),
                    if (_keep)
                      TextField(
                        key: const ValueKey('convert-note-title'),
                        controller: _noteTitle,
                        onChanged: (_) => setState(() {}),
                        decoration: InputDecoration(
                          labelText: 'Note title',
                          errorText: _noteTitle.text.trim().isEmpty
                              ? 'A title is required to keep the outline'
                              : null,
                        ),
                      ),
                    if (_error != null)
                      Padding(
                        padding: const EdgeInsets.only(top: 12),
                        child: Text(
                          _error!,
                          key: const ValueKey('convert-error'),
                          style: TextStyle(
                            color: Theme.of(context).colorScheme.error,
                            fontSize: 13,
                          ),
                        ),
                      ),
                  ],
                ),
              ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed:
              p == null ||
                  _busy ||
                  _title.text.trim().isEmpty ||
                  (_keep && _noteTitle.text.trim().isEmpty)
              ? null
              : _convert,
          child: const Text('Convert'),
        ),
      ],
    );
  }
}
