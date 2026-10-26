import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';

/// Maps a run of rendered text back to its Markdown source, so a tap on rendered text can
/// place the caret at the matching spot of the raw text.
class SourceRun {
  const SourceRun(
    this.r0,
    this.rLen,
    this.s0,
    this.sLen, {
    this.atomic = false,
  });
  final int r0;
  final int rLen;
  final int s0;
  final int sLen;

  /// Rendered differently from the source (widgets, resolved refs): maps to its end.
  final bool atomic;
}

/// A rendered block: its span tree, the source mapping and the recognizers to dispose.
class RenderedBlock {
  RenderedBlock(this.span, this.runs, this.recognizers);

  final InlineSpan span;
  final List<SourceRun> runs;
  final List<GestureRecognizer> recognizers;

  /// Source offset for a rendered text offset.
  int toSource(int rendered) {
    SourceRun? best;
    for (final r in runs) {
      if (rendered < r.r0) break;
      best = r;
      if (rendered < r.r0 + r.rLen) break;
    }
    if (best == null) return 0;
    if (best.atomic) return best.s0 + best.sLen;
    final d = (rendered - best.r0).clamp(0, best.sLen);
    return best.s0 + d;
  }

  void dispose() {
    for (final r in recognizers) {
      r.dispose();
    }
  }
}

/// Callbacks and context for rendering.
class RenderContext {
  const RenderContext({
    required this.colors,
    required this.base,
    required this.refs,
    required this.assetPath,
    required this.onPage,
    required this.onBlockRef,
    required this.onUrl,
    this.onToggleTodo,
    this.onPageNewTab,
    this.onBlockRefNewTab,
    this.maxWidth = 600,
  });

  final SynapseColors colors;
  final TextStyle base;
  final BlockRefCache refs;
  final String? Function(String hash) assetPath;
  final void Function(String title) onPage;
  final void Function(BlockRefDtoLike ref) onBlockRef;
  final void Function(String url) onUrl;
  final VoidCallback? onToggleTodo;

  /// Middle-click on a page link or block ref.
  final void Function(String title)? onPageNewTab;
  final void Function(BlockRefDtoLike ref)? onBlockRefNewTab;
  final double maxWidth;
}

/// The bits of a block ref the renderer needs (keeps this file free of FFI types).
class BlockRefDtoLike {
  const BlockRefDtoLike(this.id, this.pageId);
  final String id;
  final String pageId;
}

/// Characters a backslash can escape in a block (Markdown's ASCII punctuation).
const escapable = r'\\`*_{}[]()#+-.!~=|<>:"';

final _marker = RegExp(r'^(TODO|DOING|NOW|LATER|DONE|CANCELED|CANCELLED) ');
final _heading = RegExp(r'^(#{1,6}) ');
final _uuid = RegExp(
  r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$',
);
final _property = RegExp(r'^([A-Za-z0-9_\-]+):: ?(.*)$');

/// Renders Synapse outline Markdown of one block into a single [InlineSpan] tree.
class BlockRenderer {
  BlockRenderer(this.ctx);

  final RenderContext ctx;
  final _spans = <InlineSpan>[];
  final _runs = <SourceRun>[];
  final _recognizers = <GestureRecognizer>[];
  int _r = 0;

  RenderedBlock render(String src) {
    final lines = src.split('\n');
    var off = 0;
    var fenceStart = -1;
    final fence = StringBuffer();
    for (var li = 0; li < lines.length; li++) {
      final line = lines[li];
      final lineStart = off;
      off += line.length + 1;
      if (fenceStart >= 0) {
        if (line.trimLeft().startsWith('```')) {
          _codeBlock(fence.toString(), fenceStart, off - 1 - fenceStart);
          fenceStart = -1;
          fence.clear();
        } else {
          if (fence.isNotEmpty) fence.write('\n');
          fence.write(line);
        }
        continue;
      }
      if (li > 0) _text('\n', lineStart - 1, ctx.base);
      if (line.trimLeft().startsWith('```')) {
        fenceStart = lineStart;
        continue;
      }
      _line(line, lineStart, first: li == 0);
    }
    if (fenceStart >= 0) {
      _codeBlock(fence.toString(), fenceStart, src.length - fenceStart);
    }
    return RenderedBlock(
      TextSpan(style: ctx.base, children: List.of(_spans)),
      List.of(_runs),
      List.of(_recognizers),
    );
  }

  void _line(String line, int off, {required bool first}) {
    var style = ctx.base;
    var i = 0;
    if (first) {
      final m = _marker.firstMatch(line);
      if (m != null) {
        final done = m.group(1) == 'DONE';
        final canceled = m.group(1)!.startsWith('CANCEL');
        _atomic(_checkbox(done || canceled), 1, off, m.end);
        if (done || canceled) {
          style = style.copyWith(
            color: ctx.colors.muted,
            decoration: TextDecoration.lineThrough,
            decorationColor: ctx.colors.faint,
          );
        } else if (m.group(1) != 'TODO' && m.group(1) != 'LATER') {
          _text(
            '${m.group(1)} ',
            off,
            style.copyWith(
              color: ctx.colors.accent,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          );
        }
        i = m.end;
      }
    }
    final rest = line.substring(i);
    final h = _heading.firstMatch(rest);
    if (h != null) {
      final level = h.group(1)!.length;
      const sizes = [26.0, 22.0, 19.0, 17.0, 16.0, 15.0];
      style = style.copyWith(
        fontSize: sizes[level - 1],
        fontWeight: level <= 2 ? FontWeight.w700 : FontWeight.w600,
        height: 1.35,
        letterSpacing: level <= 2 ? -0.2 : 0,
      );
      _inline(rest.substring(h.end), off + i + h.end, style);
      return;
    }
    final p = _property.firstMatch(rest);
    if (p != null) {
      final small = style.copyWith(fontSize: 13, color: ctx.colors.muted);
      _text(
        '${p.group(1)}: ',
        off + i,
        small.copyWith(fontWeight: FontWeight.w600),
      );
      _inline(p.group(2)!, off + i + rest.length - p.group(2)!.length, small);
      return;
    }
    if (rest.startsWith('> ')) {
      _atomic(
        WidgetSpan(
          alignment: PlaceholderAlignment.middle,
          child: Container(
            width: 3,
            height: 18,
            margin: const EdgeInsets.only(right: 8),
            color: ctx.colors.line,
          ),
        ),
        1,
        off + i,
        2,
      );
      _inline(
        rest.substring(2),
        off + i + 2,
        style.copyWith(color: ctx.colors.muted, fontStyle: FontStyle.italic),
      );
      return;
    }
    final list = RegExp(r'^(\s*)([-*+]|\d+\.) ').firstMatch(rest);
    if (list != null) {
      final bullet = list.group(2)!.endsWith('.') ? list.group(2)! : '•';
      _atomic(
        TextSpan(
          text: '${list.group(1)}$bullet ',
          style: style.copyWith(color: ctx.colors.muted),
        ),
        list.group(1)!.length + bullet.length + 1,
        off + i,
        list.end,
      );
      _inline(rest.substring(list.end), off + i + list.end, style);
      return;
    }
    _inline(rest, off + i, style);
  }

  void _inline(String s, int off, TextStyle style) {
    var i = 0;
    var plain = 0;
    void flush(int to) {
      if (to > plain) _text(s.substring(plain, to), off + plain, style);
    }

    bool ws(int k) => k < 0 || k >= s.length || s[k].trim().isEmpty;

    while (i < s.length) {
      final c = s[i];
      // Backslash escape: `\*` shows a literal `*` and hides the backslash.
      if (c == r'\' && i + 1 < s.length && escapable.contains(s[i + 1])) {
        flush(i);
        _text(s[i + 1], off + i + 1, style);
        i = plain = i + 2;
        continue;
      }
      // `code`
      if (c == '`') {
        final j = s.indexOf('`', i + 1);
        if (j > i + 1) {
          flush(i);
          _text(
            s.substring(i + 1, j),
            off + i + 1,
            style.copyWith(
              fontFamily: monoFont,
              fontSize: (style.fontSize ?? blockFontSize) - 1.5,
              backgroundColor: ctx.colors.codeBg,
              color: ctx.colors.ink,
            ),
          );
          i = plain = j + 1;
          continue;
        }
      }
      // ![alt](src)
      if (s.startsWith('![', i)) {
        final link = _mdLink(s, i + 1);
        if (link != null) {
          flush(i);
          _atomic(_image(link.$2, link.$1), 1, off + i, link.$3 - i);
          i = plain = link.$3;
          continue;
        }
      }
      // [[Page]]
      if (s.startsWith('[[', i)) {
        final close = _findClose(s, i + 2);
        if (close != null && close > i + 2) {
          flush(i);
          final title = s.substring(i + 2, close);
          _text(
            title,
            off + i + 2,
            style.copyWith(
              color: ctx.colors.accent,
              fontWeight: FontWeight.w500,
            ),
            onTap: () => ctx.onPage(title),
            onMiddle: ctx.onPageNewTab == null
                ? null
                : () => ctx.onPageNewTab!(title),
          );
          i = plain = close + 2;
          continue;
        }
      }
      // ((block ref))
      if (s.startsWith('((', i)) {
        final j = s.indexOf('))', i + 2);
        if (j > 0 && _uuid.hasMatch(s.substring(i + 2, j))) {
          flush(i);
          _blockRef(s.substring(i + 2, j), off + i, j + 2 - i, style);
          i = plain = j + 2;
          continue;
        }
      }
      // #tag, #[[tag]]
      if (c == '#' && ws(i - 1) && i + 1 < s.length) {
        String? tag;
        var end = i;
        if (s.startsWith('#[[', i)) {
          final close = _findClose(s, i + 3);
          if (close != null && close > i + 3) {
            tag = s.substring(i + 3, close);
            end = close + 2;
          }
        } else {
          var j = i + 1;
          while (j < s.length && !_tagStop(s[j])) {
            j++;
          }
          // Trailing punctuation is not part of the tag.
          while (j > i + 1 && '.!?;:'.contains(s[j - 1])) {
            j--;
          }
          if (j > i + 1) {
            tag = s.substring(i + 1, j);
            end = j;
          }
        }
        if (tag != null) {
          flush(i);
          final t = tag;
          final tagStyle = style.copyWith(
            color: ctx.colors.accent.withAlpha(0xCC),
          );
          if (end - i == t.length + 1) {
            _text('#$t', off + i, tagStyle, onTap: () => ctx.onPage(t));
          } else {
            _text('#', off + i, tagStyle, onTap: () => ctx.onPage(t));
            _text(t, off + i + 3, tagStyle, onTap: () => ctx.onPage(t));
          }
          i = plain = end;
          continue;
        }
      }
      // **bold**, __bold__, ~~strike~~, ==highlight==
      final pair = s.length > i + 1 ? s.substring(i, i + 2) : '';
      if (pair == '**' || pair == '__' || pair == '~~' || pair == '==') {
        final j = s.indexOf(pair, i + 2);
        if (j > i + 2) {
          flush(i);
          final inner = switch (pair) {
            '~~' => style.copyWith(
              decoration: TextDecoration.lineThrough,
              color: ctx.colors.muted,
            ),
            '==' => style.copyWith(backgroundColor: ctx.colors.highlight),
            _ => style.copyWith(fontWeight: FontWeight.w700),
          };
          _inline(s.substring(i + 2, j), off + i + 2, inner);
          i = plain = j + 2;
          continue;
        }
      }
      // *italic*, _italic_
      if ((c == '*' || (c == '_' && ws(i - 1))) &&
          i + 1 < s.length &&
          !ws(i + 1)) {
        var j = s.indexOf(c, i + 1);
        while (j > 0 && (ws(j - 1) || (c == '_' && !ws(j + 1)))) {
          j = s.indexOf(c, j + 1);
        }
        if (j > i + 1) {
          flush(i);
          _inline(
            s.substring(i + 1, j),
            off + i + 1,
            style.copyWith(fontStyle: FontStyle.italic),
          );
          i = plain = j + 1;
          continue;
        }
      }
      // [label](url)
      if (c == '[') {
        final link = _mdLink(s, i);
        if (link != null) {
          flush(i);
          final url = link.$2;
          _text(
            link.$1,
            off + i + 1,
            style.copyWith(
              color: ctx.colors.accent,
              decoration: TextDecoration.underline,
              decorationColor: ctx.colors.accent.withAlpha(0x66),
            ),
            onTap: () => ctx.onUrl(url),
          );
          i = plain = link.$3;
          continue;
        }
      }
      // bare URL
      if ((s.startsWith('https://', i) || s.startsWith('http://', i)) &&
          ws(i - 1)) {
        var j = i;
        while (j < s.length && !ws(j)) {
          j++;
        }
        while (j > i && '.,;:!?)'.contains(s[j - 1])) {
          j--;
        }
        flush(i);
        final url = s.substring(i, j);
        _text(
          url,
          off + i,
          style.copyWith(
            color: ctx.colors.accent,
            decoration: TextDecoration.underline,
            decorationColor: ctx.colors.accent.withAlpha(0x66),
          ),
          onTap: () => ctx.onUrl(url),
        );
        i = plain = j;
        continue;
      }
      i++;
    }
    flush(s.length);
  }

  static bool _tagStop(String ch) =>
      ch.trim().isEmpty || ',()[]{}"\'`#'.contains(ch);

  /// `[label](target)` starting at `i` (the `[`): (label, target, end).
  static (String, String, int)? _mdLink(String s, int i) {
    if (i >= s.length || s[i] != '[') return null;
    final close = s.indexOf('](', i + 1);
    if (close < 0 || s.substring(i + 1, close).contains('\n')) return null;
    final end = s.indexOf(')', close + 2);
    if (end < 0) return null;
    final target = s.substring(close + 2, end).trim();
    if (target.isEmpty || target.contains(' ')) return null;
    return (s.substring(i + 1, close), target, end + 1);
  }

  /// Index of the `]]` closing a `[[` whose content starts at `from` (nesting aware).
  static int? _findClose(String s, int from) {
    var depth = 1;
    var i = from;
    while (i < s.length - 1) {
      if (s.startsWith('[[', i)) {
        depth++;
        i += 2;
      } else if (s.startsWith(']]', i)) {
        depth--;
        if (depth == 0) return i;
        i += 2;
      } else {
        if (s[i] == '\n') return null;
        i++;
      }
    }
    return null;
  }

  void _text(
    String t,
    int s0,
    TextStyle style, {
    VoidCallback? onTap,
    VoidCallback? onMiddle,
  }) {
    if (t.isEmpty) return;
    TapGestureRecognizer? rec;
    if (onTap != null) {
      rec = TapGestureRecognizer()..onTap = onTap;
      if (onMiddle != null) rec.onTertiaryTapUp = (_) => onMiddle();
      _recognizers.add(rec);
    }
    _spans.add(
      TextSpan(
        text: t,
        style: style,
        recognizer: rec,
        mouseCursor: rec == null ? null : SystemMouseCursors.click,
      ),
    );
    _runs.add(SourceRun(_r, t.length, s0, t.length));
    _r += t.length;
  }

  void _atomic(InlineSpan span, int rLen, int s0, int sLen) {
    _spans.add(span);
    _runs.add(SourceRun(_r, rLen, s0, sLen, atomic: true));
    _r += rLen;
  }

  void _blockRef(String id, int s0, int sLen, TextStyle style) {
    final ref = ctx.refs.get(id);
    final String text;
    if (ref != null) {
      final first = ref.content.split('\n').first.replaceFirst(_marker, '');
      text = first.isEmpty ? '(empty block)' : first;
    } else {
      text = ctx.refs.isMissing(id) ? '((missing block))' : '…';
    }
    TapGestureRecognizer? rec;
    if (ref != null) {
      rec = TapGestureRecognizer()
        ..onTap = (() => ctx.onBlockRef(BlockRefDtoLike(ref.id, ref.pageId)))
        ..onTertiaryTapUp = (_) =>
            ctx.onBlockRefNewTab?.call(BlockRefDtoLike(ref.id, ref.pageId));
      _recognizers.add(rec);
    }
    _spans.add(
      TextSpan(
        text: text,
        recognizer: rec,
        mouseCursor: rec == null ? null : SystemMouseCursors.click,
        style: style.copyWith(
          decoration: TextDecoration.underline,
          decorationStyle: TextDecorationStyle.dotted,
          decorationColor: ctx.colors.ref,
          color: ref == null ? ctx.colors.faint : null,
        ),
      ),
    );
    _runs.add(SourceRun(_r, text.length, s0, sLen, atomic: true));
    _r += text.length;
  }

  InlineSpan _checkbox(bool checked) {
    final c = ctx.colors;
    return WidgetSpan(
      alignment: PlaceholderAlignment.middle,
      child: Padding(
        padding: const EdgeInsets.only(right: 7),
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: GestureDetector(
            onTap: ctx.onToggleTodo,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 140),
              width: 15,
              height: 15,
              decoration: BoxDecoration(
                color: checked ? c.done : Colors.transparent,
                borderRadius: BorderRadius.circular(4),
                border: Border.all(
                  color: checked ? c.done : c.faint,
                  width: 1.5,
                ),
              ),
              child: checked
                  ? Icon(Icons.check_rounded, size: 12, color: c.paper)
                  : null,
            ),
          ),
        ),
      ),
    );
  }

  InlineSpan _image(String src, String alt) {
    Widget img;
    if (src.startsWith('asset://')) {
      final path = ctx.assetPath(src.substring(8));
      img = path == null
          ? _missing(alt.isEmpty ? 'missing asset' : alt)
          : Image.file(
              File(path),
              fit: BoxFit.contain,
              cacheWidth: (ctx.maxWidth * 2).round(),
              errorBuilder: (_, _, _) => _missing(alt),
            );
    } else if (src.startsWith('http://') || src.startsWith('https://')) {
      img = Image.network(
        src,
        fit: BoxFit.contain,
        errorBuilder: (_, _, _) => _missing(alt),
      );
    } else {
      img = _missing(alt.isEmpty ? src : alt);
    }
    return WidgetSpan(
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: 360, maxWidth: ctx.maxWidth),
          child: ClipRRect(borderRadius: BorderRadius.circular(8), child: img),
        ),
      ),
    );
  }

  Widget _missing(String label) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
    decoration: BoxDecoration(
      color: ctx.colors.surface,
      borderRadius: BorderRadius.circular(6),
      border: Border.all(color: ctx.colors.line),
    ),
    child: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(
          Icons.image_not_supported_outlined,
          size: 14,
          color: ctx.colors.muted,
        ),
        const SizedBox(width: 6),
        Text(label, style: TextStyle(fontSize: 12, color: ctx.colors.muted)),
      ],
    ),
  );

  void _codeBlock(String code, int s0, int sLen) {
    _atomic(
      WidgetSpan(
        child: Container(
          width: ctx.maxWidth,
          margin: const EdgeInsets.symmetric(vertical: 2),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          decoration: BoxDecoration(
            color: ctx.colors.codeBg,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Text(
            code,
            style: TextStyle(
              fontFamily: monoFont,
              fontSize: 13,
              height: 1.5,
              color: ctx.colors.ink,
            ),
          ),
        ),
      ),
      1,
      s0,
      sLen,
    );
  }
}
