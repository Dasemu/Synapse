import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../state/keymap.dart';
import '../state/outline_controller.dart';
import '../state/outline_ops.dart';
import '../theme/theme.dart';
import 'completion.dart';
import 'markdown.dart';

const indentWidth = 24.0;
const _gutter = 18.0;
const _bulletBox = 18.0;

/// What a bullet drag carries.
class BlockDrag {
  const BlockDrag(this.controller, this.node);
  final OutlineController controller;
  final OutlineNode node;
}

enum _Drop { before, after, child }

/// One outline row: guides, collapse arrow, bullet (drag handle) and the block itself,
/// rendered when idle and a raw-Markdown editor when focused.
class BlockRow extends StatefulWidget {
  const BlockRow({
    super.key,
    required this.controller,
    required this.node,
    required this.editing,
    required this.hasChildren,
  });

  final OutlineController controller;
  final OutlineNode node;
  final bool editing;
  final bool hasChildren;

  @override
  State<BlockRow> createState() => _BlockRowState();
}

class _BlockRowState extends State<BlockRow> {
  bool _hover = false;
  _Drop? _drop;

  OutlineController get c => widget.controller;
  OutlineNode get n => widget.node;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final depth = n.depth;
    final lineH = blockFontSize * blockLineHeight;
    return DragTarget<BlockDrag>(
      onWillAcceptWithDetails: (d) =>
          d.data.controller == c && d.data.node != n,
      onMove: (d) => _updateDrop(d.offset),
      onLeave: (_) => setState(() => _drop = null),
      onAcceptWithDetails: (d) {
        final drop = _drop;
        setState(() => _drop = null);
        if (drop == null) return;
        c.moveTo(d.data.node, switch (drop) {
          _Drop.before => PlaceKind.before,
          _Drop.after => PlaceKind.after,
          _Drop.child => PlaceKind.firstChildOf,
        }, n);
      },
      builder: (context, _, _) => MouseRegion(
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: CustomPaint(
          painter: _GuidesPainter(depth, colors.line, _drop, colors.accent),
          child: Padding(
            padding: EdgeInsets.only(left: depth * indentWidth),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: _gutter,
                  height: lineH + 4,
                  child: (widget.hasChildren && (_hover || n.collapsed))
                      ? _Arrow(
                          collapsed: n.collapsed,
                          onTap: () => c.toggleCollapsed(n),
                        )
                      : null,
                ),
                _bullet(colors, lineH),
                const SizedBox(width: 6),
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 2),
                    child: widget.editing
                        ? BlockEditor(
                            key: ValueKey('edit-${identityHashCode(n)}'),
                            controller: c,
                            node: n,
                          )
                        : BlockView(controller: c, node: n),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _bullet(SynapseColors colors, double lineH) {
    final collapsedWithKids = n.collapsed && widget.hasChildren;
    final dot = SizedBox(
      width: _bulletBox,
      height: lineH + 4,
      child: Center(
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          width: 16,
          height: 16,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: collapsedWithKids ? colors.line : Colors.transparent,
          ),
          child: Center(
            child: Container(
              width: 6,
              height: 6,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _hover ? colors.muted : colors.faint,
              ),
            ),
          ),
        ),
      ),
    );
    return Draggable<BlockDrag>(
      data: BlockDrag(c, n),
      dragAnchorStrategy: pointerDragAnchorStrategy,
      feedback: _DragFeedback(text: n.content),
      childWhenDragging: Opacity(opacity: 0.3, child: dot),
      child: MouseRegion(
        cursor: SystemMouseCursors.grab,
        child: GestureDetector(
          onTap: widget.hasChildren ? () => c.toggleCollapsed(n) : null,
          child: dot,
        ),
      ),
    );
  }

  void _updateDrop(Offset global) {
    final box = context.findRenderObject() as RenderBox?;
    if (box == null) return;
    final p = box.globalToLocal(global);
    final contentX = n.depth * indentWidth + _gutter + _bulletBox;
    final _Drop d;
    if (p.dy < box.size.height / 2) {
      d = _Drop.before;
    } else {
      // Below an expanded parent the gap is above its first child: drop there.
      final expandedParent = widget.hasChildren && !n.collapsed;
      d = expandedParent || p.dx > contentX + 28 ? _Drop.child : _Drop.after;
    }
    if (d != _drop) setState(() => _drop = d);
  }
}

class _Arrow extends StatelessWidget {
  const _Arrow({required this.collapsed, required this.onTap});
  final bool collapsed;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => MouseRegion(
    cursor: SystemMouseCursors.click,
    child: GestureDetector(
      onTap: onTap,
      child: Center(
        child: AnimatedRotation(
          turns: collapsed ? 0 : 0.25,
          duration: const Duration(milliseconds: 140),
          child: Icon(
            Icons.chevron_right_rounded,
            size: 16,
            color: context.colors.faint,
          ),
        ),
      ),
    ),
  );
}

class _DragFeedback extends StatelessWidget {
  const _DragFeedback({required this.text});
  final String text;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Material(
      color: Colors.transparent,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 320),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          color: colors.paper,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: colors.line),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withAlpha(0x22),
              blurRadius: 16,
              offset: const Offset(0, 6),
            ),
          ],
        ),
        child: Text(
          text.split('\n').first,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: 14, color: colors.ink),
        ),
      ),
    );
  }
}

/// Vertical indent guides plus the drop indicator.
class _GuidesPainter extends CustomPainter {
  _GuidesPainter(this.depth, this.color, this.drop, this.accent);
  final int depth;
  final Color color;
  final _Drop? drop;
  final Color accent;

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()
      ..color = color
      ..strokeWidth = 1;
    for (var k = 0; k < depth; k++) {
      final x = k * indentWidth + _gutter + _bulletBox / 2;
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), p);
    }
    if (drop != null) {
      final a = Paint()
        ..color = accent
        ..strokeWidth = 2
        ..strokeCap = StrokeCap.round;
      var x = depth * indentWidth + _gutter;
      if (drop == _Drop.child) x += indentWidth;
      final y = drop == _Drop.before ? 1.0 : size.height - 1;
      canvas.drawLine(Offset(x, y), Offset(size.width - 8, y), a);
    }
  }

  @override
  bool shouldRepaint(_GuidesPainter old) =>
      old.depth != depth ||
      old.drop != drop ||
      old.color != color ||
      old.accent != accent;
}

TextStyle blockTextStyle(BuildContext context) => TextStyle(
  fontFamily: uiFont,
  fontSize: blockFontSize,
  height: blockLineHeight,
  color: context.colors.ink,
);

/// Rendered (read) mode of a block. A tap places the caret at the matching source offset.
class BlockView extends StatefulWidget {
  const BlockView({super.key, required this.controller, required this.node});
  final OutlineController controller;
  final OutlineNode node;

  @override
  State<BlockView> createState() => _BlockViewState();
}

class _BlockViewState extends State<BlockView> {
  final _key = GlobalKey();
  RenderedBlock? _rendered;
  String? _renderedFor;
  Object? _renderedTheme;
  double _width = 0;

  @override
  void dispose() {
    _rendered?.dispose();
    super.dispose();
  }

  RenderedBlock _render(BuildContext context, double width) {
    final content = widget.node.content;
    final theme = Theme.of(context);
    if (_rendered != null &&
        _renderedFor == content &&
        _renderedTheme == theme &&
        _width == width) {
      return _rendered!;
    }
    _rendered?.dispose();
    final app = AppScope.read(context);
    final ctx = RenderContext(
      colors: context.colors,
      base: blockTextStyle(context),
      refs: app.refs,
      assetPath: app.backend.assetPath,
      onPage: app.openTitle,
      onBlockRef: (r) => app.openId(r.pageId),
      onUrl: openUrl,
      onToggleTodo: () => widget.controller.cycleTodo(widget.node),
      maxWidth: width,
    );
    _renderedFor = content;
    _renderedTheme = theme;
    _width = width;
    return _rendered = BlockRenderer(ctx).render(content);
  }

  void _tap(TapUpDetails d) {
    final rp = _key.currentContext?.findRenderObject();
    var caret = widget.node.content.length;
    if (rp is RenderParagraph && _rendered != null) {
      final local = rp.globalToLocal(d.globalPosition);
      if (local.dy <= rp.size.height) {
        caret = _rendered!.toSource(rp.getPositionForOffset(local).offset);
      }
    }
    widget.controller.focus(widget.node.id, caret: caret);
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final hasRef = widget.node.content.contains('((');
    Widget build(double width) {
      final r = _render(context, width);
      return Text.rich(r.span, key: _key);
    }

    return LayoutBuilder(
      builder: (context, box) => MouseRegion(
        cursor: SystemMouseCursors.text,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapUp: _tap,
          child: ConstrainedBox(
            constraints: const BoxConstraints(
              minHeight: blockFontSize * blockLineHeight,
            ),
            child: hasRef
                ? ListenableBuilder(
                    listenable: app.refs,
                    builder: (context, _) {
                      _renderedFor = null;
                      return build(box.maxWidth);
                    },
                  )
                : build(box.maxWidth),
          ),
        ),
      ),
    );
  }
}

/// Opens an external link. Replaced at startup with a platform launcher.
void Function(String url) openUrl = (url) {};

/// Edit mode of a block: raw Markdown with outliner key handling and autocomplete.
class BlockEditor extends StatefulWidget {
  const BlockEditor({super.key, required this.controller, required this.node});
  final OutlineController controller;
  final OutlineNode node;

  @override
  State<BlockEditor> createState() => _BlockEditorState();
}

class _BlockEditorState extends State<BlockEditor> {
  late final TextEditingController _text;
  late final FocusNode _focus;
  final _fieldKey = GlobalKey();
  final _link = LayerLink();
  final _portal = OverlayPortalController();
  TextEditingValue _last = TextEditingValue.empty;
  bool _allowNewline = false;

  CompletionQuery? _query;
  List<CompletionItem> _items = const [];
  int _selected = 0;
  int _searchSeq = 0;

  OutlineController get c => widget.controller;
  OutlineNode get n => widget.node;

  @override
  void initState() {
    super.initState();
    final caret = (c.pendingCaret ?? n.content.length).clamp(
      0,
      n.content.length,
    );
    c.pendingCaret = null;
    _text = TextEditingController.fromValue(
      TextEditingValue(
        text: n.content,
        selection: TextSelection.collapsed(offset: caret),
      ),
    );
    _last = _text.value;
    _focus = FocusNode(debugLabel: 'block', onKeyEvent: _onKey);
    _focus.addListener(_focusChanged);
    c.addListener(_controllerChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _focus.requestFocus();
    });
  }

  @override
  void dispose() {
    c.removeListener(_controllerChanged);
    _focus.removeListener(_focusChanged);
    _focus.dispose();
    _text.dispose();
    super.dispose();
  }

  void _focusChanged() {
    if (!_focus.hasFocus) {
      _closeCompletion();
      // Defer: focus may be moving to another block of the same outline.
      scheduleMicrotask(() {
        if (mounted && !_focus.hasFocus) c.blurred(n.id);
      });
    }
  }

  /// Applies caret requests and external content changes (merges, todo toggles).
  void _controllerChanged() {
    if (!mounted || c.focusedId != n.id) return;
    final caret = c.pendingCaret;
    if (n.content != _text.text || caret != null) {
      c.pendingCaret = null;
      final off = (caret ?? _text.selection.extentOffset).clamp(
        0,
        n.content.length,
      );
      _text.value = TextEditingValue(
        text: n.content,
        selection: TextSelection.collapsed(offset: off),
      );
      _last = _text.value;
    }
    if (!_focus.hasFocus) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && c.focusedId == n.id) _focus.requestFocus();
      });
    }
  }

  void _onChanged(String _) {
    var v = _text.value;
    // A newline that did not come from Shift+Enter (soft keyboards send Enter as text):
    // split the block instead.
    if (!_allowNewline &&
        v.text.length == _last.text.length + 1 &&
        v.selection.isCollapsed &&
        v.selection.baseOffset > 0 &&
        v.text[v.selection.baseOffset - 1] == '\n') {
      final at = v.selection.baseOffset - 1;
      final restored = v.text.substring(0, at) + v.text.substring(at + 1);
      c.setText(n, restored);
      c.split(n, at);
      return;
    }
    _allowNewline = false;
    final paired = autoPair(_last, v);
    if (paired != null) {
      _text.value = paired;
      v = paired;
    }
    _last = v;
    c.setText(n, v.text);
    _updateCompletion();
  }

  // -------------------------------------------------------- completion

  void _updateCompletion() {
    final sel = _text.selection;
    final q = sel.isCollapsed
        ? detectCompletion(_text.text, sel.baseOffset)
        : null;
    if (q == null || (q.kind == CompletionKind.tag && q.query.isEmpty)) {
      _closeCompletion();
      return;
    }
    if (q == _query) return;
    _query = q;
    final seq = ++_searchSeq;
    final backend = c.backend;
    Future<List<CompletionItem>> search() async {
      if (q.kind == CompletionKind.command) {
        return [
          for (final cmd in matchCommands(q.query))
            CompletionItem('/${cmd.name}', cmd.name, detail: cmd.label),
        ];
      }
      if (q.kind == CompletionKind.block) {
        final hits = await backend.searchBlocks(q.query, limit: 8);
        return [
          for (final h in hits)
            if (h.id != n.id)
              CompletionItem(
                h.content.split('\n').first,
                h.id,
                detail: h.pageTitle,
              ),
        ];
      }
      final hits = await backend.searchTitles(q.query, limit: 8);
      final items = [
        for (final h in hits)
          CompletionItem(h.title, h.title, detail: h.isAlias ? 'alias' : null),
      ];
      final query = q.query.trim();
      if (query.isNotEmpty &&
          !items.any((i) => i.value.toLowerCase() == query.toLowerCase())) {
        items.add(CompletionItem(query, query, isNew: true));
      }
      return items;
    }

    search().then((items) {
      if (!mounted || seq != _searchSeq) return;
      setState(() {
        _items = items;
        _selected = 0;
      });
      if (items.isEmpty) {
        _portal.hide();
      } else {
        _portal.show();
      }
    }, onError: (Object _) {});
  }

  void _closeCompletion() {
    _searchSeq++;
    _query = null;
    if (_items.isNotEmpty && mounted) setState(() => _items = const []);
    if (_portal.isShowing) _portal.hide();
  }

  bool get _completing => _query != null && _items.isNotEmpty;

  void _accept(int i) {
    final q = _query;
    if (q == null || i >= _items.length) return;
    final v = applyCompletion(_text.text, q, _items[i]);
    final again =
        q.kind == CompletionKind.command &&
        slashCommands.firstWhere((c) => c.name == _items[i].value).then;
    _closeCompletion();
    _text.value = v;
    _last = v;
    c.setText(n, v.text);
    // `/link` leaves the caret inside `[[]]`: go straight on to the page picker.
    if (again) _updateCompletion();
  }

  /// Applies an editor formatting shortcut to the selection; false if `a` is not one.
  bool _format(AppAction a) {
    final (open, close) = switch (a) {
      AppAction.bold => ('**', '**'),
      AppAction.italic => ('*', '*'),
      AppAction.strike => ('~~', '~~'),
      AppAction.highlight => ('==', '=='),
      AppAction.code => ('`', '`'),
      AppAction.link => ('[[', ']]'),
      _ => ('', ''),
    };
    if (open.isEmpty) return false;
    final v = wrapSelection(_text.value, open, close);
    _text.value = v;
    _last = v;
    c.setText(n, v.text);
    _updateCompletion();
    return true;
  }

  // -------------------------------------------------------------- keys

  RenderEditable? _editable() {
    RenderEditable? found;
    void visit(RenderObject o) {
      if (found != null) return;
      if (o is RenderEditable) {
        found = o;
        return;
      }
      o.visitChildren(visit);
    }

    final ro = _fieldKey.currentContext?.findRenderObject();
    if (ro != null) visit(ro);
    return found;
  }

  /// Whether the caret is on the first (or last) visual line, wrapping included.
  bool _onEdgeLine({required bool top}) {
    final re = _editable();
    if (re == null || !re.hasSize) return true;
    final caret = _text.selection.extentOffset;
    final rect = re.getLocalRectForCaret(TextPosition(offset: caret));
    final lh = re.preferredLineHeight;
    return top ? rect.top < lh * 0.5 : rect.bottom > re.size.height - lh * 0.5;
  }

  int get _column {
    final caret = _text.selection.extentOffset;
    final lineStart =
        _text.text.lastIndexOf('\n', caret == 0 ? 0 : caret - 1) + 1;
    return caret - lineStart;
  }

  KeyEventResult _onKey(FocusNode _, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final kb = HardwareKeyboard.instance;
    final shift = kb.isShiftPressed;
    final ctrl = kb.isControlPressed || kb.isMetaPressed;
    final alt = kb.isAltPressed;
    final key = e.logicalKey;
    final sel = _text.selection;
    final collapsed = sel.isValid && sel.isCollapsed;
    final caret = sel.isValid ? sel.extentOffset : _text.text.length;
    final len = _text.text.length;

    if (ctrl || alt) {
      final bound = AppScope.read(context).keymap.actionFor(
        Combo(key, ctrl: ctrl, shift: shift, alt: alt),
        editor: true,
      );
      if (bound != null && _format(bound)) return KeyEventResult.handled;
    }

    if (_completing) {
      if (key == LogicalKeyboardKey.arrowDown) {
        setState(() => _selected = (_selected + 1) % _items.length);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.arrowUp) {
        setState(
          () => _selected = (_selected - 1 + _items.length) % _items.length,
        );
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.enter ||
          key == LogicalKeyboardKey.numpadEnter ||
          key == LogicalKeyboardKey.tab) {
        _accept(_selected);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.escape) {
        _closeCompletion();
        return KeyEventResult.handled;
      }
      // Moving the caret elsewhere ends the completion.
      if (key == LogicalKeyboardKey.arrowLeft ||
          key == LogicalKeyboardKey.arrowRight ||
          key == LogicalKeyboardKey.home ||
          key == LogicalKeyboardKey.end) {
        _closeCompletion();
      }
    }

    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      if (shift) {
        _allowNewline = true;
        return KeyEventResult.ignored;
      }
      if (ctrl) {
        final shiftBy = c.cycleTodo(n);
        c.pendingCaret = (caret + shiftBy).clamp(0, n.content.length);
        _controllerChanged();
        return KeyEventResult.handled;
      }
      if (sel.isValid && !collapsed) {
        c.setText(n, _text.text.replaceRange(sel.start, sel.end, ''));
      }
      c.split(n, sel.isValid ? sel.start : len);
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.tab) {
      if (shift) {
        c.outdent(n, caret: caret);
      } else {
        c.indent(n, caret: caret);
      }
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.backspace &&
        collapsed &&
        caret == 0 &&
        !ctrl) {
      return c.mergeIntoPrevious(n)
          ? KeyEventResult.handled
          : KeyEventResult.ignored;
    }
    if (key == LogicalKeyboardKey.delete &&
        collapsed &&
        caret == len &&
        !ctrl) {
      return c.mergeNext(n) ? KeyEventResult.handled : KeyEventResult.ignored;
    }
    if (key == LogicalKeyboardKey.arrowUp ||
        key == LogicalKeyboardKey.arrowDown) {
      final up = key == LogicalKeyboardKey.arrowUp;
      if (alt && shift) {
        c.moveAmongSiblings(n, up: up, caret: caret);
        return KeyEventResult.handled;
      }
      if (ctrl && !shift && !alt) {
        if (up != n.collapsed && c.hasChildren(n)) c.toggleCollapsed(n);
        return KeyEventResult.handled;
      }
      if (shift || alt || ctrl) return KeyEventResult.ignored;
      if (!_onEdgeLine(top: up)) return KeyEventResult.ignored;
      final other = up ? c.previousVisible(n) : c.nextVisible(n);
      if (other == null) return KeyEventResult.ignored;
      final col = _column;
      final t = other.content;
      final int target;
      if (up) {
        final ls = t.lastIndexOf('\n') + 1;
        target = ls + col.clamp(0, t.length - ls);
      } else {
        final le = t.indexOf('\n');
        target = col.clamp(0, le < 0 ? t.length : le);
      }
      c.focus(other.id, caret: target);
      return KeyEventResult.handled;
    }
    if (!shift && !ctrl && !alt && collapsed) {
      if (key == LogicalKeyboardKey.arrowLeft && caret == 0) {
        final prev = c.previousVisible(n);
        if (prev != null) {
          c.focus(prev.id, caret: prev.content.length);
          return KeyEventResult.handled;
        }
      }
      if (key == LogicalKeyboardKey.arrowRight && caret == len) {
        final next = c.nextVisible(n);
        if (next != null) {
          c.focus(next.id, caret: 0);
          return KeyEventResult.handled;
        }
      }
    }
    if (key == LogicalKeyboardKey.escape) {
      _focus.unfocus();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return CompositedTransformTarget(
      link: _link,
      child: OverlayPortal(
        controller: _portal,
        overlayChildBuilder: (context) => CompositedTransformFollower(
          link: _link,
          targetAnchor: Alignment.bottomLeft,
          offset: const Offset(0, 4),
          child: Align(
            alignment: Alignment.topLeft,
            child: TextFieldTapRegion(
              child: CompletionPopup(
                items: _items,
                selected: _selected,
                kind: _query?.kind ?? CompletionKind.page,
                onSelect: _accept,
              ),
            ),
          ),
        ),
        child: TextField(
          key: _fieldKey,
          controller: _text,
          focusNode: _focus,
          onChanged: _onChanged,
          maxLines: null,
          keyboardType: TextInputType.multiline,
          textInputAction: TextInputAction.newline,
          style: blockTextStyle(context).copyWith(fontFamily: uiFont),
          cursorColor: colors.accent,
          cursorWidth: 1.6,
          decoration: const InputDecoration.collapsed(hintText: null),
        ),
      ),
    );
  }
}

class CompletionPopup extends StatelessWidget {
  const CompletionPopup({
    super.key,
    required this.items,
    required this.selected,
    required this.kind,
    required this.onSelect,
  });

  final List<CompletionItem> items;
  final int selected;
  final CompletionKind kind;
  final void Function(int) onSelect;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return Material(
      color: colors.paper,
      elevation: 0,
      borderRadius: BorderRadius.circular(10),
      child: Container(
        width: 340,
        padding: const EdgeInsets.all(4),
        decoration: BoxDecoration(
          color: colors.paper,
          borderRadius: BorderRadius.circular(10),
          border: Border.all(color: colors.line),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withAlpha(0x1F),
              blurRadius: 24,
              offset: const Offset(0, 8),
            ),
          ],
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < items.length; i++)
              MouseRegion(
                cursor: SystemMouseCursors.click,
                child: GestureDetector(
                  onTap: () => onSelect(i),
                  child: Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 7,
                    ),
                    decoration: BoxDecoration(
                      color: i == selected ? colors.accentSoft : null,
                      borderRadius: BorderRadius.circular(7),
                    ),
                    child: Row(
                      children: [
                        Icon(
                          items[i].isNew
                              ? Icons.add_rounded
                              : kind == CompletionKind.block
                              ? Icons.notes_rounded
                              : kind == CompletionKind.command
                              ? Icons.bolt_rounded
                              : kind == CompletionKind.tag
                              ? Icons.tag_rounded
                              : Icons.description_outlined,
                          size: 15,
                          color: i == selected ? colors.accent : colors.muted,
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            items[i].isNew
                                ? 'New page “${items[i].label}”'
                                : items[i].label,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 13.5, color: colors.ink),
                          ),
                        ),
                        if (items[i].detail != null) ...[
                          const SizedBox(width: 8),
                          Text(
                            items[i].detail!,
                            style: TextStyle(fontSize: 12, color: colors.faint),
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
