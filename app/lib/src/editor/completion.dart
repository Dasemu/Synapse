import 'package:flutter/services.dart';

enum CompletionKind { page, tag, block }

/// An autocomplete trigger under the caret: `[[query`, `#query` or `((query`.
class CompletionQuery {
  const CompletionQuery(this.kind, this.start, this.query);

  final CompletionKind kind;

  /// Index where the query text starts (right after the trigger).
  final int start;
  final String query;

  @override
  bool operator ==(Object other) =>
      other is CompletionQuery &&
      other.kind == kind &&
      other.start == start &&
      other.query == query;

  @override
  int get hashCode => Object.hash(kind, start, query);
}

/// One suggestion. `value` is the page title or the block id to insert.
class CompletionItem {
  const CompletionItem(
    this.label,
    this.value, {
    this.detail,
    this.isNew = false,
  });
  final String label;
  final String value;
  final String? detail;
  final bool isNew;
}

bool _ws(String c) => c.trim().isEmpty;

/// Finds the autocomplete trigger that the caret is in, if any.
CompletionQuery? detectCompletion(String text, int caret) {
  if (caret < 0 || caret > text.length) return null;
  final lineStart = text.lastIndexOf('\n', caret == 0 ? 0 : caret - 1) + 1;
  final before = text.substring(lineStart, caret);
  CompletionQuery? best;
  void consider(CompletionQuery q) {
    if (best == null || q.start > best!.start) best = q;
  }

  final link = before.lastIndexOf('[[');
  if (link >= 0 && !before.substring(link + 2).contains(']]')) {
    consider(
      CompletionQuery(
        CompletionKind.page,
        lineStart + link + 2,
        before.substring(link + 2),
      ),
    );
  }
  final ref = before.lastIndexOf('((');
  if (ref >= 0 && !before.substring(ref + 2).contains('))')) {
    consider(
      CompletionQuery(
        CompletionKind.block,
        lineStart + ref + 2,
        before.substring(ref + 2),
      ),
    );
  }
  final hash = before.lastIndexOf('#');
  if (hash >= 0 && (hash == 0 || _ws(before[hash - 1]))) {
    final q = before.substring(hash + 1);
    if (!q.startsWith('[') &&
        !q.contains(RegExp(r'\s')) &&
        !q.startsWith('#')) {
      // A bare `#` followed by a space is a heading, not a tag.
      consider(CompletionQuery(CompletionKind.tag, lineStart + hash + 1, q));
    }
  }
  return best;
}

/// Text after accepting `item` for `q` with the caret at `caret`.
TextEditingValue applyCompletion(
  String text,
  int caret,
  CompletionQuery q,
  CompletionItem item,
) {
  final head = text.substring(0, q.start);
  var tail = text.substring(caret);
  switch (q.kind) {
    case CompletionKind.page:
      if (tail.startsWith(']]')) tail = tail.substring(2);
      final t = '$head${item.value}]]';
      return TextEditingValue(
        text: t + tail,
        selection: TextSelection.collapsed(offset: t.length),
      );
    case CompletionKind.block:
      if (tail.startsWith('))')) tail = tail.substring(2);
      final t = '$head${item.value}))';
      return TextEditingValue(
        text: t + tail,
        selection: TextSelection.collapsed(offset: t.length),
      );
    case CompletionKind.tag:
      final v = item.value.contains(RegExp(r'\s'))
          ? '[[${item.value}]]'
          : item.value;
      final sep = tail.startsWith(' ') ? '' : ' ';
      final t = '$head$v$sep';
      return TextEditingValue(
        text: t + tail,
        selection: TextSelection.collapsed(offset: t.length),
      );
  }
}

/// Auto-pairs `[[` → `[[]]` and `((` → `(())` right after the second bracket is typed.
TextEditingValue? autoPair(TextEditingValue old, TextEditingValue now) {
  if (now.text.length != old.text.length + 1 || !now.selection.isCollapsed) {
    return null;
  }
  final c = now.selection.baseOffset;
  if (c < 2) return null;
  final typed = now.text[c - 1];
  final close = switch (typed) {
    '[' => ']]',
    '(' => '))',
    _ => null,
  };
  if (close == null || now.text[c - 2] != typed) return null;
  if (now.text.substring(c).startsWith(close[0])) return null;
  return TextEditingValue(
    text: now.text.substring(0, c) + close + now.text.substring(c),
    selection: TextSelection.collapsed(offset: c),
  );
}
