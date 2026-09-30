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

  /// The smallest bank top-up the service's own input rule accepts, in fen.
  static final BigInt minTopUpCents = BigInt.from(1000);

  /// The largest bank top-up that rule accepts, in fen.
  static final BigInt maxTopUpCents = BigInt.from(20000);

  /// The largest spending limit this SDK will send, in fen.
  ///
  /// No service-side bound is observed, so this is this SDK's own limit: it
  /// exists so a mistyped amount is refused here instead of sent as a limit
  /// nobody could mean.
  static final BigInt maxLimitCents = BigInt.from(100000000);

  /// Reports the card lost, so the card service blocks it.
  ///
  /// [transactionPassword] is the card's own service password (six digits),
  /// not the account password and not the card-SSO password
  /// [submitPassword] carries. It is validated, sent once and dropped by Rust;
  /// it is never stored, logged or written into any result.
  ///
  /// The change leaves **exactly once**. If this future completes without
  /// throwing, the service accepted it. `outcome_unconfirmed` (see the
  /// [SdkError] code) means the request was sent and its effect is unknown: it
  /// is not sent again, not by this call and not by any retry, so read
  /// [account] to learn the card's state instead of retrying.
  /// `authentication_rejected` is the service's own refusal — nothing was
  /// applied and the password was wrong.
  Future<void> reportLoss(String transactionPassword) => _sdkCall(
        () => _handle.campusCardReportLoss(
          transactionPassword: transactionPassword,
        ),
      );

  /// Reverses a loss report, making the card spendable again.
  ///
  /// The one card change whose success makes a blocked card usable again; it
  /// obeys the same single-dispatch rule as [reportLoss].
  Future<void> cancelLoss(String transactionPassword) => _sdkCall(
        () => _handle.campusCardCancelLoss(
          transactionPassword: transactionPassword,
        ),
      );

  /// Replaces the card's transaction password.
  ///
  /// Both secrets are zeroized after the one request. A password that may
  /// already be in effect is not the old one, so this must not be retried with
  /// the old value after an `outcome_unconfirmed`.
  Future<void> changeTransactionPassword({
    required String oldPassword,
    required String newPassword,
  }) =>
      _sdkCall(
        () => _handle.campusCardChangeTransactionPassword(
          oldPassword: oldPassword,
          newPassword: newPassword,
        ),
      );

  /// Changes the card's two spending limits, in fen.
  ///
  /// The two parameters are named after the wire fields they fill, not after a
  /// meaning. The card service's own client pairs `maxconsamt` and
  /// `maxconstolamt` the opposite way round in its read and write halves, and
  /// there is no evidence available here that says which pairing was
  /// transposed, so this surface refuses to guess: a caller sets the two
  /// fields deliberately. Both are bounded by [maxLimitCents] before any
  /// request exists.
  ///
  /// The physical card identifier the route also needs is read by Rust from a
  /// fresh account read bound to the proven session; it is not a parameter and
  /// never crosses this boundary.
  Future<void> modifySpendingLimit({
    required String transactionPassword,
    required BigInt maxconsamtCents,
    required BigInt maxconstolamtCents,
  }) =>
      _sdkCall(
        () => _handle.campusCardModifySpendingLimit(
          transactionPassword: transactionPassword,
          maxconsamtCents: platformInt64FromBigInt(maxconsamtCents),
          maxconstolamtCents: platformInt64FromBigInt(maxconstolamtCents),
        ),
      );

  /// Transfers money from the bound bank account onto the card (圈存).
  ///
  /// [amountCents] must be between [minTopUpCents] and [maxTopUpCents]; the
  /// amount is the only input, because the observed route sends no password at
  /// all. A bank transfer that has already left is not undone by retrying, so
  /// an `outcome_unconfirmed` must be resolved by reading [account].
  Future<void> topUpFromBank(BigInt amountCents) => _sdkCall(
        () => _handle.campusCardTopUpFromBank(
          amountCents: platformInt64FromBigInt(amountCents),
        ),
      );
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
