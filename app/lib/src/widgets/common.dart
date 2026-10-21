import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../editor/block_row.dart';
import '../editor/markdown.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';

/// Max width of the reading column.
const contentWidth = 780.0;

/// Horizontal padding that centres a [contentWidth] column in `width`.
double sidePadding(double width) => math.max(20, (width - contentWidth) / 2);

/// Small caps section label.
class SectionLabel extends StatelessWidget {
  const SectionLabel(this.text, {super.key, this.trailing});
  final String text;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      Text(text.toUpperCase(), style: Theme.of(context).textTheme.labelSmall),
      if (trailing != null) ...[const SizedBox(width: 8), trailing!],
    ],
  );
}

/// A link-like text button.
class TextLink extends StatefulWidget {
  const TextLink(
    this.text, {
    super.key,
    required this.onTap,
    this.style,
    this.icon,
    this.iconAfter = false,
  });
  final String text;
  final VoidCallback onTap;
  final TextStyle? style;
  final IconData? icon;
  final bool iconAfter;

  @override
  State<TextLink> createState() => _TextLinkState();
}

class _TextLinkState extends State<TextLink> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    final color = _hover
        ? colors.accent
        : (widget.style?.color ?? colors.muted);
    final style = (widget.style ?? const TextStyle(fontSize: 13)).copyWith(
      color: color,
    );
    final icon = widget.icon == null
        ? null
        : Icon(widget.icon, size: (style.fontSize ?? 13) + 2, color: color);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (icon != null && !widget.iconAfter) ...[
              icon,
              const SizedBox(width: 4),
            ],
            Flexible(
              child: Text(
                widget.text,
                style: style,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (icon != null && widget.iconAfter) ...[
              const SizedBox(width: 4),
              icon,
            ],
          ],
        ),
      ),
    );
  }
}

/// Read-only rendered block (linked references). Taps on links navigate; a tap elsewhere
/// calls [onTap].
class StaticBlock extends StatefulWidget {
  const StaticBlock({super.key, required this.content, this.onTap});
  final String content;
  final VoidCallback? onTap;

  @override
  State<StaticBlock> createState() => _StaticBlockState();
}

class _StaticBlockState extends State<StaticBlock> {
  RenderedBlock? _r;

  @override
  void dispose() {
    _r?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    return LayoutBuilder(
      builder: (context, box) => ListenableBuilder(
        listenable: app.refs,
        builder: (context, _) {
          _r?.dispose();
          _r = BlockRenderer(
            RenderContext(
              colors: context.colors,
              base: blockTextStyle(context),
              refs: app.refs,
              assetPath: app.backend.assetPath,
              onPage: app.openTitle,
              onBlockRef: (r) => app.openId(r.pageId),
              onUrl: openUrl,
              maxWidth: box.maxWidth,
            ),
          ).render(widget.content);
          return GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: widget.onTap,
            child: Text.rich(_r!.span),
          );
        },
      ),
    );
  }
}

/// Bullet dot used outside the editor.
class Dot extends StatelessWidget {
  const Dot({super.key});

  @override
  Widget build(BuildContext context) => SizedBox(
    width: 18,
    height: blockFontSize * blockLineHeight,
    child: Center(
      child: Container(
        width: 5,
        height: 5,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: context.colors.faint,
        ),
      ),
    ),
  );
}
