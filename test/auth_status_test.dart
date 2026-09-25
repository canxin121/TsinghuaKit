import 'package:flutter_test/flutter_test.dart';
import 'package:tsinghua_kit/auth.dart';

void main() {
  test('keeps Identity and SelfService account labels in separate slots', () {
    const status = AuthStatus(
      identity: AccountStatus(
        AccountState.authenticated,
        username: 'identity-account',
      ),
      selfService: AccountStatus(
        AccountState.signedOut,
        username: 'selfservice-account',
      ),
    );

    expect(status.identity.username, 'identity-account');
    expect(status.identity.state, AccountState.authenticated);
    expect(status.selfService.username, 'selfservice-account');
    expect(status.selfService.state, AccountState.signedOut);
  });
}
