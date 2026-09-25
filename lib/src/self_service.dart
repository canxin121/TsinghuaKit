part of '../tsinghua_kit.dart';

/// Read-only account, device, and usage data for the independent SelfService
/// authentication domain.
///
/// Login and logout are managed separately through `client.auth.selfService`.
class SelfServiceClient {
  SelfServiceClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads account details and masked contact fields from SelfService.
  Future<ReadResult<SelfServiceAccount>> account() => _sdkCall(
        () async => _selfServiceAccountResult(
          await _handle.selfServiceAccount(),
        ),
      );

  /// Reads current devices. Their references are scoped to this Client and
  /// this exact list snapshot.
  Future<ReadResult<List<SelfServiceDevice>>> onlineDevices() => _sdkCall(
        () async => _selfServiceDevicesResult(
          await _handle.selfServiceOnlineDevices(),
        ),
      );

  /// Disconnects a device selected from this Client's latest device list.
  /// The reference is consumed before Rust sends the one-shot operation.
  Future<void> disconnectDevice({
    required SelfServiceDeviceReference device,
  }) =>
      _sdkCall(
        () => _handle.selfServiceDisconnectDevice(
          referenceId: device._id,
        ),
      );

  /// Reads the service-formatted usage, balance, and settlement values.
  Future<ReadResult<SelfServiceUsage>> usage() => _sdkCall(
        () async => _selfServiceUsageResult(await _handle.selfServiceUsage()),
      );
}

/// Account details returned by the authenticated SelfService page.
class SelfServiceAccount {
  const SelfServiceAccount({
    required this.contactEmail,
    required this.contactPhone,
    required this.contactLandline,
    required this.realName,
    required this.status,
    required this.userGroup,
    required this.location,
    required this.allowedDevices,
  });

  final String contactEmail;
  final String contactPhone;
  final String contactLandline;
  final String realName;
  final String status;
  final String userGroup;
  final String location;
  final int allowedDevices;
}

/// Opaque, Client-bound handle for one current SelfService device.
class SelfServiceDeviceReference {
  const SelfServiceDeviceReference._(this._id);

  final String _id;
}

/// A currently online SelfService device.
class SelfServiceDevice {
  const SelfServiceDevice({
    required this.reference,
    required this.ipv4,
    required this.ipv6,
    required this.loggedAt,
    required this.authorization,
    required this.macSuffix,
  });

  final SelfServiceDeviceReference reference;
  final String ipv4;
  final String ipv6;
  final String loggedAt;
  final String authorization;
  final String macSuffix;
}

/// Service-formatted SelfService usage, balance, and settlement values.
class SelfServiceUsage {
  const SelfServiceUsage({
    required this.productName,
    required this.usedBytes,
    required this.usedSeconds,
    required this.accountBalance,
    required this.settlementDate,
  });

  final String productName;
  final String usedBytes;
  final String usedSeconds;
  final String accountBalance;
  final String settlementDate;
}

ReadResult<SelfServiceAccount> _selfServiceAccountResult(
  native.SelfServiceAccountResultDto value,
) =>
    ReadResult(
      data: SelfServiceAccount(
        contactEmail: value.data.contactEmail,
        contactPhone: value.data.contactPhone,
        contactLandline: value.data.contactLandline,
        realName: value.data.realName,
        status: value.data.status,
        userGroup: value.data.userGroup,
        location: value.data.location,
        allowedDevices: value.data.allowedDevices,
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<List<SelfServiceDevice>> _selfServiceDevicesResult(
  native.SelfServiceDevicesResultDto value,
) =>
    ReadResult(
      data: value.devices
          .map(
            (device) => SelfServiceDevice(
              reference: SelfServiceDeviceReference._(device.referenceId),
              ipv4: device.ipv4,
              ipv6: device.ipv6,
              loggedAt: device.loggedAt,
              authorization: device.authorization,
              macSuffix: device.macSuffix,
            ),
          )
          .toList(growable: false),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<SelfServiceUsage> _selfServiceUsageResult(
  native.SelfServiceUsageResultDto value,
) =>
    ReadResult(
      data: SelfServiceUsage(
        productName: value.data.productName,
        usedBytes: value.data.usedBytes,
        usedSeconds: value.data.usedSeconds,
        accountBalance: value.data.accountBalance,
        settlementDate: value.data.settlementDate,
      ),
      metadata: _readMetadata(value.metadata),
    );
