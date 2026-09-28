part of '../tsinghua_kit.dart';

/// One issued e-invoice.
///
/// The amounts are exact integer cents, so a two-decimal service value is
/// never rounded on the way to the caller. The service's own record
/// identifier and every payer-identifying field stay inside Rust.
class InvoiceRecord {
  const InvoiceRecord({
    required this.businessNo,
    required this.title,
    required this.issuerDepartment,
    required this.paymentItem,
    required this.invoiceNo,
    required this.issuedOn,
    required this.note,
    required this.kind,
    required this.reimbursable,
    required this.redLetter,
    required this.billAmountCents,
    required this.invoiceAmountCents,
    required this.taxAmountCents,
    required this.referenceId,
  });

  /// The service's business number, used as the row key.
  final String businessNo;

  /// The billed item's human name.
  final String title;

  /// The issuing department's name.
  final String issuerDepartment;

  /// The payment item's name.
  final String paymentItem;

  /// The invoice number as the service rendered it.
  final String invoiceNo;

  /// The issue date as the service rendered it; no timezone is inferred.
  final String issuedOn;

  /// The service's free-form note for this record.
  final String note;

  /// The service's own document-kind label.
  final String kind;

  /// True when the service marked this record reimbursable.
  final bool reimbursable;

  /// True when the service marked this record as a red-letter (reversal) one.
  final bool redLetter;

  /// The billed amount in cents.
  final BigInt billAmountCents;

  /// The invoiced amount in cents.
  final BigInt invoiceAmountCents;

  /// The tax amount in cents.
  final BigInt taxAmountCents;

  /// An opaque handle for this record's document. It is **not** a service URL
  /// or record identifier, and it stops resolving once a later invoice page is
  /// read on the same client or the session is dropped.
  final String? referenceId;
}

/// One live page of issued e-invoices.
///
/// An empty page with a zero total is a valid answer: the service really does
/// report that an account has no invoices. It is only produced when the
/// response carried a well-formed record array, so "no invoices" can never be
/// manufactured from a response that failed to parse.
class InvoicePage {
  InvoicePage({
    required this.total,
    required List<InvoiceRecord> records,
  }) : records = List.unmodifiable(records);

  /// The service's total record count across all pages.
  final BigInt total;

  final List<InvoiceRecord> records;

  bool get isEmpty => records.isEmpty;

  int get length => records.length;
}

/// One invoice document's bytes.
class InvoiceDocument {
  InvoiceDocument({required this.bytes});

  /// The service's own PDF bytes. A response that was not that document is
  /// reported as a `TsinghuaKitException` instead of being returned here.
  final Uint8List bytes;

  int get length => bytes.length;

  bool get isEmpty => bytes.isEmpty;
}

/// Issued e-invoices through the shared Rust Client.
class InvoiceClient {
  InvoiceClient._(this._handle);

  final native.ClientHandle _handle;

  /// The last one-based page the service accepts through this client.
  static const int maxPage = 1000;

  /// The page size this client requests from the service.
  static const int pageSize = 20;

  /// Reads one page of issued e-invoices.
  ///
  /// [page] is one-based and bounded by [maxPage]; a page outside that range is
  /// refused before any request reaches the service.
  Future<ReadResult<InvoicePage>> list({required int page}) => _sdkCall(
        () async => _invoicePage(
          await _handle.invoiceListResult(page: page),
        ),
      );

  /// Reads one invoice document addressed by a [InvoiceRecord.referenceId].
  ///
  /// The reference must come from this client's most recent [list] result; a
  /// reference from an earlier page or a dropped session is refused, and the
  /// service's own PDF bytes are returned only when the answer really was that
  /// document.
  Future<ReadResult<InvoiceDocument>> document({
    required String referenceId,
  }) =>
      _sdkCall(
        () async => _invoiceDocument(
          await _handle.invoiceDocumentResult(referenceId: referenceId),
        ),
      );
}

ReadResult<InvoicePage> _invoicePage(native.InvoiceListResultDto value) =>
    ReadResult(
      data: InvoicePage(
        total: _platformInt64ToBigInt(value.data.total),
        records: value.data.records
            .map(
              (record) => InvoiceRecord(
                businessNo: record.businessNo,
                title: record.title,
                issuerDepartment: record.issuerDepartment,
                paymentItem: record.paymentItem,
                invoiceNo: record.invoiceNo,
                issuedOn: record.issuedOn,
                note: record.note,
                kind: record.kind,
                reimbursable: record.reimbursable,
                redLetter: record.redLetter,
                billAmountCents: _platformInt64ToBigInt(record.billAmountCents),
                invoiceAmountCents:
                    _platformInt64ToBigInt(record.invoiceAmountCents),
                taxAmountCents: _platformInt64ToBigInt(record.taxAmountCents),
                referenceId: record.referenceId,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<InvoiceDocument> _invoiceDocument(
  native.InvoiceDocumentResultDto value,
) =>
    ReadResult(
      data: InvoiceDocument(bytes: value.data.bytes),
      metadata: _readMetadata(value.metadata),
    );
