import 'package:flutter_test/flutter_test.dart';
import 'package:tsinghua_kit/src/second_factor_method.dart' as mapping;
import 'package:tsinghua_kit/src/rust/sdk_api.dart' as native;
import 'package:tsinghua_kit/tsinghua_kit.dart';

void main() {
  test('public API exports supported second-factor methods', () {
    expect(
      mapping.secondFactorMethodToBridge(SecondFactorMethod.sms),
      native.SecondFactorMethodDto.sms,
    );
    expect(
      mapping.secondFactorMethodFromServer('totp'),
      SecondFactorMethod.totp,
    );
  });

  test('preserves server methods this SDK does not support', () {
    expect(
      mapping.secondFactorMethodFromServer('push-webauthn'),
      SecondFactorMethod.unknown,
    );
  });

  test('does not submit an unknown method as TOTP', () {
    expect(
      () => mapping.secondFactorMethodToBridge(SecondFactorMethod.unknown),
      throwsArgumentError,
    );
  });
}
