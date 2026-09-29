part of '../tsinghua_kit.dart';

/// One bookable slot the venue reports for one date.
class SportsResource {
  const SportsResource({
    required this.resId,
    required this.resHash,
    required this.timeSession,
    required this.fieldName,
    required this.overlaySize,
    required this.canNetBook,
    required this.cost,
    required this.bookId,
    required this.locked,
    required this.userType,
    required this.paymentStatus,
  });

  /// The slot's own identifier, as the venue wrote it.
  final String resId;

  /// The slot's single-purpose booking reference.
  ///
  /// It is an opaque service value: display it or store it as an opaque string,
  /// never parse it or render it as user-facing text.
  final String resHash;

  /// The venue's time-session label, e.g. `20:00-21:00`.
  final String timeSession;

  /// The court or table this slot belongs to.
  final String fieldName;

  /// The venue's own overlay size, absent when it reported none.
  final int? overlaySize;

  /// Whether the venue offers this slot for online booking.
  final bool canNetBook;

  /// The venue's own cost token, unchanged.
  ///
  /// The service supplies no unit for it, so this client does not convert it
  /// into a currency amount. Show it verbatim or not at all.
  final String? cost;

  /// The order this slot is already attached to, when it is.
  final String? bookId;

  /// The venue's own lock flag, absent when it did not mark the slot.
  final bool? locked;

  /// The user type the venue tagged the slot with, when it did.
  final String? userType;

  /// Whether the venue marked the slot's payment as settled.
  final bool? paymentStatus;
}

/// One venue's slot list for one date, with the venue's own booking limits.
class SportsResources {
  SportsResources({
    required this.count,
    required this.init,
    required this.phone,
    required List<SportsResource> data,
  }) : data = List.unmodifiable(data);

  /// The venue's booking limit, as it reported it.
  final int count;

  /// The venue's initial booking limit, as it reported it.
  final int init;

  /// The phone number the account has configured with the venue, or null when
  /// it has none.
  ///
  /// It is the account holder's own number. Never log it or send it anywhere.
  final String? phone;

  final List<SportsResource> data;
}

/// One row of the account's own sports reservation list.
class SportsReservationRecord {
  const SportsReservationRecord({
    required this.name,
    required this.field,
    required this.time,
    required this.price,
    required this.method,
    required this.bookTimestamp,
    required this.bookId,
    required this.payId,
  });

  /// The venue's name, as it wrote it.
  final String name;

  /// The court or table.
  final String field;

  /// The venue's time text, unchanged.
  final String time;

  /// The venue's own price text, unchanged. Like a slot's cost, it carries no
  /// unit this client can justify converting.
  final String price;

  /// The venue's payment-method label, e.g. `网上支付` or `已支付`.
  final String method;

  /// The venue's own booking timestamp, unchanged, absent when it reported
  /// none.
  ///
  /// The evidence does not establish a unit for it, so it is reported as the
  /// service wrote it rather than converted into a `DateTime` this client
  /// cannot justify.
  final int? bookTimestamp;

  /// The order reference this row belongs to, when the venue gave one. Opaque:
  /// never parse or display it as text.
  final String? bookId;

  /// The payment reference this row belongs to, when the venue gave one.
  /// Opaque, same as [bookId].
  final String? payId;
}

/// Sports-venue availability and reservation records (`体育场馆`).
///
/// Both reads are live and account-bound: a proven INFO/WebVPN session is
/// required, and nothing here is served from cache.
///
/// This API is read-only by construction. Ordering, paying, cancelling, the
/// venue's captcha, and the phone-number update route are not modelled, so
/// they cannot be reached through it.
class SportsClient {
  SportsClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads one venue's limits, configured phone number, and slot list for one
  /// date.
  ///
  /// [gymId] and [itemId] must be digit strings the caller already holds, and
  /// [date] a real calendar day in `YYYY-MM-DD`. Anything else is refused in
  /// Rust before any request is made.
  ///
  /// A venue that reports no slots for the date returns an empty list; a page
  /// this client cannot read is an error, never an empty venue.
  Future<ReadResult<SportsResources>> resources({
    required String gymId,
    required String itemId,
    required String date,
  }) =>
      _sdkCall(
        () async => _sportsResources(
          await _handle.sportsResourcesResult(
            gymId: gymId,
            itemId: itemId,
            date: date,
          ),
        ),
      );

  /// Reads the account's unpaid sports reservations followed by its paid ones.
  ///
  /// Each row carries the venue's own [SportsReservationRecord.method] label,
  /// so a caller can tell the two sections apart without guessing.
  Future<ReadResult<List<SportsReservationRecord>>> records() => _sdkCall(
        () async => _sportsRecords(await _handle.sportsRecordsResult()),
      );
}

ReadResult<SportsResources> _sportsResources(
  native.SportsResourcesResultDto value,
) =>
    ReadResult(
      data: SportsResources(
        count: value.data.count,
        init: value.data.init,
        phone: value.data.phone,
        data: value.data.data
            .map(
              (slot) => SportsResource(
                resId: slot.resId,
                resHash: slot.resHash,
                timeSession: slot.timeSession,
                fieldName: slot.fieldName,
                overlaySize: slot.overlaySize,
                canNetBook: slot.canNetBook,
                cost: slot.cost,
                bookId: slot.bookId,
                locked: slot.locked,
                userType: slot.userType,
                paymentStatus: slot.paymentStatus,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<List<SportsReservationRecord>> _sportsRecords(
  native.SportsRecordsResultDto value,
) =>
    ReadResult(
      data: value.data.records
          .map(
            (record) => SportsReservationRecord(
              name: record.name,
              field: record.field,
              time: record.time,
              price: record.price,
              method: record.method,
              bookTimestamp: record.bookTimestamp == null
                  ? null
                  : _platformInt64ToBigInt(record.bookTimestamp!).toInt(),
              bookId: record.bookId,
              payId: record.payId,
            ),
          )
          .toList(growable: false),
      metadata: _readMetadata(value.metadata),
    );
