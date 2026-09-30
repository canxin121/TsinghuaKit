import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';

/// The inclusive bounds of the bridge's `i64` argument type.
final BigInt _minInt64 = BigInt.parse('-9223372036854775808');
final BigInt _maxInt64 = BigInt.parse('9223372036854775807');

/// Converts a Dart integer to the bridge's `i64` argument type.
///
/// The bridge models `i64` as `int` on native platforms and as `BigInt` on the
/// web, so a caller cannot pass one plain Dart type in both. On the web the
/// value stays exact; on native it has to fit a 64-bit `int`, and a value that
/// does not is rejected here rather than silently truncated, because a
/// truncated amount is a different amount.
PlatformInt64 platformInt64FromBigInt(BigInt value) {
  if (value < _minInt64 || value > _maxInt64) {
    throw ArgumentError.value(value, 'value', 'does not fit the bridge i64');
  }
  return PlatformInt64Util.from(value.toInt());
}
