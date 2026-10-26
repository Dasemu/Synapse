import 'dart:math' as math;
import 'dart:typed_data';

import 'package:synapse/src/backend/graph_backend.dart';
import 'package:synapse/src/graph/graph_model.dart';
import 'package:synapse/src/state/outline_ops.dart';

/// In-memory [GraphBackend] for widget tests. Outline semantics reuse [OutlineOps], which
/// mirror the Rust FFI (the Rust side has its own tests).
class MemoryBackend implements GraphBackend {
  MemoryBackend({this.todayDate = '2026-10-21'});

  final String todayDate;

  @override
  String get graphName => 'Test graph';
  final pages = <String, PageDto>{};
  final outlines = <String, List<OutlineNode>>{};
  final aliases = <String, String>{};
  final calls = <String>[];
  int _seq = 0;

  String _id() => 'id-${_seq++}';

  PageDto createPage(String title, {String? journalDate}) {
    final p = PageDto(
      id: journalDate != null ? 'j-$journalDate' : _id(),
      title: title,
      journalDate: journalDate,
      createdAt: 0,
      updatedAt: 0,
      isHub: false,
      description: '',
      tags: const [],
      isTag: false,
      types: const [],
    );
    pages[p.id] = p;
    outlines[p.id] = [];
    return p;
  }

  /// Adds blocks `(depth, content)` to a page.
  List<String> seed(String pageId, List<(int, String)> rows) {
    final l = outlines[pageId]!;
    final ids = <String>[];
    for (final (d, c) in rows) {
      final id = _id();
      l.add(OutlineNode(id: id, depth: d, content: c));
      ids.add(id);
    }
    OutlineOps.fixParents(l);
    return ids;
  }

  List<BlockDto> _dto(String pageId) {
    final l = outlines[pageId]!;
    return [
      for (var i = 0; i < l.length; i++)
        BlockDto(
          id: l[i].id,
          parentId: l[i].parentId,
          depth: l[i].depth,
          content: l[i].content,
          collapsed: l[i].collapsed,
          hasChildren: OutlineOps.hasChildren(l, i),
        ),
    ];
  }

  (String, int) _find(String blockId) {
    for (final e in outlines.entries) {
      final i = e.value.indexWhere((n) => n.id == blockId);
      if (i >= 0) return (e.key, i);
    }
    throw StateError('block $blockId not found');
  }

  PageData _view(PageDto p) => PageData(
    page: p,
    blocks: _dto(p.id),
    aliases: [
      for (final e in aliases.entries)
        if (e.value == p.id) e.key,
    ],
  );

  PageDto? _byTitle(String title) {
    final k = title.trim().toLowerCase();
    for (final p in pages.values) {
      if (p.title.toLowerCase() == k) return p;
    }
    final a = aliases[k];
    return a == null ? null : pages[a];
  }

  @override
  String today() => todayDate;

  @override
  Future<List<PageData>> journals({String? before, required int limit}) async {
    if (before == null) await journal(todayDate);
    final js =
        pages.values
            .where(
              (p) =>
                  p.journalDate != null &&
                  (before == null || p.journalDate!.compareTo(before) < 0),
            )
            .toList()
          ..sort((a, b) => b.journalDate!.compareTo(a.journalDate!));
    return js.take(limit).map(_view).toList();
  }

  @override
  Future<PageData> journal(String date) async =>
      _view(pages['j-$date'] ?? createPage(date, journalDate: date));

  @override
  Future<PageData?> page(String id) async {
    final p = pages[id];
    return p == null ? null : _view(p);
  }

  @override
  Future<PageData?> pageByTitle(String title) async {
    final p = _byTitle(title);
    return p == null ? null : _view(p);
  }

  @override
  Future<PageData> openPage(String title) async =>
      _view(_byTitle(title) ?? createPage(title));

  @override
  Future<List<PageDto>> listPages({
    bool includeJournals = false,
    required int limit,
    int offset = 0,
  }) async => pages.values
      .where((p) => includeJournals || p.journalDate == null)
      .skip(offset)
      .take(limit)
      .toList();

  @override
  Future<PageDto> renamePage(String id, String newTitle) async {
    final p = pages[id]!;
    final t = newTitle.trim();
    final clash = _byTitle(t);
    if (clash != null && clash.id != id) {
      throw Exception('A page titled "$t" already exists');
    }
    for (final l in outlines.values) {
      for (final n in l) {
        n.content = n.content.replaceAll('[[${p.title}]]', '[[$t]]');
      }
    }
    final old = p.title;
    _put(p, title: t);
    if (p.isTag) {
      for (final q in pages.values.toList()) {
        if (q.tags.any((x) => _tagKey(x) == _tagKey(old))) {
          _put(
            q,
            tags: [for (final x in q.tags) _tagKey(x) == _tagKey(old) ? t : x],
          );
        }
      }
    }
    return pages[id]!;
  }

  @override
  Future<void> deletePage(String id) async {
    final gone = pages[id];
    if (gone != null && gone.isTag) {
      for (final q in pages.values.toList()) {
        _put(
          q,
          tags: [
            for (final x in q.tags)
              if (_tagKey(x) != _tagKey(gone.title)) x,
          ],
        );
      }
    }
    pages.remove(id);
    outlines.remove(id);
    _forget(id);
  }

  void _forget(String id) {
    hubsOfPage.remove(id);
    for (final s in hubsOfPage.values) {
      s.remove(id);
    }
  }

  @override
  Future<void> addAlias(String pageId, String alias) async =>
      aliases[alias.toLowerCase()] = pageId;

  @override
  Future<void> removeAlias(String alias) async =>
      aliases.remove(alias.toLowerCase());

  int? _index(String pageId, String? id) =>
      id == null ? null : outlines[pageId]!.indexWhere((n) => n.id == id);

  @override
  Future<BlockDto> insertBlock(
    String pageId,
    Place place,
    String content,
  ) async {
    calls.add('insert');
    if (_isHub(pageId)) throw Exception('A hub has no blocks');
    final n = OutlineNode(id: _id(), depth: 0, content: content);
    OutlineOps.insert(
      outlines[pageId]!,
      n,
      place.kind,
      _index(pageId, place.anchor),
    );
    return BlockDto(
      id: n.id,
      parentId: n.parentId,
      depth: n.depth,
      content: content,
      collapsed: false,
      hasChildren: false,
    );
  }

  @override
  Future<void> updateBlock(String id, String content) async {
    calls.add('update');
    final (p, i) = _find(id);
    outlines[p]![i].content = content;
  }

  @override
  Future<void> setCollapsed(String id, bool collapsed) async {
    final (p, i) = _find(id);
    outlines[p]![i].collapsed = collapsed;
  }

  @override
  Future<List<BlockDto>> deleteBlock(String id) async {
    final (p, i) = _find(id);
    OutlineOps.delete(outlines[p]!, i);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> moveBlock(
    String id,
    String pageId,
    Place place,
  ) async {
    final (p, i) = _find(id);
    OutlineOps.move(outlines[p]!, i, place.kind, _index(p, place.anchor));
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> indentBlock(String id) async {
    calls.add('indent');
    final (p, i) = _find(id);
    OutlineOps.indent(outlines[p]!, i);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> outdentBlock(String id) async {
    calls.add('outdent');
    final (p, i) = _find(id);
    OutlineOps.outdent(outlines[p]!, i);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> mergeBlock(String id, String into) async {
    calls.add('merge');
    final (p, i) = _find(id);
    OutlineOps.merge(outlines[p]!, i, _index(p, into)!);
    return _dto(p);
  }

  @override
  Future<List<BlockDto>> outline(String pageId) async => _dto(pageId);

  @override
  Future<List<BacklinkGroup>> backlinks(String pageId) async => [];
  @override
  Future<List<RelatedDto>> related(String pageId, {int limit = 12}) async => [];
  @override
  Future<void> recordVisit(String? from, String to) async {}
  @override
  Future<BlockRefDto?> blockRef(String id) async => null;
  @override
  Future<TimelineDto> timeline(String from, String to) async =>
      const TimelineDto(days: [], pages: []);

  @override
  Future<List<TitleHit>> searchTitles(String query, {int limit = 20}) async {
    final q = query.toLowerCase();
    return pages.values
        .where(
          (p) => p.journalDate == null && p.title.toLowerCase().contains(q),
        )
        .take(limit)
        .map((p) => TitleHit(pageId: p.id, title: p.title, isAlias: false))
        .toList();
  }

  @override
  Future<List<BlockHit>> searchBlocks(String query, {int limit = 20}) async =>
      [];

  @override
  String? assetPath(String hash) => null;

  @override
  Future<AssetDto> putAssetFile(String path, {String? mime}) =>
      throw UnimplementedError();

  /// Pages `legacyCandidates` reports, id to reason. Preselection follows the core: not a
  /// hub, and empty or one of Logseq's organisational pages.
  final legacy = <String, String>{};

  /// Ids passed to `purgePages`, in call order.
  final purged = <String>[];

  /// Parent hubs of each page (membership lives on the member, like the core).
  final hubsOfPage = <String, Set<String>>{};

  /// Test shortcut: `hubFlags[id] = true` turns a page into a hub whose members are the
  /// pages its outline linked to (same as converting it with all candidates kept).
  late final hubFlags = _HubFlags(this);

  bool _isHub(String id) => pages[id]?.isHub ?? false;

  PageBrief _brief(PageDto p) => PageBrief(
    id: p.id,
    title: p.title,
    isJournal: p.journalDate != null,
    isHub: p.isHub,
  );

  void _put(
    PageDto p, {
    String? title,
    bool? isHub,
    String? description,
    List<String>? tags,
    bool? isTag,
    List<String>? types,
  }) {
    pages[p.id] = PageDto(
      id: p.id,
      title: title ?? p.title,
      journalDate: p.journalDate,
      originJournal: p.originJournal,
      createdAt: p.createdAt,
      updatedAt: p.updatedAt,
      isHub: isHub ?? p.isHub,
      description: description ?? p.description,
      tags: tags ?? p.tags,
      isTag: isTag ?? p.isTag,
      types: types ?? p.types,
    );
  }

  /// Members of a hub: sub-hubs first, then notes, each by title.
  List<PageDto> _members(String hubId) {
    final out = [
      for (final e in hubsOfPage.entries)
        if (e.value.contains(hubId) && pages.containsKey(e.key)) pages[e.key]!,
    ];
    out.sort((a, b) {
      if (a.isHub != b.isHub) return a.isHub ? -1 : 1;
      return a.title.toLowerCase().compareTo(b.title.toLowerCase());
    });
    return out;
  }

  /// Whether `target` is `hub` or lies below it.
  bool _reaches(String hub, String target) {
    if (hub == target) return true;
    return _members(hub).any((m) => m.isHub && _reaches(m.id, target));
  }

  /// Pages a page's outline links to or tags (outline order, journals and self skipped).
  List<PageDto> _linked(String id) {
    final link = RegExp(r'\[\[([^\]]+)\]\]|(?:^|\s)#(\w+)');
    final out = <PageDto>[];
    for (final n in outlines[id] ?? const <OutlineNode>[]) {
      for (final m in link.allMatches(n.content)) {
        final p = _byTitle(m.group(1) ?? m.group(2)!);
        if (p == null || p.id == id || p.journalDate != null) continue;
        if (!out.any((b) => b.id == p.id)) out.add(p);
      }
    }
    return out;
  }

  /// Makes `page` a hub, dropping its blocks; `members` are added when valid.
  void makeHub(String id, {String? description, List<String>? members}) {
    final linked = members ?? [for (final p in _linked(id)) p.id];
    outlines[id] = [];
    _put(pages[id]!, isHub: true, description: description);
    for (final m in linked) {
      _tryAdd(m, id);
    }
  }

  bool _tryAdd(String page, String hub) {
    final p = pages[page];
    if (p == null ||
        p.journalDate != null ||
        !_isHub(hub) ||
        page == hub ||
        _reaches(page, hub)) {
      return false;
    }
    (hubsOfPage[page] ??= {}).add(hub);
    return true;
  }

  /// Synchronous `createHub` for test setup.
  PageDto addHub(String title, [String description = '']) {
    final p = createPage(title);
    _put(p, isHub: true, description: description);
    return pages[p.id]!;
  }

  @override
  Future<PageDto> createHub(String title, String description) async {
    final t = title.trim();
    if (t.isEmpty) throw Exception('A title is required');
    if (_byTitle(t) != null) {
      throw Exception('A page titled "$t" already exists');
    }
    final p = createPage(t);
    _put(p, isHub: true, description: description);
    return pages[p.id]!;
  }

  final templates = <String, String>{};

  String _tagKey(String t) => t.trim().toLowerCase();

  @override
  Future<PageDto> setPageTags(String pageId, List<String> tags) async {
    final p = pages[pageId]!;
    if (p.journalDate != null) throw Exception('Journals cannot have tags');
    final next = <String>[];
    for (final raw in tags) {
      final t = raw.trim().replaceFirst(RegExp(r'^#+'), '').trim();
      if (t.isNotEmpty && !next.any((x) => _tagKey(x) == _tagKey(t))) {
        next.add(t);
      }
    }
    final stored = <String>[];
    for (final t in next) {
      var tp = _byTitle(t);
      if (tp == null) {
        tp = createPage(t);
        _put(tp, isTag: true);
      } else if (tp.journalDate != null || tp.isHub) {
        throw Exception('"$t" cannot be a tag');
      } else if (!tp.isTag) {
        _put(tp, isTag: true);
      }
      stored.add(pages[tp.id]!.title);
    }
    _put(p, tags: stored);
    return pages[pageId]!;
  }

  static const suggestedTypes = ['persona', 'lugar', 'empresa'];

  @override
  Future<PageDto> setPageTypes(String pageId, List<String> types) async {
    final p = pages[pageId]!;
    if (p.journalDate != null) throw Exception('Journals cannot have types');
    final next = <String>[];
    for (final raw in types) {
      final t = raw.trim();
      if (t.isNotEmpty && !next.any((x) => _tagKey(x) == _tagKey(t))) {
        next.add(t);
      }
    }
    _put(p, types: next);
    return pages[pageId]!;
  }

  @override
  Future<PageDto> addPageType(String pageId, String ty) =>
      setPageTypes(pageId, [...pages[pageId]!.types, ty]);

  @override
  Future<PageDto> removePageType(String pageId, String ty) =>
      setPageTypes(pageId, [
        for (final t in pages[pageId]!.types)
          if (_tagKey(t) != _tagKey(ty)) t,
      ]);

  @override
  Future<List<TagDto>> typeCatalog() async {
    final used = <String, TagDto>{};
    for (final p in pages.values) {
      for (final t in p.types) {
        final k = _tagKey(t);
        used[k] = TagDto(
          name: used[k]?.name ?? t,
          count: (used[k]?.count ?? 0) + 1,
          suggested: suggestedTypes.contains(k),
        );
      }
    }
    final out = used.values.toList()
      ..sort((a, b) => b.count.compareTo(a.count));
    for (final s in suggestedTypes) {
      if (!used.containsKey(s)) {
        out.add(TagDto(name: s, count: 0, suggested: true));
      }
    }
    return out;
  }

  @override
  Future<PageDto> addPageTag(String pageId, String tag) =>
      setPageTags(pageId, [...pages[pageId]!.tags, tag]);

  @override
  Future<PageDto> removePageTag(String pageId, String tag) =>
      setPageTags(pageId, [
        for (final t in pages[pageId]!.tags)
          if (_tagKey(t) != _tagKey(tag)) t,
      ]);

  @override
  Future<List<TagDto>> tagCatalog() async {
    final out = [
      for (final t in pages.values)
        if (t.isTag)
          TagDto(
            name: t.title,
            count: pages.values
                .where((p) => p.tags.any((x) => _tagKey(x) == _tagKey(t.title)))
                .length,
            suggested: false,
          ),
    ]..sort((a, b) => b.count.compareTo(a.count));
    return out;
  }

  @override
  Future<List<PageBrief>> pagesWithTag(String tag) async => [
    for (final p in pages.values)
      if (p.tags.any((t) => _tagKey(t) == _tagKey(tag))) _brief(p),
  ];

  @override
  Future<String> hubTemplate(String hubId) async {
    if (!_isHub(hubId)) throw Exception('Not a hub');
    return templates.putIfAbsent(hubId, () {
      final t = createPage('Template: ${pages[hubId]!.title}');
      return t.id;
    });
  }

  @override
  Future<PageDto> createPageFromTemplate(String hubId, String title) async {
    final t = await hubTemplate(hubId);
    final name = title.trim();
    if (name.isEmpty) throw Exception('A title is required');
    if (_byTitle(name) != null) {
      throw Exception('A page titled "$name" already exists');
    }
    final p = createPage(name);
    seed(p.id, [for (final n in outlines[t]!) (n.depth, n.content)]);
    await addToHub(p.id, hubId);
    return pages[p.id]!;
  }

  @override
  Future<PageDto> setHubDescription(String hubId, String description) async {
    if (!_isHub(hubId)) throw Exception('Not a hub');
    _put(pages[hubId]!, description: description);
    return pages[hubId]!;
  }

  @override
  Future<void> addToHub(String pageId, String hubId) async {
    if (!_isHub(hubId)) throw Exception('The target is not a hub');
    if (pageId == hubId) throw Exception('A hub cannot contain itself');
    if (pages[pageId]?.journalDate != null) {
      throw Exception('Journals cannot be in a hub');
    }
    if (_reaches(pageId, hubId)) throw Exception('That would create a cycle');
    // A page belongs to one hub: adding it elsewhere moves it.
    hubsOfPage[pageId] = {hubId};
  }

  @override
  Future<void> removeFromHub(String pageId, String hubId) async {
    hubsOfPage[pageId]?.remove(hubId);
  }

  @override
  Future<HubPreviewDto> hubConversionPreview(String pageId) async {
    final p = pages[pageId]!;
    final title = p.title.split('/').last.trim();
    final strip = RegExp(r'\[\[([^\]]+)\]\]|(?:^|\s)#\w+|^#+\s*|\w+::.*');
    var prose = 0;
    String? first;
    final l = outlines[pageId] ?? const <OutlineNode>[];
    for (final n in l) {
      final text = n.content
          .replaceAllMapped(strip, (m) => m.group(1) ?? '')
          .trim();
      if (text.isEmpty) continue;
      prose++;
      first ??= text;
    }
    final members = [..._linked(pageId)];
    final ref = '[[${p.title}]]';
    for (final q in pages.values) {
      if (q.id == pageId || q.journalDate != null) continue;
      final refs = (outlines[q.id] ?? const <OutlineNode>[]).any(
        (n) => n.content.contains(ref) || n.content.contains('#${p.title}'),
      );
      if (refs && !members.any((m) => m.id == q.id)) members.add(q);
    }
    return HubPreviewDto(
      suggestedTitle: title,
      suggestedDescription: first ?? '',
      members: [for (final m in members) _brief(m)],
      proseBlocks: prose,
      blocks: l.length,
      suggestedNoteTitle: '$title (notes)',
    );
  }

  @override
  Future<PageDto> convertToHub(
    String pageId,
    String title,
    String description,
    List<String> memberIds, {
    String? keepOutlineAs,
  }) async {
    final p = pages[pageId]!;
    if (p.journalDate != null) throw Exception('Journals cannot be hubs');
    if (p.isHub) throw Exception('Already a hub');
    final t = title.trim();
    if (t.isEmpty) throw Exception('A title is required');
    final clash = _byTitle(t);
    if (clash != null && clash.id != pageId) {
      throw Exception('A page titled "$t" already exists');
    }
    final keep = keepOutlineAs?.trim();
    if (keep != null) {
      if (keep.isEmpty) {
        throw Exception('The note that keeps the outline needs a title');
      }
      // Checked against the new hub title too, like the core (which rolls back).
      // (the old title is free once the page is renamed).
      final c = _byTitle(keep);
      if ((c != null && c.id != pageId) ||
          keep.toLowerCase() == t.toLowerCase()) {
        throw Exception('A page titled "$keep" already exists');
      }
    }
    if (t != p.title) {
      for (final l in outlines.values) {
        for (final n in l) {
          n.content = n.content.replaceAll('[[${p.title}]]', '[[$t]]');
        }
      }
      _put(p, title: t);
    }
    String? noteId;
    if (keep != null) {
      final note = createPage(keep);
      outlines[note.id] = outlines[pageId]!;
      noteId = note.id;
    }
    outlines[pageId] = [];
    _put(pages[pageId]!, isHub: true, description: description);
    if (noteId != null) _tryAdd(noteId, pageId);
    for (final m in memberIds) {
      _tryAdd(m, pageId);
    }
    return pages[pageId]!;
  }

  @override
  Future<PageDto> convertToNote(String pageId) async {
    final p = pages[pageId]!;
    if (!p.isHub) throw Exception('Not a hub');
    final members = _members(pageId);
    final l = <OutlineNode>[];
    if (p.description.isNotEmpty) {
      l.add(OutlineNode(id: _id(), depth: 0, content: p.description));
    }
    for (final m in members) {
      l.add(OutlineNode(id: _id(), depth: 0, content: '[[${m.title}]]'));
      hubsOfPage[m.id]?.remove(pageId);
    }
    OutlineOps.fixParents(l);
    outlines[pageId] = l;
    _put(p, isHub: false, description: '');
    return pages[pageId]!;
  }

  @override
  Future<HubsDto> hubs() async {
    final all = [
      for (final p in pages.values)
        if (p.isHub) p,
    ]..sort((a, b) => a.title.compareTo(b.title));
    final roots = [
      for (final p in all)
        if (!(hubsOfPage[p.id] ?? const {}).any(_isHub)) p.id,
    ];
    final depth = <String, int>{};
    var frontier = roots;
    for (var d = 0; frontier.isNotEmpty; d++) {
      final next = <String>[];
      for (final id in frontier) {
        if (depth.containsKey(id)) continue;
        depth[id] = d;
        next.addAll([
          for (final m in _members(id))
            if (m.isHub) m.id,
        ]);
      }
      frontier = next;
    }
    return HubsDto(
      hubs: [
        for (final p in all)
          HubDto(
            id: p.id,
            title: p.title,
            description: p.description,
            depth: depth[p.id] ?? 0,
            members: [for (final m in _members(p.id)) _brief(m)],
            subHubs: [
              for (final m in _members(p.id))
                if (m.isHub) m.id,
            ],
          ),
      ],
      roots: roots,
    );
  }

  /// Deepest hub containing each page (ties: fewer members, then title); a hub maps to
  /// itself.
  Map<String, String> _home(HubsDto h) {
    final byId = {for (final x in h.hubs) x.id: x};
    final home = <String, String>{};
    for (final p in pages.values) {
      if (p.isHub) {
        home[p.id] = p.id;
        continue;
      }
      final cand = [
        for (final id in hubsOfPage[p.id] ?? const <String>{})
          if (byId.containsKey(id)) byId[id]!,
      ];
      if (cand.isEmpty) continue;
      cand.sort((a, b) {
        final d = b.depth.compareTo(a.depth);
        if (d != 0) return d;
        final m = a.members.length.compareTo(b.members.length);
        return m != 0 ? m : a.title.compareTo(b.title);
      });
      home[p.id] = cand.first.id;
    }
    return home;
  }

  @override
  Future<PageStructure> pageStructure(String pageId) async {
    final p = pages[pageId]!;
    final parts = p.title.split('/');
    final path = <NamespaceEntry>[];
    for (var i = 1; i < parts.length; i++) {
      final t = parts.sublist(0, i).join('/');
      path.add(NamespaceEntry(title: t, id: _byTitle(t)?.id));
    }
    final prefix = '${p.title}/';
    final children = [
      for (final q in pages.values)
        if (q.title.startsWith(prefix) &&
            !q.title.substring(prefix.length).contains('/'))
          NamespaceEntry(title: q.title, id: q.id),
    ]..sort((a, b) => a.title.compareTo(b.title));
    final inHubs = [
      for (final id in hubsOfPage[pageId] ?? const <String>{})
        if (pages[id] != null && pages[id]!.isHub) pages[id]!,
    ]..sort((a, b) => a.title.compareTo(b.title));
    return PageStructure(
      isHub: p.isHub,
      description: p.description,
      members: p.isHub ? [for (final m in _members(pageId)) _brief(m)] : [],
      inHubs: [for (final h in inHubs) _brief(h)],
      nsPath: path,
      nsChildren: children,
    );
  }

  @override
  Future<List<LegacyCandidate>> legacyCandidates() async => [
    for (final e in legacy.entries)
      if (pages.containsKey(e.key))
        LegacyCandidate(
          id: e.key,
          title: pages[e.key]!.title,
          reason: e.value,
          blocks: outlines[e.key]?.length ?? 0,
          preselected: _preselected(e.key),
        ),
  ];

  bool _preselected(String id) {
    final p = pages[id]!;
    final t = p.title.toLowerCase();
    return !p.isHub &&
        ((outlines[id]?.length ?? 0) == 0 ||
            const ['contents', 'card', 'favorites'].contains(t) ||
            t.startsWith('hls__'));
  }

  @override
  Future<int> purgePages(List<String> ids) async {
    var n = 0;
    for (final id in ids) {
      if (pages.remove(id) != null) n++;
      outlines.remove(id);
      _forget(id);
      purged.add(id);
    }
    return n;
  }

  /// Links (`[[x]]`) and tags (`#x`) between pages, built like the Rust side but simpler.
  @override
  Future<GraphData> graphData() async {
    final ids = pages.keys.toList();
    final index = {for (var i = 0; i < ids.length; i++) ids[i]: i};
    final s = <int>[], t = <int>[], k = <int>[];
    final link = RegExp(r'\[\[([^\]]+)\]\]');
    final tag = RegExp(r'(?:^|\s)#(\w+)');
    final hubData = await hubs();
    final home = _home(hubData);
    final containsEdges = <(String, String)>[
      for (final e in hubsOfPage.entries)
        for (final h in e.value)
          if (pages.containsKey(e.key) && pages.containsKey(h)) (h, e.key),
    ];
    final degree = List.filled(ids.length, 0, growable: true);
    for (final id in List.of(ids)) {
      for (final n in outlines[id]!) {
        void add(String title, EdgeKind kind) {
          final p = _byTitle(title) ?? createPageForGraph(title, index, ids);
          s.add(index[id]!);
          t.add(index[p.id]!);
          k.add(kind.index);
        }

        for (final m in link.allMatches(n.content)) {
          add(m.group(1)!, EdgeKind.link);
        }
        for (final m in tag.allMatches(n.content)) {
          add(m.group(1)!, EdgeKind.tag);
        }
      }
    }
    for (final (h, m) in containsEdges) {
      s.add(index[h]!);
      t.add(index[m]!);
      k.add(EdgeKind.contains.index);
    }
    // A note points at each of its tag pages.
    for (final p in pages.values) {
      final i = index[p.id];
      if (i == null) continue;
      for (final tg in p.tags) {
        final tp = _byTitle(tg);
        final j = tp == null ? null : index[tp.id];
        if (j == null) continue;
        s.add(i);
        t.add(j);
        k.add(EdgeKind.tagged.index);
      }
    }
    while (degree.length < ids.length) {
      degree.add(0);
    }
    for (var e = 0; e < s.length; e++) {
      degree[s[e]]++;
      degree[t[e]]++;
    }
    return GraphData(
      nodes: [
        for (var i = 0; i < ids.length; i++)
          GraphNode(
            id: ids[i],
            title: pages[ids[i]]!.title,
            journalDate: pages[ids[i]]!.journalDate,
            createdAt: 0,
            degree: degree[i],
            backlinks: 0,
            cluster: 0,
            isHub: _isHub(ids[i]),
            homeHub: home[ids[i]] == null ? null : index[home[ids[i]]!],
            tags: pages[ids[i]]!.tags,
          ),
      ],
      sources: Uint32List.fromList(s),
      targets: Uint32List.fromList(t),
      kinds: Uint8List.fromList(k),
      weights: Float32List.fromList(List.filled(s.length, 1)),
      seq: 0,
    );
  }

  PageDto createPageForGraph(
    String title,
    Map<String, int> index,
    List<String> ids,
  ) {
    final p = createPage(title);
    index[p.id] = ids.length;
    ids.add(p.id);
    return p;
  }

  final layouts = <FakeLayout>[];

  /// Steps each fake layout needs before it reports settled.
  int layoutSteps = 1;

  @override
  LayoutEngine createLayout(
    int nodeCount,
    Uint32List sources,
    Uint32List targets,
    Float32List weights, {
    Float32List? initial,
  }) {
    final l = FakeLayout(nodeCount, settleAfter: layoutSteps);
    layouts.add(l);
    return l;
  }
}

/// Places nodes on a circle and is settled after `settleAfter` steps.
class FakeLayout implements LayoutEngine {
  FakeLayout(this.n, {this.settleAfter = 1});
  final int n;
  final int settleAfter;
  int steps = 0;
  bool disposed = false;
  final pins = <int>[];

  @override
  Future<(Float32List, double)> step(int iterations) async {
    steps++;
    final p = Float32List(2 * n);
    for (var i = 0; i < n; i++) {
      final a = 2 * math.pi * i / math.max(n, 1);
      p[2 * i] = 200 * math.cos(a);
      p[2 * i + 1] = 200 * math.sin(a);
    }
    return (p, steps >= settleAfter ? 0.0 : 0.5);
  }

  @override
  Future<void> pin(int index, double x, double y) async => pins.add(index);
  @override
  Future<void> unpin(int index) async {}
  @override
  Future<void> reheat(double alpha) async {}
  @override
  void dispose() => disposed = true;
}

/// Map-like shortcut for tests: assigning `true` converts the page to a hub.
class _HubFlags {
  _HubFlags(this._b);
  final MemoryBackend _b;
  void operator []=(String id, bool hub) {
    if (hub && !_b._isHub(id)) _b.makeHub(id);
  }
}
