import 'package:flutter/material.dart';

import '../state/outline_controller.dart';
import '../state/outline_ops.dart';
import '../theme/theme.dart';
import 'block_row.dart';

/// The outline of one page as a lazily built sliver (only visible rows are built).
class OutlineSliver extends StatelessWidget {
  const OutlineSliver({super.key, required this.controller});

  final OutlineController controller;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        final nodes = controller.nodes;
        if (nodes.isEmpty) {
          return SliverToBoxAdapter(
            child: _EmptyOutline(onTap: controller.insertFirst),
          );
        }
        final vis = controller.visible;
        final keys = <Object, int>{
          for (var k = 0; k < vis.length; k++) nodes[vis[k]]: k,
        };
        return SliverList.builder(
          itemCount: vis.length,
          findChildIndexCallback: (key) =>
              key is ObjectKey ? keys[key.value] : null,
          itemBuilder: (context, k) {
            final i = vis[k];
            final n = nodes[i];
            return BlockRow(
              key: ObjectKey(n),
              controller: controller,
              node: n,
              editing: controller.focusedId == n.id,
              hasChildren: OutlineOps.hasChildren(nodes, i),
            );
          },
        );
      },
    );
  }
}

class _EmptyOutline extends StatelessWidget {
  const _EmptyOutline({required this.onTap});
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final colors = context.colors;
    return MouseRegion(
      cursor: SystemMouseCursors.text,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 2),
          child: Row(
            children: [
              const SizedBox(width: 18),
              SizedBox(
                width: 18,
                child: Center(
                  child: Container(
                    width: 6,
                    height: 6,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: colors.line,
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 6),
              Text(
                'Click to start writing…',
                style: blockTextStyle(context).copyWith(color: colors.faint),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
