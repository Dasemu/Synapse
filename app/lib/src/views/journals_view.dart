import 'package:flutter/material.dart';

import '../backend/graph_backend.dart';
import '../editor/outline_view.dart';
import '../state/app_state.dart';
import '../state/outline_controller.dart';
import '../theme/theme.dart';
import '../widgets/common.dart';
import '../widgets/dates.dart';

/// Continuous feed of journals, today on top, loading older days as you scroll.
class JournalsView extends StatefulWidget {
  const JournalsView({super.key});

  @override
  State<JournalsView> createState() => _JournalsViewState();
}

class _Day {
  _Day(this.view, this.controller);
  final PageData view;
  final OutlineController controller;
}

class _JournalsViewState extends State<JournalsView> {
  static const _batch = 7;
  final _days = <_Day>[];
  bool _loading = false;
  bool _exhausted = false;
  Object? _error;

  @override
  void initState() {
    super.initState();
    _loadMore();
  }

  @override
  void dispose() {
    for (final d in _days) {
      d.controller.dispose();
    }
    super.dispose();
  }

  Future<void> _loadMore() async {
    if (_loading || _exhausted) return;
    setState(() => _loading = true);
    final app = AppScope.read(context);
    try {
      final before = _days.isEmpty ? null : _days.last.view.page.journalDate;
      if (_days.isEmpty) await WidgetsBinding.instance.endOfFrame;
      final views = await app.queue.run(
        () => app.backend.journals(before: before, limit: _batch),
        report: false,
      );
      if (!mounted) return;
      setState(() {
        for (final v in views) {
          _days.add(
            _Day(
              v,
              OutlineController(
                backend: app.backend,
                queue: app.queue,
                pageId: v.page.id,
                blocks: v.blocks,
              ),
            ),
          );
        }
        _exhausted = views.length < _batch;
        _loading = false;
      });
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = e;
          _loading = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final today = app.backend.today();
    return LayoutBuilder(
      builder: (context, box) {
        final pad = sidePadding(box.maxWidth);
        return NotificationListener<ScrollNotification>(
          onNotification: (n) {
            if (n.metrics.extentAfter < 800) _loadMore();
            return false;
          },
          child: CustomScrollView(
            slivers: [
              const SliverToBoxAdapter(child: SizedBox(height: 28)),
              for (final d in _days)
                SliverPadding(
                  padding: EdgeInsets.symmetric(horizontal: pad),
                  sliver: SliverMainAxisGroup(
                    slivers: [
                      SliverToBoxAdapter(
                        child: _JournalHeader(view: d.view, today: today),
                      ),
                      OutlineSliver(controller: d.controller),
                      SliverToBoxAdapter(
                        child: Padding(
                          padding: const EdgeInsets.symmetric(vertical: 36),
                          child: Divider(height: 1, color: context.colors.line),
                        ),
                      ),
                    ],
                  ),
                ),
              SliverToBoxAdapter(
                child: Padding(
                  padding: const EdgeInsets.only(bottom: 80),
                  child: Center(
                    child: _error != null
                        ? Text('Could not load journals: $_error')
                        : _exhausted
                        ? Text(
                            'The beginning of your journals',
                            style: TextStyle(
                              color: context.colors.faint,
                              fontSize: 13,
                            ),
                          )
                        : const SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _JournalHeader extends StatelessWidget {
  const _JournalHeader({required this.view, required this.today});
  final PageData view;
  final String today;

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final date = view.page.journalDate!;
    final colors = context.colors;
    final rel = relativeDay(date, today);
    return Padding(
      padding: const EdgeInsets.only(bottom: 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Flexible(
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  child: GestureDetector(
                    onTap: () => app.openId(view.page.id),
                    child: Text(
                      longDate(date),
                      style: Theme.of(context).textTheme.headlineSmall,
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 12),
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: Text(
                  rel == null ? weekday(date) : '$rel · ${weekday(date)}',
                  style: TextStyle(color: colors.muted, fontSize: 13),
                ),
              ),
            ],
          ),
          const SizedBox(height: 4),
          JournalChain(view: view),
        ],
      ),
    );
  }
}

/// Previous / next journal links (the episodic chain).
class JournalChain extends StatelessWidget {
  const JournalChain({super.key, required this.view});
  final PageData view;

  @override
  Widget build(BuildContext context) {
    final app = AppScope.read(context);
    final prev = view.prevJournal;
    final next = view.nextJournal;
    if (prev == null && next == null) return const SizedBox.shrink();
    return Row(
      children: [
        if (prev != null)
          TextLink(
            shortDate(parseDay(prev)),
            icon: Icons.west_rounded,
            onTap: () => app.openTitle(prev),
            style: const TextStyle(fontSize: 12),
          ),
        if (prev != null && next != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10),
            child: Text('·', style: TextStyle(color: context.colors.faint)),
          ),
        if (next != null)
          TextLink(
            shortDate(parseDay(next)),
            icon: Icons.east_rounded,
            iconAfter: true,
            onTap: () => app.openTitle(next),
            style: const TextStyle(fontSize: 12),
          ),
      ],
    );
  }
}
