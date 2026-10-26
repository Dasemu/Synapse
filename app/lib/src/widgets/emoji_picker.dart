import 'package:flutter/material.dart';

import '../theme/theme.dart';

/// Curated emoji, grouped (no emoji package: a short list covers the usual types, and any
/// other emoji can be pasted).
const emojiGroups = <(String, List<String>)>[
  (
    'People',
    [
      '🧑',
      '👤',
      '👥',
      '👩',
      '👨',
      '🧒',
      '👪',
      '🧑‍💼',
      '🧑‍🔬',
      '🧑‍🎨',
      '🧑‍🏫',
      '🧑‍💻',
      '🧑‍⚕️',
      '🤝',
      '💬',
      '🫂',
    ],
  ),
  (
    'Places',
    [
      '📍',
      '🏠',
      '🏢',
      '🏛️',
      '🏙️',
      '🏞️',
      '🗺️',
      '🌍',
      '✈️',
      '🏫',
      '🏥',
      '🏪',
      '⛪',
      '🏰',
      '🏖️',
      '⛰️',
    ],
  ),
  (
    'Things',
    [
      '📕',
      '📚',
      '📄',
      '📝',
      '📎',
      '💻',
      '📱',
      '🖥️',
      '⚙️',
      '🔧',
      '🧰',
      '💡',
      '🔑',
      '📦',
      '🎁',
      '🗂️',
    ],
  ),
  (
    'Ideas',
    [
      '💭',
      '🧠',
      '💡',
      '❓',
      '❗',
      '🎯',
      '🧩',
      '🔬',
      '🧪',
      '📐',
      '📊',
      '📈',
      '🔍',
      '✨',
      '🌀',
      '♾️',
    ],
  ),
  (
    'Nature',
    [
      '🌱',
      '🌿',
      '🌳',
      '🌸',
      '🍄',
      '🌊',
      '🔥',
      '⭐',
      '🌙',
      '☀️',
      '⚡',
      '❄️',
      '🐱',
      '🐶',
      '🐦',
      '🦋',
    ],
  ),
  (
    'Food',
    [
      '🍳',
      '🥗',
      '🍝',
      '🍕',
      '🍰',
      '🍎',
      '🥖',
      '🧀',
      '☕',
      '🍵',
      '🍷',
      '🍺',
      '🥘',
      '🌮',
      '🍣',
      '🥕',
    ],
  ),
  (
    'Activities',
    [
      '📅',
      '🎉',
      '🎵',
      '🎬',
      '🎮',
      '🎨',
      '📷',
      '⚽',
      '🏃',
      '🧘',
      '✍️',
      '🎤',
      '🎓',
      '💼',
      '🛠️',
      '🧳',
    ],
  ),
  (
    'Symbols',
    [
      '❤️',
      '⭐',
      '✅',
      '⚠️',
      '🔖',
      '🏷️',
      '📌',
      '🔗',
      '💰',
      '⏳',
      '🔒',
      '🚩',
      '🔴',
      '🟢',
      '🔵',
      '🟣',
    ],
  ),
];

/// True when [s] (trimmed) is one emoji: a single user-perceived character that is not plain
/// ASCII (so letters, digits and punctuation are refused).
bool isSingleEmoji(String s) {
  final t = s.trim();
  return t.isNotEmpty &&
      t.characters.length == 1 &&
      t.runes.any((r) => r > 0x7f) &&
      !t.contains(RegExp(r'\s'));
}

/// What the picker returned: an emoji, or `null` to clear the icon.
class EmojiChoice {
  const EmojiChoice(this.emoji);
  final String? emoji;
}

/// Lets the user pick an emoji from [emojiGroups], paste any other one, or clear it.
/// Returns `null` when cancelled.
Future<EmojiChoice?> showEmojiPicker(
  BuildContext context, {
  required String title,
  String? current,
}) => showDialog<EmojiChoice>(
  context: context,
  builder: (_) => _EmojiPicker(title: title, current: current),
);

class _EmojiPicker extends StatefulWidget {
  const _EmojiPicker({required this.title, this.current});
  final String title;
  final String? current;

  @override
  State<_EmojiPicker> createState() => _EmojiPickerState();
}

class _EmojiPickerState extends State<_EmojiPicker> {
  final _text = TextEditingController();
  String? _error;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  void _usePasted() {
    final t = _text.text.trim();
    if (t.isEmpty) return;
    if (!isSingleEmoji(t)) {
      setState(() => _error = 'Enter a single emoji');
      return;
    }
    Navigator.pop(context, EmojiChoice(t));
  }

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return AlertDialog(
      title: Text(widget.title),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              key: const ValueKey('emoji-input'),
              controller: _text,
              decoration: InputDecoration(
                hintText: 'Paste any emoji',
                errorText: _error,
                suffixIcon: IconButton(
                  key: const ValueKey('emoji-use'),
                  tooltip: 'Use this emoji',
                  icon: const Icon(Icons.check_rounded),
                  onPressed: _usePasted,
                ),
              ),
              onChanged: (_) {
                if (_error != null) setState(() => _error = null);
              },
              onSubmitted: (_) => _usePasted(),
            ),
            const SizedBox(height: 12),
            Flexible(
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (final (group, list) in emojiGroups) ...[
                      Padding(
                        padding: const EdgeInsets.only(top: 8, bottom: 4),
                        child: Text(
                          group.toUpperCase(),
                          style: Theme.of(context).textTheme.labelSmall,
                        ),
                      ),
                      Wrap(
                        spacing: 2,
                        runSpacing: 2,
                        children: [
                          for (final e in list)
                            InkWell(
                              key: ValueKey('emoji-$group-$e'),
                              borderRadius: BorderRadius.circular(6),
                              onTap: () =>
                                  Navigator.pop(context, EmojiChoice(e)),
                              child: Container(
                                width: 38,
                                height: 38,
                                alignment: Alignment.center,
                                decoration: BoxDecoration(
                                  color: e == widget.current
                                      ? colors.accentSoft
                                      : null,
                                  borderRadius: BorderRadius.circular(6),
                                ),
                                child: Text(
                                  e,
                                  style: const TextStyle(fontSize: 20),
                                ),
                              ),
                            ),
                        ],
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
      actions: [
        if (widget.current != null)
          TextButton(
            key: const ValueKey('emoji-clear'),
            onPressed: () => Navigator.pop(context, const EmojiChoice(null)),
            child: const Text('Remove emoji'),
          ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}
