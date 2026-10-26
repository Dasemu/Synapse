import 'dart:convert';
import 'dart:io';

/// Where user preferences (theme, key bindings) are kept between runs.
abstract class SettingsStore {
  Map<String, dynamic> load();
  void save(Map<String, dynamic> data);
}

/// Keeps nothing (tests, previews).
class MemorySettings implements SettingsStore {
  Map<String, dynamic> data = {};

  @override
  Map<String, dynamic> load() => data;

  @override
  void save(Map<String, dynamic> d) => data = d;
}

/// A small JSON file; unreadable or corrupt files just mean defaults.
class FileSettings implements SettingsStore {
  FileSettings(this.path);
  final String path;

  @override
  Map<String, dynamic> load() {
    try {
      final v = jsonDecode(File(path).readAsStringSync());
      return v is Map<String, dynamic> ? v : {};
    } catch (_) {
      return {};
    }
  }

  @override
  void save(Map<String, dynamic> data) {
    try {
      File(path).writeAsStringSync(jsonEncode(data));
    } catch (_) {}
  }
}
