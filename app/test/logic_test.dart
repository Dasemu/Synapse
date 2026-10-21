import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/editor/completion.dart';
import 'package:synapse/src/editor/markdown.dart';
import 'package:synapse/src/state/app_state.dart';
import 'package:synapse/src/state/outline_ops.dart';
import 'package:synapse/src/state/write_queue.dart';
import 'package:synapse/src/theme/theme.dart';

import 'support/memory_backend.dart';

List<OutlineNode> tree(List<(int, String)> rows) {
  var k = 0;
  final l = [
    for (final (d, c) in rows) OutlineNode(id: 'n${k++}', depth: d, content: c),
  ];
  OutlineOps.fixParents(l);
  return l;
}

List<(int, String)> shape(List<OutlineNode> l) => [
  for (final n in l) (n.depth, n.content),
];

void main() {
  group('OutlineOps', () {
    test('merge into parent keeps grandchildren in place', () {
      final l = tree([(0, 'p'), (1, 'c'), (2, 'gc'), (1, 's')]);
      OutlineOps.merge(l, 1, 0);
      expect(shape(l), [(0, 'pc'), (1, 'gc'), (1, 's')]);
      expect(l[1].parentId, 'n0');
    });

    test('merge into a deeper previous row re-parents children', () {
      final l = tree([(0, 'a'), (1, 'a1'), (0, 'b'), (1, 'b1')]);
      OutlineOps.merge(l, 2, 1);
      expect(shape(l), [(0, 'a'), (1, 'a1b'), (2, 'b1')]);
    });

    test('move with subtree, before and as first child', () {
      final l = tree([(0, 'a'), (1, 'a1'), (0, 'b')]);
      expect(OutlineOps.move(l, 0, PlaceKind.after, 2), isTrue);
      expect(shape(l), [(0, 'b'), (0, 'a'), (1, 'a1')]);
      expect(OutlineOps.move(l, 0, PlaceKind.firstChildOf, 1), isTrue);
      expect(shape(l), [(0, 'a'), (1, 'b'), (1, 'a1')]);
      expect(OutlineOps.move(l, 0, PlaceKind.after, 1), isFalse);
    });

    test('visible skips collapsed subtrees', () {
      final l = tree([(0, 'a'), (1, 'a1'), (2, 'a2'), (0, 'b')]);
      l[0].collapsed = true;
      expect(OutlineOps.visible(l), [0, 3]);
    });
  });

  group('completion', () {
    test('detects page, tag and block triggers', () {
      expect(
        detectCompletion('see [[Ru', 8),
        const CompletionQuery(CompletionKind.page, 6, 'Ru'),
      );
      expect(detectCompletion('see [[Ru]] x', 12), isNull);
      expect(
        detectCompletion('a #pro', 6),
        const CompletionQuery(CompletionKind.tag, 3, 'pro'),
      );
      expect(detectCompletion('a#pro', 5), isNull);
      expect(
        detectCompletion('((abc', 5),
        const CompletionQuery(CompletionKind.block, 2, 'abc'),
      );
    });

    test('applies completions', () {
      final q = detectCompletion('#my ta', 6);
      expect(q, isNull, reason: 'tags stop at whitespace');
      final tag = detectCompletion('x #se', 5)!;
      final v = applyCompletion(
        'x #se',
        tag,
        const CompletionItem('Second Brain', 'Second Brain'),
      );
      expect(v.text, 'x #[[Second Brain]] ');
      final page = detectCompletion('[[Ru]]', 4)!;
      expect(
        applyCompletion(
          '[[Ru]]',
          page,
          const CompletionItem('Rust', 'Rust'),
        ).text,
        '[[Rust]]',
      );
    });

    test('replaces only the query even if the caret moved', () {
      final q = detectCompletion('[[foo]]', 5)!;
      final v = applyCompletion(
        '[[foo]]',
        q,
        const CompletionItem('Foo', 'Foo'),
      );
      expect(v.text, '[[Foo]]');
    });

    test('auto-pairs brackets', () {
      final v = autoPair(
        const TextEditingValue(
          text: 'a[',
          selection: TextSelection.collapsed(offset: 2),
        ),
        const TextEditingValue(
          text: 'a[[',
          selection: TextSelection.collapsed(offset: 3),
        ),
      );
      expect(v!.text, 'a[[]]');
      expect(v.selection.baseOffset, 3);
    });
  });

  group('markdown', () {
    RenderedBlock render(String s) {
      final backend = MemoryBackend();
      return BlockRenderer(
        RenderContext(
          colors: SynapseColors.light,
          base: const TextStyle(fontSize: 15),
          refs: BlockRefCache(backend),
          assetPath: (_) => null,
          onPage: (_) {},
          onBlockRef: (_) {},
          onUrl: (_) {},
        ),
      ).render(s);
    }

    String plain(RenderedBlock r) => r.span.toPlainText(
      includePlaceholders: true,
      includeSemanticsLabels: false,
    );

    test('hides markup and maps taps back to the source', () {
      final r = render('a **bold** [[Page]] end');
      expect(plain(r), 'a bold Page end');
      // "b" of bold: rendered 2 → source 4.
      expect(r.toSource(2), 4);
      // "P" of Page: rendered 7 → source 13.
      expect(r.toSource(7), 13);
      expect(r.toSource(plain(r).length), 'a **bold** [[Page]] end'.length);
      r.dispose();
    });

    test('renders tags, headings and todo markers', () {
      expect(plain(render('# Title')), 'Title');
      expect(plain(render('note #idea, ok')), 'note #idea, ok');
      final todo = render('DONE ship it');
      expect(plain(todo), '￼ship it');
      expect(todo.toSource(0), 5);
    });
  });

  test('a failed write makes the app reload its view', () async {
    final app = AppState(MemoryBackend());
    final gen = app.generation;
    await app.queue
        .run<void>(() async => throw StateError('boom'))
        .catchError((_) {});
    expect(app.generation, gen + 1);
    expect(app.writeError, contains('boom'));
    await app.queue
        .run<void>(() async => throw StateError('read'), report: false)
        .catchError((_) {});
    expect(app.generation, gen + 1);
  });

  test(
    'WriteQueue runs tasks in order even when they finish out of order',
    () async {
      final q = WriteQueue();
      final log = <int>[];
      final f1 = q.run(() async {
        await Future<void>.delayed(const Duration(milliseconds: 20));
        log.add(1);
      });
      final f2 = q.run(() async => log.add(2));
      await Future.wait([f1, f2]);
      expect(log, [1, 2]);
      expect(q.isIdle, isTrue);
    },
  );
}
