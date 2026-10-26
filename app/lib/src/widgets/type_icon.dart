import 'package:flutter/material.dart';

import '../state/app_state.dart';

/// The emoji of a page's type, drawn just left of the page's name (titles, lists, tabs, the
/// tree view; never in the network graph). Nothing when the type has no emoji. Icons come
/// from [AppState.typeIcons], so lists only need the page's type.
class TypeIcon extends StatelessWidget {
  const TypeIcon(this.pageType, {super.key, this.size = 13, this.gap = 5});

  final String? pageType;
  final double size;

  /// Space between the emoji and the name.
  final double gap;

  @override
  Widget build(BuildContext context) {
    final icon = AppScope.of(context).typeIcon(pageType);
    if (icon == null) return const SizedBox.shrink();
    return Padding(
      padding: EdgeInsets.only(right: gap),
      child: Text(
        icon,
        key: const ValueKey('type-icon'),
        style: TextStyle(fontSize: size, height: 1.15),
      ),
    );
  }
}

/// [title] prefixed with [icon] (for places that only take a string: canvas labels).
String withTypeIcon(String? icon, String title) =>
    icon == null ? title : '$icon $title';
