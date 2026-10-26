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
  SynapseColors copyWith() => this;

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

/// The theme with `id`, or null for `auto` and unknown ids.
AppTheme? themeById(String id) {
  for (final t in appThemes) {
    if (t.id == id) return t;
  }
  return null;
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
