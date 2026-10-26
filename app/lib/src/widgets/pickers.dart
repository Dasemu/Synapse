import 'package:flutter/material.dart';

import '../theme/theme.dart';

/// A page offered by [showPagePicker].
class PickItem {
  const PickItem({required this.id, required this.title, this.isHub = false});
  final String id;
  final String title;
  final bool isHub;
}

/// The outcome of [showPagePicker]: an existing page, or a title to create.
class PickResult {
  const PickResult.existing(PickItem this.item)
    : newTitle = null,
      newHub = false;
  const PickResult.create(String this.newTitle, {required this.newHub})
    : item = null;
  final PickItem? item;
  final String? newTitle;
  final bool newHub;
}

/// Search-as-you-type page picker. [search] returns candidates for the query (empty
/// query included). Creation rows are offered for a non-empty query without an exact match.
Future<PickResult?> showPagePicker(
  BuildContext context, {
  required String heading,
  required Future<List<PickItem>> Function(String query) search,
  bool canCreateNote = false,
  bool canCreateHub = false,
}) => showDialog<PickResult>(
  context: context,
  builder: (_) => _PagePicker(
    heading: heading,
    search: search,
    canCreateNote: canCreateNote,
    canCreateHub: canCreateHub,
  ),
);

class _PagePicker extends StatefulWidget {
  const _PagePicker({
    required this.heading,
    required this.search,
    required this.canCreateNote,
    required this.canCreateHub,
  });
  final String heading;
  final Future<List<PickItem>> Function(String) search;
  final bool canCreateNote;
  final bool canCreateHub;

  @override
  State<_PagePicker> createState() => _PagePickerState();
}

class _PagePickerState extends State<_PagePicker> {
  final _text = TextEditingController();
  List<PickItem> _items = const [];
  int _seq = 0;

  @override
  void initState() {
    super.initState();
    _query('');
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _query(String q) async {
    final n = ++_seq;
    final r = await widget.search(q.trim());
    if (mounted && n == _seq) setState(() => _items = r);
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final q = _text.text.trim();
    final exact = _items.any((i) => i.title.toLowerCase() == q.toLowerCase());
    final create = q.isNotEmpty && !exact;
    Widget row(IconData icon, String label, VoidCallback onTap, {Key? key}) =>
        ListTile(
          key: key,
          dense: true,
          leading: Icon(icon, size: 18, color: colors.accent),
          title: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
          onTap: onTap,
        );
    return AlertDialog(
      title: Text(widget.heading),
      content: SizedBox(
        width: 420,
        height: 380,
        child: Column(
          children: [
            TextField(
              controller: _text,
              autofocus: true,
              decoration: const InputDecoration(hintText: 'Search pages'),
              onChanged: _query,
            ),
            const SizedBox(height: 8),
            Expanded(
              child: ListView(
                children: [
                  if (create && widget.canCreateNote)
                    row(
                      Icons.add_rounded,
                      'Create note "$q"',
                      () => Navigator.pop(
                        context,
                        PickResult.create(q, newHub: false),
                      ),
                    ),
                  if (create && widget.canCreateHub)
                    row(
                      Icons.create_new_folder_outlined,
                      'Create hub "$q"',
                      () => Navigator.pop(
                        context,
                        PickResult.create(q, newHub: true),
                      ),
                    ),
                  for (final i in _items)
                    row(
                      i.isHub
                          ? Icons.folder_outlined
                          : Icons.description_outlined,
                      i.title,
                      () => Navigator.pop(context, PickResult.existing(i)),
                      key: ValueKey('pick-${i.id}'),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

/// Asks for a name. [submit] runs on confirm; when it throws, the message is shown inline
/// and the dialog stays open. Returns what [submit] returned.
Future<T?> showNameDialog<T>(
  BuildContext context, {
  required String heading,
  required String confirm,
  required Future<T> Function(String name) submit,
  String hint = 'Title',
  String initial = '',
}) => showDialog<T>(
  context: context,
  builder: (_) => _NameDialog<T>(
    heading: heading,
    confirm: confirm,
    submit: submit,
    hint: hint,
    initial: initial,
  ),
);

class _NameDialog<T> extends StatefulWidget {
  const _NameDialog({
    required this.heading,
    required this.confirm,
    required this.submit,
    required this.hint,
    this.initial = '',
  });
  final String heading;
  final String initial;
  final String confirm;
  final String hint;
  final Future<T> Function(String) submit;

  @override
  State<_NameDialog<T>> createState() => _NameDialogState<T>();
}

class _NameDialogState<T> extends State<_NameDialog<T>> {
  late final _text = TextEditingController(text: widget.initial);
  String? _error;
  bool _busy = false;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _go() async {
    final name = _text.text.trim();
    if (name.isEmpty || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final r = await widget.submit(name);
      if (mounted) Navigator.pop(context, r);
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
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.heading),
    content: SizedBox(
      width: 380,
      child: TextField(
        controller: _text,
        autofocus: true,
        decoration: InputDecoration(hintText: widget.hint, errorText: _error),
        onSubmitted: (_) => _go(),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(onPressed: _go, child: Text(widget.confirm)),
    ],
  );
}

/// A readable message for an exception coming back from the core.
String errorText(Object e) {
  var s = '$e';
  for (final p in ['Exception: ', 'AnyhowException(', 'PanicException(']) {
    if (s.startsWith(p)) s = s.substring(p.length);
  }
  if (s.endsWith(')') && !s.contains('(')) s = s.substring(0, s.length - 1);
  return s.trim();
}
