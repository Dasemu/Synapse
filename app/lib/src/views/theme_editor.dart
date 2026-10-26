import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../state/app_state.dart';
import '../theme/theme.dart';

/// Opens the editor for custom theme `theme`. Edits apply live (the app re-themes as you
/// type); Cancel restores the values the theme had when the editor opened, and for a theme
/// that was just created it deletes it again and reselects `previousThemeId`.
Future<void> showThemeEditor(
  BuildContext context,
  AppTheme theme, {
  bool isNew = false,
  String previousThemeId = autoThemeId,
}) {
  final narrow = MediaQuery.sizeOf(context).width < 640;
  return showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => Dialog(
      insetPadding: narrow
          ? EdgeInsets.zero
          : const EdgeInsets.symmetric(horizontal: 40, vertical: 24),
      shape: narrow ? const RoundedRectangleBorder() : null,
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: narrow ? double.infinity : 640,
          maxHeight: narrow ? double.infinity : 760,
        ),
        child: ThemeEditor(
          original: theme,
          isNew: isNew,
          previousThemeId: previousThemeId,
        ),
      ),
    ),
  );
}

class ThemeEditor extends StatefulWidget {
  const ThemeEditor({
    super.key,
    required this.original,
    this.isNew = false,
    this.previousThemeId = autoThemeId,
  });

  final AppTheme original;
  final bool isNew;
  final String previousThemeId;

  @override
  State<ThemeEditor> createState() => _ThemeEditorState();
}

class _ThemeEditorState extends State<ThemeEditor> {
  late AppTheme _theme = widget.original;
  late final _name = TextEditingController(text: widget.original.label);
  late final _css = TextEditingController(text: themeToCss(widget.original));
  List<String> _errors = const [];
  bool _cssTab = false;

  @override
  void dispose() {
    _name.dispose();
    _css.dispose();
    super.dispose();
  }

  void _set(AppTheme t, {bool refreshCss = true}) {
    setState(() => _theme = t);
    if (refreshCss) _css.text = themeToCss(t);
    AppScope.read(context).updateCustomTheme(t);
  }

  void _cancel() {
    final app = AppScope.read(context);
    if (widget.isNew) {
      app.deleteCustomTheme(widget.original.id);
      app.setTheme(widget.previousThemeId);
    } else {
      app.updateCustomTheme(widget.original);
      app.setTheme(widget.previousThemeId);
    }
    Navigator.of(context).pop();
  }

  void _applyCss() {
    final p = parseThemeCss(_css.text, fallback: _theme.colors);
    setState(() => _errors = p.errors);
    _set(
      _theme.copyWith(
        colors: p.colors,
        brightness: p.brightness ?? _theme.brightness,
      ),
      refreshCss: p.errors.isEmpty,
    );
  }

  Future<void> _pick(ColorToken tok) async {
    final before = _theme.colors.of(tok);
    final c = await showDialog<Color>(
      context: context,
      builder: (_) => _ColorPickerDialog(
        token: tok,
        initial: _theme.colors.of(tok),
        onChanged: (c) =>
            _set(_theme.copyWith(colors: _theme.colors.withToken(tok, c))),
      ),
    );
    // Dismissed (Esc, tap outside, back): undo the live preview.
    _set(_theme.copyWith(colors: _theme.colors.withToken(tok, c ?? before)));
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    // System back (Android) cancels, like the Cancel button.
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _cancel();
      },
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              widget.isNew ? 'New theme' : 'Edit theme',
              style: Theme.of(context).textTheme.headlineSmall,
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    key: const ValueKey('theme-name'),
                    controller: _name,
                    decoration: const InputDecoration(
                      labelText: 'Name',
                      isDense: true,
                      border: OutlineInputBorder(),
                    ),
                    onChanged: (v) {
                      final label = v.trim().isEmpty ? 'Custom' : v.trim();
                      _set(_theme.copyWith(label: label));
                    },
                  ),
                ),
                const SizedBox(width: 12),
                SegmentedButton<Brightness>(
                  key: const ValueKey('theme-brightness'),
                  showSelectedIcon: false,
                  segments: const [
                    ButtonSegment(
                      value: Brightness.light,
                      icon: Icon(Icons.light_mode_outlined, size: 16),
                      label: Text('Light'),
                    ),
                    ButtonSegment(
                      value: Brightness.dark,
                      icon: Icon(Icons.dark_mode_outlined, size: 16),
                      label: Text('Dark'),
                    ),
                  ],
                  selected: {_theme.brightness},
                  onSelectionChanged: (s) =>
                      _set(_theme.copyWith(brightness: s.first)),
                ),
              ],
            ),
            const SizedBox(height: 12),
            _Preview(colors: _theme.colors),
            const SizedBox(height: 12),
            SegmentedButton<bool>(
              showSelectedIcon: false,
              segments: const [
                ButtonSegment(value: false, label: Text('Colors')),
                ButtonSegment(value: true, label: Text('CSS')),
              ],
              selected: {_cssTab},
              onSelectionChanged: (s) => setState(() => _cssTab = s.first),
            ),
            const SizedBox(height: 8),
            Expanded(child: _cssTab ? _cssPane(c) : _colorsPane(c)),
            const SizedBox(height: 8),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton(
                  key: const ValueKey('theme-cancel'),
                  onPressed: _cancel,
                  child: const Text('Cancel'),
                ),
                const SizedBox(width: 8),
                FilledButton(
                  key: const ValueKey('theme-save'),
                  onPressed: () => Navigator.of(context).pop(),
                  child: const Text('Save'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _colorsPane(SynapseColors c) => ListView(
    children: [
      for (final tok in colorTokens)
        ListTile(
          key: ValueKey('token-${tok.name}'),
          dense: true,
          contentPadding: EdgeInsets.zero,
          onTap: () => _pick(tok),
          leading: _Swatch(_theme.colors.of(tok), c.line),
          title: Text(tok.label),
          subtitle: Text(
            tok.hint,
            style: TextStyle(color: c.muted, fontSize: 12),
          ),
          trailing: Text(
            colorToHex(_theme.colors.of(tok)),
            style: TextStyle(
              fontFamily: monoFont,
              fontSize: 12,
              color: c.muted,
            ),
          ),
        ),
    ],
  );

  Widget _cssPane(SynapseColors c) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Expanded(
        child: TextField(
          key: const ValueKey('theme-css'),
          controller: _css,
          expands: true,
          maxLines: null,
          minLines: null,
          textAlignVertical: TextAlignVertical.top,
          style: const TextStyle(fontFamily: monoFont, fontSize: 12.5),
          decoration: const InputDecoration(
            border: OutlineInputBorder(),
            isDense: true,
          ),
        ),
      ),
      if (_errors.isNotEmpty)
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text(
            _errors.join('\n'),
            key: const ValueKey('theme-css-errors'),
            style: TextStyle(color: c.ref, fontSize: 12),
          ),
        ),
      const SizedBox(height: 8),
      Row(
        children: [
          OutlinedButton(
            key: const ValueKey('theme-css-copy'),
            onPressed: () => Clipboard.setData(ClipboardData(text: _css.text)),
            child: const Text('Copy'),
          ),
          const SizedBox(width: 8),
          FilledButton.tonal(
            key: const ValueKey('theme-css-apply'),
            onPressed: _applyCss,
            child: const Text('Apply'),
          ),
        ],
      ),
    ],
  );
}

class _Swatch extends StatelessWidget {
  const _Swatch(this.color, this.border);
  final Color color;
  final Color border;

  @override
  Widget build(BuildContext context) => Container(
    width: 32,
    height: 32,
    decoration: BoxDecoration(
      borderRadius: BorderRadius.circular(8),
      border: Border.all(color: border),
    ),
    child: ClipRRect(
      borderRadius: BorderRadius.circular(7),
      child: Stack(
        fit: StackFit.expand,
        children: [
          // Checker-ish backdrop so translucent colours are visible.
          Row(
            children: [
              for (final g in const [0xFFFFFFFF, 0xFF999999])
                Expanded(child: ColoredBox(color: Color(g))),
            ],
          ),
          ColoredBox(color: color),
        ],
      ),
    ),
  );
}

/// A mock page drawn with the edited colours.
class _Preview extends StatelessWidget {
  const _Preview({required this.colors});
  final SynapseColors colors;

  @override
  Widget build(BuildContext context) {
    final c = colors;
    TextStyle s({Color? color, FontWeight? w}) => TextStyle(
      fontSize: 13.5,
      color: color ?? c.ink,
      fontWeight: w,
      height: 1.4,
    );
    Widget bullet(List<InlineSpan> spans) => Padding(
      padding: const EdgeInsets.only(top: 3),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 7, right: 8),
            child: CircleAvatar(radius: 2.5, backgroundColor: c.faint),
          ),
          Expanded(
            child: Text.rich(TextSpan(children: spans, style: s())),
          ),
        ],
      ),
    );
    return Container(
      key: const ValueKey('theme-preview'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: c.paper,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: c.line),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Preview page',
            style: TextStyle(
              fontFamily: titleFont,
              fontSize: 17,
              fontWeight: FontWeight.w600,
              color: c.ink,
            ),
          ),
          Text('Secondary text', style: s(color: c.muted)),
          bullet([
            const TextSpan(text: 'A link to '),
            TextSpan(
              text: '[[Page]]',
              style: TextStyle(color: c.accent, backgroundColor: c.accentSoft),
            ),
            const TextSpan(text: ' and '),
            TextSpan(
              text: 'marked',
              style: TextStyle(backgroundColor: c.highlight),
            ),
          ]),
          bullet([
            TextSpan(
              text: '((block reference))',
              style: TextStyle(color: c.ref),
            ),
          ]),
          bullet([
            TextSpan(
              text: 'Done task',
              style: TextStyle(
                color: c.done,
                decoration: TextDecoration.lineThrough,
              ),
            ),
          ]),
          const SizedBox(height: 6),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
            color: c.codeBg,
            child: Text(
              'let x = 1;',
              style: TextStyle(
                fontFamily: monoFont,
                fontSize: 12,
                color: c.ink,
              ),
            ),
          ),
          const SizedBox(height: 6),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
            color: c.surface,
            child: Text('Sidebar surface', style: s(color: c.muted)),
          ),
        ],
      ),
    );
  }
}

const _presets = <Color>[
  Color(0xFFFFFFFF),
  Color(0xFFFAF8F4),
  Color(0xFF23262B),
  Color(0xFF16181B),
  Color(0xFF000000),
  Color(0xFFE5484D),
  Color(0xFFF76B15),
  Color(0xFFF5B800),
  Color(0xFF30A46C),
  Color(0xFF1F7A74),
  Color(0xFF3E63DD),
  Color(0xFF8E4EC6),
  Color(0xFFD05CE3),
  Color(0xFF8E93AB),
];

/// A dependency-free colour picker: hue, saturation, lightness and (optionally) alpha
/// sliders, a hex field and preset swatches. `onChanged` fires on every change so the app
/// can preview live; the dialog pops the final colour on Done.
class _ColorPickerDialog extends StatefulWidget {
  const _ColorPickerDialog({
    required this.token,
    required this.initial,
    required this.onChanged,
  });

  final ColorToken token;
  final Color initial;
  final ValueChanged<Color> onChanged;

  @override
  State<_ColorPickerDialog> createState() => _ColorPickerDialogState();
}

class _ColorPickerDialogState extends State<_ColorPickerDialog> {
  late HSLColor _hsl = HSLColor.fromColor(widget.initial);
  late final _hex = TextEditingController(text: colorToHex(widget.initial));

  @override
  void dispose() {
    _hex.dispose();
    super.dispose();
  }

  void _update(HSLColor h, {bool syncHex = true}) {
    setState(() => _hsl = h);
    final c = h.toColor();
    if (syncHex) _hex.text = colorToHex(c);
    widget.onChanged(c);
  }

  Widget _slider(
    String label,
    double value,
    double max,
    ValueChanged<double> onChanged,
  ) => Row(
    children: [
      SizedBox(width: 24, child: Text(label)),
      Expanded(
        child: Slider(
          value: value.clamp(0, max),
          max: max,
          onChanged: onChanged,
        ),
      ),
    ],
  );

  @override
  Widget build(BuildContext context) {
    final tok = widget.token;
    return AlertDialog(
      title: Text(tok.label),
      content: SizedBox(
        width: 360,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(tok.hint),
              const SizedBox(height: 10),
              Container(
                height: 36,
                decoration: BoxDecoration(
                  color: _hsl.toColor(),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(color: context.colors.line),
                ),
              ),
              _slider('H', _hsl.hue, 360, (v) => _update(_hsl.withHue(v))),
              _slider(
                'S',
                _hsl.saturation,
                1,
                (v) => _update(_hsl.withSaturation(v)),
              ),
              _slider(
                'L',
                _hsl.lightness,
                1,
                (v) => _update(_hsl.withLightness(v)),
              ),
              if (tok.alpha)
                _slider('A', _hsl.alpha, 1, (v) => _update(_hsl.withAlpha(v))),
              TextField(
                key: const ValueKey('picker-hex'),
                controller: _hex,
                style: const TextStyle(fontFamily: monoFont),
                decoration: const InputDecoration(
                  labelText: 'Hex',
                  isDense: true,
                  border: OutlineInputBorder(),
                ),
                onChanged: (v) {
                  final c = parseCssColor(v);
                  if (c != null) _update(HSLColor.fromColor(c), syncHex: false);
                },
              ),
              const SizedBox(height: 10),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final p in _presets)
                    InkWell(
                      onTap: () => _update(
                        HSLColor.fromColor(p)
                            .withAlpha(tok.alpha ? _hsl.alpha : 1),
                      ),
                      child: _Swatch(p, context.colors.line),
                    ),
                ],
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () {
            widget.onChanged(widget.initial);
            Navigator.of(context).pop();
          },
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_hsl.toColor()),
          child: const Text('Done'),
        ),
      ],
    );
  }
}
