import 'rust/sdk_api.dart' as native;

/// A second-factor method understood by the current Identity protocol.
///
/// `unknown` is used only when the server advertises a method that this SDK
/// version does not implement. It must never be submitted as another method.
enum SecondFactorMethod { sms, wechat, mobile, totp, unknown }

/// Maps a server-advertised wire name without guessing unsupported methods.
SecondFactorMethod secondFactorMethodFromServer(String value) =>
    switch (value) {
      'sms' => SecondFactorMethod.sms,
      'wechat' => SecondFactorMethod.wechat,
      'mobile' => SecondFactorMethod.mobile,
      'totp' => SecondFactorMethod.totp,
      _ => SecondFactorMethod.unknown,
    };

/// Converts a supported user selection to the generated bridge enum.
///
/// Unsupported server methods stay visible to the caller and fail locally
/// before a request can be sent.
native.SecondFactorMethodDto secondFactorMethodToBridge(
  SecondFactorMethod value,
) =>
    switch (value) {
      SecondFactorMethod.sms => native.SecondFactorMethodDto.sms,
      SecondFactorMethod.wechat => native.SecondFactorMethodDto.wechat,
      SecondFactorMethod.mobile => native.SecondFactorMethodDto.mobile,
      SecondFactorMethod.totp => native.SecondFactorMethodDto.totp,
      SecondFactorMethod.unknown => throw ArgumentError.value(
          value,
          'method',
          'The server returned an unsupported second-factor method.',
        ),
    };
