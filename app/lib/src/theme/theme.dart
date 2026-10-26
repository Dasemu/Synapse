import 'package:flutter/material.dart';

/// Synapse's design tokens.
///
/// Typography: Inter for UI and note text (designed for screens, clear at small sizes, wide
/// language coverage), Noto Serif for page and journal titles (a quiet, bookish voice that
/// separates "the document" from the chrome), Source Code Pro for code. All three are bundled
/// (OFL) so the app looks the same offline on every platform.
///
/// Colour: warm paper neutrals instead of pure white/black to keep long reading calm, one
/// accent (deep teal) for links and focus, amber for block references and a muted green for
/// done tasks. Dark mode mirrors the same hues at lower saturation.
@immutable
class SynapseColors extends ThemeExtension<SynapseColors> {
  const SynapseColors({
    required this.paper,
    required this.surface,
    required this.ink,
    required this.muted,
    required this.faint,
    required this.line,
    required this.accent,
    required this.accentSoft,
    required this.ref,
    required this.done,
    required this.codeBg,
    required this.highlight,
  });

  final Color paper;
  final Color surface;
  final Color ink;
  final Color muted;
  final Color faint;
  final Color line;
  final Color accent;
  final Color accentSoft;
  final Color ref;
  final Color done;
  final Color codeBg;
  final Color highlight;

  static const light = SynapseColors(
    paper: Color(0xFFFAF8F4),
    surface: Color(0xFFF3F0EA),
    ink: Color(0xFF23262B),
    muted: Color(0xFF6B6F76),
    faint: Color(0xFFA9A69F),
    line: Color(0xFFE4E0D8),
    accent: Color(0xFF1F7A74),
    accentSoft: Color(0x1A1F7A74),
    ref: Color(0xFFB0741E),
    done: Color(0xFF6F8F5E),
    codeBg: Color(0xFFEFEBE3),
    highlight: Color(0x66F3D86B),
  );

  static const dark = SynapseColors(
    paper: Color(0xFF16181B),
    surface: Color(0xFF1D2024),
    ink: Color(0xFFE4E2DD),
    muted: Color(0xFF9A9FA6),
    faint: Color(0xFF61666D),
    line: Color(0xFF2B2F35),
    accent: Color(0xFF63C2B8),
    accentSoft: Color(0x2263C2B8),
    ref: Color(0xFFD9A55A),
    done: Color(0xFF8DAF7B),
    codeBg: Color(0xFF23272C),
    highlight: Color(0x55B89B2E),
  );

  /// "Sweet" (the KDE/GTK theme): deep indigo surfaces, magenta accent, mint for done tasks.
  static const sweet = SynapseColors(
    paper: Color(0xFF1B1D2B),
    surface: Color(0xFF151722),
    ink: Color(0xFFC9CDDA),
    muted: Color(0xFF8E93AB),
    faint: Color(0xFF555A78),
    line: Color(0xFF2A2D42),
    accent: Color(0xFFD05CE3),
    accentSoft: Color(0x33D05CE3),
    ref: Color(0xFFF2A65A),
    done: Color(0xFF4DDBA7),
    codeBg: Color(0xFF12141D),
    highlight: Color(0x55D05CE3),
  );

  static const sweetLight = SynapseColors(
    paper: Color(0xFFF8F6FB),
    surface: Color(0xFFEFEBF5),
    ink: Color(0xFF2B2D42),
    muted: Color(0xFF6A6E88),
    faint: Color(0xFFA8ABC0),
    line: Color(0xFFE1DCEB),
    accent: Color(0xFFB21FC2),
    accentSoft: Color(0x1AB21FC2),
    ref: Color(0xFFC77A1A),
    done: Color(0xFF2FA37A),
    codeBg: Color(0xFFEBE6F2),
    highlight: Color(0x66F2C26B),
  );

  static const nord = SynapseColors(
    paper: Color(0xFF2E3440),
    surface: Color(0xFF272C36),
    ink: Color(0xFFD8DEE9),
    muted: Color(0xFF9AA5B8),
    faint: Color(0xFF616E88),
    line: Color(0xFF3B4252),
    accent: Color(0xFF88C0D0),
    accentSoft: Color(0x2288C0D0),
    ref: Color(0xFFEBCB8B),
    done: Color(0xFFA3BE8C),
    codeBg: Color(0xFF242933),
    highlight: Color(0x55EBCB8B),
  );

  @override
  SynapseColors copyWith({
    Color? paper,
    Color? surface,
    Color? ink,
    Color? muted,
    Color? faint,
    Color? line,
    Color? accent,
    Color? accentSoft,
    Color? ref,
    Color? done,
    Color? codeBg,
    Color? highlight,
  }) => SynapseColors(
    paper: paper ?? this.paper,
    surface: surface ?? this.surface,
    ink: ink ?? this.ink,
    muted: muted ?? this.muted,
    faint: faint ?? this.faint,
    line: line ?? this.line,
    accent: accent ?? this.accent,
    accentSoft: accentSoft ?? this.accentSoft,
    ref: ref ?? this.ref,
    done: done ?? this.done,
    codeBg: codeBg ?? this.codeBg,
    highlight: highlight ?? this.highlight,
  );

  /// The colour of `token`.
  Color of(ColorToken token) => token.get(this);

  /// `copyWith` for one token chosen at runtime.
  SynapseColors withToken(ColorToken token, Color c) => switch (token.name) {
    'paper' => copyWith(paper: c),
    'surface' => copyWith(surface: c),
    'ink' => copyWith(ink: c),
    'muted' => copyWith(muted: c),
    'faint' => copyWith(faint: c),
    'line' => copyWith(line: c),
    'accent' => copyWith(accent: c),
    'accentSoft' => copyWith(accentSoft: c),
    'ref' => copyWith(ref: c),
    'done' => copyWith(done: c),
    'codeBg' => copyWith(codeBg: c),
    _ => copyWith(highlight: c),
  };

  /// `{token: '#rrggbbaa'}` (CSS order, always 8 digits) for persistence.
  Map<String, String> toMap() => {
    for (final t in colorTokens) t.name: colorToHex(of(t), always8: true),
  };

  /// Reads `toMap` output; missing or unparsable tokens come from `fallback`.
  static SynapseColors fromMap(
    Map<dynamic, dynamic> map,
    SynapseColors fallback,
  ) {
    var out = fallback;
    for (final t in colorTokens) {
      final v = map[t.name];
      final c = v is String ? parseCssColor(v) : null;
      if (c != null) out = out.withToken(t, c);
    }
    return out;
  }

  @override
  bool operator ==(Object other) =>
      other is SynapseColors && colorTokens.every((t) => of(t) == other.of(t));

  @override
  int get hashCode => Object.hashAll(colorTokens.map(of));

  @override
  SynapseColors lerp(SynapseColors? other, double t) {
    if (other == null) return this;
    Color l(Color a, Color b) => Color.lerp(a, b, t)!;
    return SynapseColors(
      paper: l(paper, other.paper),
      surface: l(surface, other.surface),
      ink: l(ink, other.ink),
      muted: l(muted, other.muted),
      faint: l(faint, other.faint),
      line: l(line, other.line),
      accent: l(accent, other.accent),
      accentSoft: l(accentSoft, other.accentSoft),
      ref: l(ref, other.ref),
      done: l(done, other.done),
      codeBg: l(codeBg, other.codeBg),
      highlight: l(highlight, other.highlight),
    );
  }
}

const uiFont = 'Inter';
const titleFont = 'NotoSerif';
const monoFont = 'SourceCodePro';

/// Text metrics of the outliner, shared by rendering and editing so a block does not jump
/// when it switches between the two.
const blockFontSize = 15.0;
const blockLineHeight = 1.55;

extension SynapseThemeX on BuildContext {
  SynapseColors get colors => Theme.of(this).extension<SynapseColors>()!;
}

/// A selectable palette. `auto` follows the system between Paper light and Paper dark.
class AppTheme {
  const AppTheme(this.id, this.label, this.brightness, this.colors);
  final String id;
  final String label;
  final Brightness brightness;
  final SynapseColors colors;
}

extension AppThemeX on AppTheme {
  AppTheme copyWith({
    String? label,
    Brightness? brightness,
    SynapseColors? colors,
  }) => AppTheme(
    id,
    label ?? this.label,
    brightness ?? this.brightness,
    colors ?? this.colors,
  );

  Map<String, dynamic> toJson() => {
    'id': id,
    'label': label,
    'brightness': brightness == Brightness.dark ? 'dark' : 'light',
    'colors': colors.toMap(),
  };
}

/// Reads a persisted custom theme; null for corrupt entries. Missing tokens come from the
/// base palette of the theme's brightness.
AppTheme? customThemeFromJson(Object? json) {
  if (json is! Map) return null;
  final id = json['id'];
  final label = json['label'];
  final b = json['brightness'];
  final colors = json['colors'];
  if (id is! String ||
      id.isEmpty ||
      id == autoThemeId ||
      themeById(id) != null) {
    return null;
  }
  if (label is! String || (b != 'light' && b != 'dark') || colors is! Map) {
    return null;
  }
  final brightness = b == 'dark' ? Brightness.dark : Brightness.light;
  return AppTheme(
    id,
    label,
    brightness,
    SynapseColors.fromMap(colors, baseColors(brightness)),
  );
}

const autoThemeId = 'auto';

const appThemes = [
  AppTheme('paper', 'Paper', Brightness.light, SynapseColors.light),
  AppTheme('paper-dark', 'Paper dark', Brightness.dark, SynapseColors.dark),
  AppTheme('sweet', 'Sweet', Brightness.dark, SynapseColors.sweet),
  AppTheme(
    'sweet-light',
    'Sweet light',
    Brightness.light,
    SynapseColors.sweetLight,
  ),
  AppTheme('nord', 'Nord', Brightness.dark, SynapseColors.nord),
];

/// The built-in theme with `id`, or null for `auto`, custom and unknown ids.
AppTheme? themeById(String id) {
  for (final t in appThemes) {
    if (t.id == id) return t;
  }
  return null;
}

/// A colour token: its field name, CSS variable, UI label and hint.
class ColorToken {
  const ColorToken(
    this.name,
    this.css,
    this.label,
    this.hint,
    this.get, {
    this.alpha = false,
  });
  final String name;
  final String css;
  final String label;
  final String hint;
  final Color Function(SynapseColors) get;

  /// Whether the token is meant to be translucent (the picker shows an alpha slider).
  final bool alpha;
}

final colorTokens = <ColorToken>[
  ColorToken('paper', 'paper', 'Paper', 'Page background', (c) => c.paper),
  ColorToken(
    'surface',
    'surface',
    'Surface',
    'Sidebar and panels',
    (c) => c.surface,
  ),
  ColorToken('ink', 'ink', 'Ink', 'Main text', (c) => c.ink),
  ColorToken('muted', 'muted', 'Muted', 'Secondary text', (c) => c.muted),
  ColorToken(
    'faint',
    'faint',
    'Faint',
    'Bullets, placeholders, icons',
    (c) => c.faint,
  ),
  ColorToken('line', 'line', 'Line', 'Borders and dividers', (c) => c.line),
  ColorToken(
    'accent',
    'accent',
    'Accent',
    'Links, focus, selection',
    (c) => c.accent,
  ),
  ColorToken(
    'accentSoft',
    'accent-soft',
    'Accent soft',
    'Tinted backgrounds behind links',
    (c) => c.accentSoft,
    alpha: true,
  ),
  ColorToken('ref', 'ref', 'Reference', 'Block references', (c) => c.ref),
  ColorToken('done', 'done', 'Done', 'Completed tasks', (c) => c.done),
  ColorToken(
    'codeBg',
    'code-bg',
    'Code background',
    'Inline code and code blocks',
    (c) => c.codeBg,
  ),
  ColorToken(
    'highlight',
    'highlight',
    'Highlight',
    'Marked text and search hits',
    (c) => c.highlight,
    alpha: true,
  ),
];

/// The built-in base palette for `brightness`.
SynapseColors baseColors(Brightness brightness) =>
    brightness == Brightness.light ? SynapseColors.light : SynapseColors.dark;

String _hex2(int v) => v.toRadixString(16).padLeft(2, '0');

/// `#rrggbb`, or `#rrggbbaa` when not opaque (CSS order); `always8` forces the alpha digits.
String colorToHex(Color c, {bool always8 = false}) {
  final v = c.toARGB32();
  final a = (v >> 24) & 0xFF;
  final rgb =
      '${_hex2((v >> 16) & 0xFF)}${_hex2((v >> 8) & 0xFF)}${_hex2(v & 0xFF)}';
  return a == 0xFF && !always8 ? '#$rgb' : '#$rgb${_hex2(a)}';
}

double? _num(String s, {double scale = 1}) {
  s = s.trim();
  if (s.endsWith('%')) {
    final v = double.tryParse(s.substring(0, s.length - 1));
    return v == null ? null : v / 100 * scale;
  }
  return double.tryParse(s);
}

double? _alpha(String? s) {
  if (s == null) return 1;
  final v = _num(s);
  return v?.clamp(0.0, 1.0);
}

/// Parses a CSS colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()/rgba()` and
/// `hsl()/hsla()` (comma or space syntax). Returns null when it is not a valid colour.
Color? parseCssColor(String input) {
  final s = input.trim().toLowerCase();
  if (s.startsWith('#')) {
    var h = s.substring(1);
    if (!RegExp(r'^[0-9a-f]+$').hasMatch(h)) return null;
    if (h.length == 3 || h.length == 4) {
      h = h.split('').map((c) => '$c$c').join();
    }
    if (h.length != 6 && h.length != 8) return null;
    final rgb = int.parse(h.substring(0, 6), radix: 16);
    final a = h.length == 8 ? int.parse(h.substring(6), radix: 16) : 0xFF;
    return Color((a << 24) | rgb);
  }
  final m = RegExp(r'^(rgba?|hsla?)\(([^)]*)\)$').matchAsPrefix(s);
  if (m == null) return null;
  final parts = m
      .group(2)!
      .split(RegExp(r'[\s,/]+'))
      .where((p) => p.isNotEmpty)
      .toList();
  if (parts.length != 3 && parts.length != 4) return null;
  final a = _alpha(parts.length == 4 ? parts[3] : null);
  if (a == null) return null;
  if (m.group(1)!.startsWith('rgb')) {
    final v = [for (var i = 0; i < 3; i++) _num(parts[i], scale: 255)];
    if (v.contains(null)) return null;
    int ch(double? x) => x!.round().clamp(0, 255);
    return Color.fromARGB((a * 255).round(), ch(v[0]), ch(v[1]), ch(v[2]));
  }
  final hue = double.tryParse(parts[0].replaceAll(RegExp(r'deg$'), ''));
  final sat = _num(parts[1]);
  final lig = _num(parts[2]);
  if (hue == null || sat == null || lig == null) return null;
  // `_num` leaves bare numbers as-is: treat "50" as 50 %, like "50%" (0..1 after scaling).
  double pct(String p, double v) => p.trim().endsWith('%') ? v : v / 100;
  return HSLColor.fromAHSL(
    a,
    ((hue % 360) + 360) % 360,
    pct(parts[1], sat).clamp(0.0, 1.0),
    pct(parts[2], lig).clamp(0.0, 1.0),
  ).toColor();
}

/// The result of `parseThemeCss`.
class ParsedTheme {
  const ParsedTheme(this.brightness, this.colors, this.errors);

  /// From `color-scheme`, or null when the CSS does not say.
  final Brightness? brightness;
  final SynapseColors colors;

  /// Human-readable problems; the offending lines were skipped.
  final List<String> errors;
}

String _norm(String name) => name.toLowerCase().replaceAll(RegExp(r'[-_]'), '');

/// Serialises a theme as CSS custom properties (see `parseThemeCss`).
String themeToCss(AppTheme t) {
  final b = StringBuffer(
    '/* Synapse theme: ${t.label.replaceAll('*/', '')} */\n:root {\n',
  );
  b.writeln(
    '  color-scheme: ${t.brightness == Brightness.dark ? 'dark' : 'light'};',
  );
  for (final tok in colorTokens) {
    b.writeln('  --${tok.css}: ${colorToHex(t.colors.of(tok))};');
  }
  b.write('}\n');
  return b.toString();
}

/// Reads `--token: colour;` declarations and `color-scheme`. Comments, selectors and braces
/// are ignored; tokens not mentioned keep their value from `fallback`.
ParsedTheme parseThemeCss(String css, {required SynapseColors fallback}) {
  final text = css.replaceAll(RegExp(r'/\*.*?\*/', dotAll: true), '');
  final byName = {for (final t in colorTokens) _norm(t.name): t};
  var colors = fallback;
  Brightness? brightness;
  final errors = <String>[];
  for (final m in RegExp(
    r'(--[\w-]+|color-scheme)\s*:\s*([^;{}]*)',
  ).allMatches(text)) {
    final name = m.group(1)!;
    final value = m.group(2)!.trim();
    if (name == 'color-scheme') {
      // The first scheme listed is the preferred one (`light dark` means light).
      final v = value.toLowerCase().trim().split(RegExp(r'\s+')).first;
      if (v == 'dark') {
        brightness = Brightness.dark;
      } else if (v == 'light') {
        brightness = Brightness.light;
      } else {
        errors.add('Unknown color-scheme "$value" (use light or dark)');
      }
      continue;
    }
    final tok = byName[_norm(name.substring(2))];
    if (tok == null) {
      errors.add('Unknown variable $name');
      continue;
    }
    final c = parseCssColor(value);
    if (c == null) {
      errors.add('Bad colour for $name: "$value"');
      continue;
    }
    colors = colors.withToken(tok, c);
  }
  return ParsedTheme(brightness, colors, errors);
}

ThemeData buildTheme(Brightness brightness, {SynapseColors? colors}) {
  final c =
      colors ??
      (brightness == Brightness.light
          ? SynapseColors.light
          : SynapseColors.dark);
  final scheme =
      ColorScheme.fromSeed(
        seedColor: c.accent,
        brightness: brightness,
      ).copyWith(
        primary: c.accent,
        surface: c.paper,
        onSurface: c.ink,
        surfaceContainerLowest: c.paper,
        surfaceContainerLow: c.surface,
        surfaceContainer: c.surface,
        surfaceContainerHigh: c.surface,
        outline: c.line,
        outlineVariant: c.line,
      );
  final base = ThemeData(
    useMaterial3: true,
    brightness: brightness,
    colorScheme: scheme,
    fontFamily: uiFont,
    scaffoldBackgroundColor: c.paper,
    canvasColor: c.paper,
    dividerColor: c.line,
    splashFactory: NoSplash.splashFactory,
    visualDensity: VisualDensity.compact,
    extensions: [c],
  );
  final t = base.textTheme.apply(bodyColor: c.ink, displayColor: c.ink);
  return base.copyWith(
    textTheme: t.copyWith(
      bodyMedium: t.bodyMedium?.copyWith(
        fontSize: blockFontSize,
        height: blockLineHeight,
      ),
      headlineLarge: TextStyle(
        fontFamily: titleFont,
        fontSize: 30,
        fontWeight: FontWeight.w600,
        height: 1.25,
        color: c.ink,
        letterSpacing: -0.2,
      ),
      headlineSmall: TextStyle(
        fontFamily: titleFont,
        fontSize: 22,
        fontWeight: FontWeight.w600,
        height: 1.3,
        color: c.ink,
      ),
      labelSmall: TextStyle(
        fontFamily: uiFont,
        fontSize: 11,
        fontWeight: FontWeight.w600,
        letterSpacing: 0.8,
        color: c.muted,
      ),
    ),
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: c.accent,
      selectionColor: c.accentSoft.withAlpha(0x55),
      selectionHandleColor: c.accent,
    ),
    tooltipTheme: TooltipThemeData(
      waitDuration: const Duration(milliseconds: 500),
      decoration: BoxDecoration(
        color: c.ink,
        borderRadius: BorderRadius.circular(6),
      ),
      textStyle: TextStyle(color: c.paper, fontSize: 12),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: c.paper,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(14),
        side: BorderSide(color: c.line),
      ),
    ),
    scrollbarTheme: ScrollbarThemeData(
      thickness: const WidgetStatePropertyAll(6),
      radius: const Radius.circular(3),
      thumbColor: WidgetStatePropertyAll(c.faint.withAlpha(0x66)),
    ),
  );
}
