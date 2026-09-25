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
