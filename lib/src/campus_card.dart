part of '../tsinghua_kit.dart';

/// Fixed filters accepted for campus-card ledger reads.
enum CampusCardTransactionType { any, consumption, recharge, subsidy, unknown }

/// A target-specific one-shot interaction requested by the card service.
enum CampusCardInteraction { passwordRequired }

/// The validated campus-card account fields exposed by the service.
class CampusCardAccount {
  const CampusCardAccount({
    required this.displayName,
    required this.departmentName,
    required this.departmentId,
    required this.effectiveAt,
    required this.validUntil,
    required this.balanceCents,
    required this.cardStatus,
    required this.lastTransactionAt,
    required this.dailyLimitCents,
    required this.oneTimeLimitCents,
    this.displayNameLatin,
    this.departmentNameLatin,
    this.gender,
  });

  final String displayName;
  final String? displayNameLatin;
  final String departmentName;
  final String? departmentNameLatin;
  final BigInt departmentId;
  final String? gender;

  /// Source display labels. No timezone is inferred.
  final String effectiveAt;
  final String validUntil;
  final BigInt balanceCents;
  final String cardStatus;
  final String lastTransactionAt;
  final BigInt dailyLimitCents;
  final BigInt oneTimeLimitCents;
}

/// One validated ledger row. Amounts use fen and retain the source sign.
class CampusCardTransaction {
  const CampusCardTransaction({
    required this.summary,
    required this.occurredAt,
    required this.postBalanceCents,
    required this.amountCents,
    required this.merchantAddress,
    required this.transactionName,
    this.merchantName,
  });

  final String summary;

  /// Source timestamp label; naive values represent campus wall time.
  final String occurredAt;

  final BigInt postBalanceCents;
  final BigInt amountCents;
  final String merchantAddress;
  final String? merchantName;
  final String transactionName;
}

/// A complete, bounded campus-card transaction result.
class CampusCardTransactions {
  CampusCardTransactions({
    required this.startDate,
    required this.endDate,
    required this.transactionType,
    required List<CampusCardTransaction> items,
  }) : items = List.unmodifiable(items);

  final String startDate;
  final String endDate;
  final CampusCardTransactionType transactionType;
  final List<CampusCardTransaction> items;
}

/// Explicit campus-card operations. The target password is not an Auth
/// account: Rust asks for it, consumes the user-entered value once and
/// zeroizes it. An ambiguous submission is never retried automatically.
class CampusCardClient {
  CampusCardClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the current card account with source and freshness metadata.
  Future<ReadResult<CampusCardAccount>> account() => _sdkCall(
        () async => _campusCardAccountResult(await _handle.campusCardAccount()),
      );

  /// Reads all transactions in an inclusive range of at most 31 campus days.
  Future<ReadResult<CampusCardTransactions>> transactions({
    required String startDate,
    required String endDate,
    CampusCardTransactionType type = CampusCardTransactionType.any,
  }) =>
      _sdkCall(() async => _campusCardTransactionsResult(
            await _handle.campusCardTransactions(
              startDate: startDate,
              endDate: endDate,
              transactionType: _campusCardTransactionType(type),
            ),
          ));

  /// Returns a service-password prompt requested by a preceding card read.
  Future<CampusCardInteraction?> pendingInteraction() => _sdkCall(
        () async {
          final value = await _handle.campusCardPendingInteraction();
          return value == null ? null : CampusCardInteraction.passwordRequired;
        },
      );

  /// Submits one user-entered target password to the active Rust challenge.
  Future<void> submitPassword(String password) => _sdkCall(
        () => _handle.campusCardSubmitPassword(password: password),
      );

  /// Cancels the current password prompt without making a request.
  Future<bool> cancelPasswordChallenge() =>
      _sdkCall(() => _handle.campusCardCancelPasswordChallenge());
}

ReadResult<CampusCardAccount> _campusCardAccountResult(
  native.CampusCardAccountResultDto value,
) =>
    ReadResult(
      data: CampusCardAccount(
        displayName: value.data.displayName,
        displayNameLatin: value.data.displayNameLatin,
        departmentName: value.data.departmentName,
        departmentNameLatin: value.data.departmentNameLatin,
        departmentId: _platformInt64ToBigInt(value.data.departmentId),
        gender: value.data.gender,
        effectiveAt: value.data.effectiveAt,
        validUntil: value.data.validUntil,
        balanceCents: _platformInt64ToBigInt(value.data.balanceCents),
        cardStatus: value.data.cardStatus,
        lastTransactionAt: value.data.lastTransactionAt,
        dailyLimitCents: _platformInt64ToBigInt(value.data.dailyLimitCents),
        oneTimeLimitCents: _platformInt64ToBigInt(value.data.oneTimeLimitCents),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<CampusCardTransactions> _campusCardTransactionsResult(
  native.CampusCardTransactionsResultDto value,
) =>
    ReadResult(
      data: CampusCardTransactions(
        startDate: value.data.startDate,
        endDate: value.data.endDate,
        transactionType: _campusCardTransactionTypeFromNative(
          value.data.transactionType,
        ),
        items: value.data.items
            .map(
              (item) => CampusCardTransaction(
                summary: item.summary,
                occurredAt: item.occurredAt,
                postBalanceCents: _platformInt64ToBigInt(item.postBalanceCents),
                amountCents: _platformInt64ToBigInt(item.amountCents),
                merchantAddress: item.merchantAddress,
                merchantName: item.merchantName,
                transactionName: item.transactionName,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

native.CampusCardTransactionTypeDto _campusCardTransactionType(
  CampusCardTransactionType value,
) =>
    switch (value) {
      CampusCardTransactionType.any => native.CampusCardTransactionTypeDto.any,
      CampusCardTransactionType.consumption =>
        native.CampusCardTransactionTypeDto.consumption,
      CampusCardTransactionType.recharge =>
        native.CampusCardTransactionTypeDto.recharge,
      CampusCardTransactionType.subsidy =>
        native.CampusCardTransactionTypeDto.subsidy,
      CampusCardTransactionType.unknown =>
        native.CampusCardTransactionTypeDto.unknown,
    };

CampusCardTransactionType _campusCardTransactionTypeFromNative(
  native.CampusCardTransactionTypeDto value,
) =>
    switch (value) {
      native.CampusCardTransactionTypeDto.any => CampusCardTransactionType.any,
      native.CampusCardTransactionTypeDto.consumption =>
        CampusCardTransactionType.consumption,
      native.CampusCardTransactionTypeDto.recharge =>
        CampusCardTransactionType.recharge,
      native.CampusCardTransactionTypeDto.subsidy =>
        CampusCardTransactionType.subsidy,
      native.CampusCardTransactionTypeDto.unknown =>
        CampusCardTransactionType.unknown,
    };

BigInt _platformInt64ToBigInt(Object value) =>
    value is BigInt ? value : BigInt.from(value as int);
