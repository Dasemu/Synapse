import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/dates.dart';

/// Every non-journal page, most recently edited first, loaded in pages of 100.
class AllPagesView extends StatefulWidget {
  const AllPagesView({super.key});

  @override
  State<AllPagesView> createState() => _AllPagesViewState();
}

class _AllPagesViewState extends State<AllPagesView> {
  final _pages = <PageDto>[];
  bool _loading = false;
  bool _done = false;

  @override
  void initState() {
    super.initState();
    _more();
  }

  Future<void> _more() async {
    if (_loading || _done) return;
    _loading = true;
    final batch = await AppScope.read(context).backend
        .listPages(limit: 100, offset: _pages.length);
    if (!mounted) return;
    setState(() {
      _pages.addAll(batch);
      _done = batch.length < 100;
      _loading = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final colors = context.colors;
    return LayoutBuilder(
      builder: (context, box) => NotificationListener<ScrollNotification>(
        onNotification: (n) {
          if (n.metrics.extentAfter < 600) _more();
          return false;
        },
        child: ListView.builder(
          padding: EdgeInsets.fromLTRB(
            sidePadding(box.maxWidth),
            36,
            sidePadding(box.maxWidth),
            80,
          ),
          itemCount: _pages.length + 1,
          itemBuilder: (context, i) {
            if (i == 0) {
              return Padding(
                padding: const EdgeInsets.only(bottom: 20),
                child: Text(
                  'All pages',
                  style: Theme.of(context).textTheme.headlineLarge,
                ),
              );
            }
            final p = _pages[i - 1];
            final edited = DateTime.fromMillisecondsSinceEpoch(
              p.updatedAt.toInt(),
            );
            return InkWell(
              borderRadius: BorderRadius.circular(8),
              onTap: () => app.openId(p.id),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 9, horizontal: 8),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        p.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 14.5, color: colors.ink),
                      ),
                    ),
                    if (p.originJournal != null)
                      Text(
                        'born ${shortDate(parseDay(p.originJournal!))}',
                        style: TextStyle(fontSize: 12, color: colors.faint),
                      ),
                    const SizedBox(width: 16),
                    SizedBox(
                      width: 70,
                      child: Text(
                        shortDate(edited),
                        textAlign: TextAlign.right,
                        style: TextStyle(fontSize: 12, color: colors.muted),
                      ),
                    ),
                  ],
                ),
              ),
            );
          },
        ),
      ),
    );
  }
}
