part of '../tsinghua_kit.dart';

/// The source's numeric remainder and its original update-time label.
class ElectricityRemainder {
  const ElectricityRemainder({required this.value, required this.updateTime});

  /// The API has not established a unit or scale for this number.
  final double value;

  /// Source-local display label; no timezone is inferred.
  final String updateTime;
}

class ElectricityPaymentRecord {
  const ElectricityPaymentRecord({
    required this.occurredAt,
    required this.amount,
    required this.status,
  });

  /// Source timestamp label; no timezone is inferred.
  final String occurredAt;

  /// Numeric amount as supplied. The source does not establish its unit/scale.
  final double amount;
  final String status;
}

/// A structurally verified payment-history result.
class ElectricityPaymentHistory {
  ElectricityPaymentHistory({
    required this.isEmpty,
    required List<ElectricityPaymentRecord> records,
  }) : records = List.unmodifiable(records);

  final bool isEmpty;
  final List<ElectricityPaymentRecord> records;
}

/// Dorm-electricity reads through the shared Rust Client.
class ElectricityClient {
  ElectricityClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the current numeric remainder with provenance metadata.
  Future<ReadResult<ElectricityRemainder>> remainder() => _sdkCall(
        () async => _electricityRemainderResult(
          await _handle.electricityRemainder(),
        ),
      );

  /// Reads the complete validated payment-history table.
  Future<ReadResult<ElectricityPaymentHistory>> paymentHistory() => _sdkCall(
        () async => _electricityPaymentHistoryResult(
          await _handle.electricityPaymentHistory(),
        ),
      );

  /// Replaces the dormitory service account's own password.
  ///
  /// The dormitory application is reached through the same proven session as
  /// the electricity reads; Rust never establishes a second one, and the
  /// password is copied into exactly one request body and zeroized after it.
  ///
  /// The reset is dispatched **exactly once** and is never retried.  When the
  /// service's answer carries no affirmative acceptance — the ordinary outcome
  /// for this route, because the service's own client discards the reply — the
  /// call fails with a [TsinghuaKitException] whose `code` is
  /// `outcome_unconfirmed`.  That means the change may already be in effect, so
  /// it must not be resolved by calling this again; signing in with the new
  /// password is how a caller finds out what happened.
  Future<void> resetHomePassword(String newPassword) => _sdkCall(
        () => _handle.electricityResetHomePassword(newPassword: newPassword),
      );

  /// The longest password Rust will put into the dormitory reset body.
  ///
  /// This is a local bound rather than an observed service rule: it exists so a
  /// mistyped value is refused before a request is built. Rust refuses an empty,
  /// whitespace-only, over-length or control-character value.
  static const int maxHomePasswordChars = 64;
}

ReadResult<ElectricityRemainder> _electricityRemainderResult(
  native.ElectricityRemainderResultDto value,
) =>
    ReadResult(
      data: ElectricityRemainder(
        value: value.data.value,
        updateTime: value.data.updateTime,
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<ElectricityPaymentHistory> _electricityPaymentHistoryResult(
  native.ElectricityPaymentHistoryResultDto value,
) =>
    ReadResult(
      data: ElectricityPaymentHistory(
        isEmpty: value.data.empty,
        records: value.data.records
            .map(
              (record) => ElectricityPaymentRecord(
                occurredAt: record.occurredAt,
                amount: record.amount,
                status: record.status,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
