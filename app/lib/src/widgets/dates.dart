const _months = [
  'January', 'February', 'March', 'April', 'May', 'June', 'July', //
  'August', 'September', 'October', 'November', 'December',
];
const _days = [
  'Monday',
  'Tuesday',
  'Wednesday',
  'Thursday',
  'Friday',
  'Saturday',
  'Sunday',
];

DateTime parseDay(String ymd) {
  final p = ymd.split('-');
  return DateTime.utc(int.parse(p[0]), int.parse(p[1]), int.parse(p[2]));
}

String formatDay(DateTime d) =>
    '${d.year.toString().padLeft(4, '0')}-${d.month.toString().padLeft(2, '0')}-'
    '${d.day.toString().padLeft(2, '0')}';

/// "21 October 2026"
String longDate(String ymd) {
  final d = parseDay(ymd);
  return '${d.day} ${_months[d.month - 1]} ${d.year}';
}

/// "Tuesday"
String weekday(String ymd) => _days[parseDay(ymd).weekday - 1];

/// "21 Oct"
String shortDate(DateTime d) =>
    '${d.day} ${_months[d.month - 1].substring(0, 3)}';

String monthName(int month) => _months[month - 1];

/// "Today", "Yesterday" or null.
String? relativeDay(String ymd, String today) {
  final diff = parseDay(today).difference(parseDay(ymd)).inDays;
  return switch (diff) {
    0 => 'Today',
    1 => 'Yesterday',
    -1 => 'Tomorrow',
    _ => null,
  };
}
