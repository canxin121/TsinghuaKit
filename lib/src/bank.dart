part of '../tsinghua_kit.dart';

/// Which payroll ledger a read addresses.
///
/// The two ledgers are two path families on one campus host, so they are
/// separate reads rather than one merged list.
enum BankLedger {
  /// `银行代发`
  main,

  /// `银行代发（基金会）`
  foundation,
}

/// One payroll receipt row.
///
/// Every amount is exact integer cents, so a two-decimal service value is
/// never rounded on the way to the caller. The row holds only the columns the
/// service prints on the statement; no account-identifying field is included.
class BankReceipt {
  const BankReceipt({
    required this.department,
    required this.project,
    required this.usage,
    required this.description,
    required this.bank,
    required this.time,
    required this.totalCents,
    required this.deductionCents,
    required this.actualCents,
    required this.depositCents,
    required this.cashCents,
  });

  /// The paying department as the service rendered it.
  final String department;

  /// The project this payment belongs to.
  final String project;

  /// The payment's stated use.
  final String usage;

  /// The service's free-form description, empty when it printed none.
  final String description;

  /// The receiving bank's name.
  final String bank;

  /// The service's tax-time label; no timezone is inferred.
  final String time;

  /// The amount before deductions, in cents.
  ///
  /// `null` when the service printed nothing in that column — a blank column is
  /// not a zero amount.
  final BigInt? totalCents;

  /// The tax withheld, in cents.
  final BigInt? deductionCents;

  /// The amount actually paid, in cents.
  final BigInt? actualCents;

  /// The part paid into the account, in cents.
  final BigInt? depositCents;

  /// The part paid in cash, in cents.
  final BigInt? cashCents;
}

/// One month section of a payroll ledger.
class BankReceiptMonth {
  BankReceiptMonth({required this.month, required List<BankReceipt> receipts})
      : receipts = List.unmodifiable(receipts);

  /// The service's own month heading, e.g. `2021年12月`.
  final String month;

  final List<BankReceipt> receipts;

  bool get isEmpty => receipts.isEmpty;

  int get length => receipts.length;
}

/// One live payroll ledger.
///
/// An empty ledger is a valid answer: the service really does report that an
/// account has no receipts. It is only produced when the response carried the
/// expected month sections, so "no receipts" can never be manufactured from a
/// response that failed to parse.
class BankPaymentLedger {
  BankPaymentLedger({
    required List<BankReceiptMonth> months,
    required this.receiptCount,
  }) : months = List.unmodifiable(months);

  final List<BankReceiptMonth> months;

  /// The number of receipt rows across every month section.
  final BigInt receiptCount;

  bool get isEmpty => receiptCount == BigInt.zero;

  int get monthCount => months.length;
}

/// One graduate-income record.
///
/// Every amount is exact integer cents, and each is `null` when the service
/// printed no value for it.
class GraduateIncomeRecord {
  const GraduateIncomeRecord({
    required this.id,
    required this.year,
    required this.month,
    required this.date,
    required this.yearMonth,
    required this.name,
    required this.department,
    required this.beforeTaxCents,
    required this.afterTaxCents,
    required this.taxCents,
  });

  /// The service's row identifier.
  final String id;

  /// The payment year as the service rendered it.
  final String year;

  /// The payment month as the service rendered it.
  final String month;

  /// The payment date as the service rendered it; no timezone is inferred.
  final String date;

  /// The service's `YYYY年M月` label.
  final String yearMonth;

  /// The payment item's name.
  final String name;

  /// The paying unit's name.
  final String department;

  /// The amount before tax, in cents.
  final BigInt? beforeTaxCents;

  /// The amount actually paid, in cents.
  final BigInt? afterTaxCents;

  /// The tax withheld, in cents.
  final BigInt? taxCents;
}

/// One live page of graduate-income records.
class GraduateIncomePage {
  GraduateIncomePage({
    required List<GraduateIncomeRecord> records,
    required this.total,
  }) : records = List.unmodifiable(records);

  final List<GraduateIncomeRecord> records;

  /// The service's total record count, when it reported one.
  final BigInt? total;

  bool get isEmpty => records.isEmpty;

  int get length => records.length;
}

/// Bank payroll receipts and graduate-income statements through the shared
/// Rust Client.
class BankClient {
  BankClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads one live payroll ledger: the years the service offers this account,
  /// then every receipt those years hold.
  ///
  /// The two ledgers are read separately; each call prepares its own session
  /// for the ledger it addresses.
  Future<ReadResult<BankPaymentLedger>> ledger({
    required BankLedger ledger,
  }) =>
      _sdkCall(
        () async => _bankPaymentLedger(
          await _handle.bankPaymentLedgerResult(
            ledger: ledger == BankLedger.main
                ? native.BankLedgerDto.main
                : native.BankLedgerDto.foundation,
          ),
        ),
      );

  /// Reads one live page of graduate-income records for the inclusive
  /// `YYYYMMDD` range `[begin, end]`.
  ///
  /// Both bounds must be eight digits. A range that is not is refused before
  /// any request, so caller text never becomes a service-side filter.
  Future<ReadResult<GraduateIncomePage>> graduateIncome({
    required String begin,
    required String end,
  }) =>
      _sdkCall(
        () async => _graduateIncomePage(
          await _handle.graduateIncomeResult(begin: begin, end: end),
        ),
      );
}

/// The bridge's 64-bit integer is `int` on native and `BigInt` on the web;
/// `null` means the service printed no value in that column, which is not the
/// same as a zero amount.
BigInt? _optionalInt64ToBigInt(Object? value) =>
    value == null ? null : _platformInt64ToBigInt(value);

ReadResult<BankPaymentLedger> _bankPaymentLedger(
  native.BankPaymentLedgerResultDto value,
) =>
    ReadResult(
      data: BankPaymentLedger(
        months: value.data.months
            .map(
              (month) => BankReceiptMonth(
                month: month.month,
                receipts: month.receipts
                    .map(
                      (receipt) => BankReceipt(
                        department: receipt.department,
                        project: receipt.project,
                        usage: receipt.usage,
                        description: receipt.description,
                        bank: receipt.bank,
                        time: receipt.time,
                        totalCents: _optionalInt64ToBigInt(receipt.totalCents),
                        deductionCents:
                            _optionalInt64ToBigInt(receipt.deductionCents),
                        actualCents:
                            _optionalInt64ToBigInt(receipt.actualCents),
                        depositCents:
                            _optionalInt64ToBigInt(receipt.depositCents),
                        cashCents: _optionalInt64ToBigInt(receipt.cashCents),
                      ),
                    )
                    .toList(growable: false),
              ),
            )
            .toList(growable: false),
        receiptCount: _platformInt64ToBigInt(value.data.receiptCount),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<GraduateIncomePage> _graduateIncomePage(
  native.GraduateIncomeResultDto value,
) =>
    ReadResult(
      data: GraduateIncomePage(
        records: value.data.records
            .map(
              (record) => GraduateIncomeRecord(
                id: record.id,
                year: record.year,
                month: record.month,
                date: record.date,
                yearMonth: record.yearMonth,
                name: record.name,
                department: record.department,
                beforeTaxCents: _optionalInt64ToBigInt(record.beforeTaxCents),
                afterTaxCents: _optionalInt64ToBigInt(record.afterTaxCents),
                taxCents: _optionalInt64ToBigInt(record.taxCents),
              ),
            )
            .toList(growable: false),
        total: _optionalInt64ToBigInt(value.data.total),
      ),
      metadata: _readMetadata(value.metadata),
    );
