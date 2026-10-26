import 'package:flutter/services.dart';

enum CompletionKind { page, tag, block, command }

/// An autocomplete trigger under the caret: `[[query`, `#query`, `((query` or `/query`.
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

/// A `/command`. `insert` is the text it produces; `|` marks where the caret lands (end of
/// the text when absent). `then` re-opens autocomplete afterwards (the link commands).
class SlashCommand {
  const SlashCommand(
    this.name,
    this.label,
    this.insert, {
    this.keywords = '',
    this.then = false,
  });
  final String name;
  final String label;
  final String insert;
  final String keywords;
  final bool then;

  /// Today's date as a journal title, for `/today`.
  static String today() {
    final d = DateTime.now();
    String two(int n) => n.toString().padLeft(2, '0');
    return '${d.year}-${two(d.month)}-${two(d.day)}';
  }

  String text() => insert.replaceAll('{today}', today());
}

const slashCommands = [
  SlashCommand(
    'link',
    'Link to a page',
    '[[|]]',
    keywords: 'page wiki',
    then: true,
  ),
  SlashCommand(
    'ref',
    'Reference a block',
    '((|))',
    keywords: 'block',
    then: true,
  ),
  SlashCommand('url', 'Web link', '[|](https://)', keywords: 'http markdown'),
  SlashCommand('image', 'Image', '![|](https://)', keywords: 'picture img'),
  SlashCommand('bold', 'Bold', '**|**', keywords: 'strong'),
  SlashCommand('italic', 'Italic', '*|*', keywords: 'emphasis'),
  SlashCommand('strike', 'Strikethrough', '~~|~~', keywords: 'delete'),
  SlashCommand('highlight', 'Highlight', '==|==', keywords: 'mark'),
  SlashCommand('code', 'Inline code', '`|`', keywords: 'mono'),
  SlashCommand('codeblock', 'Code block', '```\n|\n```', keywords: 'fence'),
  SlashCommand('h1', 'Heading 1', '# |', keywords: 'title'),
  SlashCommand('h2', 'Heading 2', '## |', keywords: 'title'),
  SlashCommand('h3', 'Heading 3', '### |', keywords: 'title'),
  SlashCommand('todo', 'Task', 'TODO |', keywords: 'task checkbox'),
  SlashCommand('quote', 'Quote', '> |', keywords: 'blockquote'),
  SlashCommand('divider', 'Divider', '---', keywords: 'rule line hr'),
  SlashCommand(
    'today',
    "Link to today's journal",
    '[[{today}]]',
    keywords: 'date',
  ),
  SlashCommand('tag', 'Tag', '#|', keywords: 'hashtag', then: true),
];

/// Commands matching `query` (name prefix first, then name/label/keyword substrings).
List<SlashCommand> matchCommands(String query, {int limit = 8}) {
  final q = query.toLowerCase();
  final starts = <SlashCommand>[];
  final rest = <SlashCommand>[];
  for (final c in slashCommands) {
    if (c.name.startsWith(q)) {
      starts.add(c);
    } else if ('${c.label} ${c.keywords}'.toLowerCase().contains(q)) {
      rest.add(c);
    }
  }
  return [...starts, ...rest].take(limit).toList();
}

/// Wraps the selection in `open`/`close` (or inserts the pair with the caret between). If the
/// selection is already wrapped, unwraps it.
TextEditingValue wrapSelection(TextEditingValue v, String open, String close) {
  final sel = v.selection;
  if (!sel.isValid) return v;
  final t = v.text;
  final inner = t.substring(sel.start, sel.end);
  if (!sel.isCollapsed &&
      sel.start >= open.length &&
      sel.end + close.length <= t.length &&
      t.substring(sel.start - open.length, sel.start) == open &&
      t.substring(sel.end, sel.end + close.length) == close) {
    return TextEditingValue(
      text:
          t.substring(0, sel.start - open.length) +
          inner +
          t.substring(sel.end + close.length),
      selection: TextSelection(
        baseOffset: sel.start - open.length,
        extentOffset: sel.end - open.length,
      ),
    );
  }
  final text =
      t.substring(0, sel.start) + open + inner + close + t.substring(sel.end);
  return TextEditingValue(
    text: text,
    selection: sel.isCollapsed
        ? TextSelection.collapsed(offset: sel.start + open.length)
        : TextSelection(
            baseOffset: sel.start + open.length,
            extentOffset: sel.end + open.length,
          ),
  );
}

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
  final slash = before.lastIndexOf('/');
  if (slash >= 0 && (slash == 0 || _ws(before[slash - 1]))) {
    final q = before.substring(slash + 1);
    if (!q.contains(RegExp(r'\s'))) {
      consider(
        CompletionQuery(CompletionKind.command, lineStart + slash + 1, q),
      );
    }
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

/// Text after accepting `item` for `q` (replacing the query text, wherever the caret is).
TextEditingValue applyCompletion(
  String text,
  CompletionQuery q,
  CompletionItem item,
) {
  final end = q.start + q.query.length;
  if (end > text.length || text.substring(q.start, end) != q.query) {
    return TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
  }
  final head = text.substring(0, q.start);
  var tail = text.substring(end);
  switch (q.kind) {
    case CompletionKind.command:
      final cmd = slashCommands.firstWhere((c) => c.name == item.value);
      final body = cmd.text();
      final at = body.indexOf('|');
      final clean = body.replaceFirst('|', '');
      // `head` still ends with the `/`.
      final before = head.substring(0, head.length - 1);
      return TextEditingValue(
        text: before + clean + tail,
        selection: TextSelection.collapsed(
          offset: before.length + (at < 0 ? clean.length : at),
        ),
      );
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
