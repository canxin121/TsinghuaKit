part of '../tsinghua_kit.dart';

/// Opaque booking handle for one slot of this Client's latest venue read.
///
/// A handle exists only for a slot the venue itself offers for online booking,
/// so an absent handle is the venue's own statement about that slot rather than
/// a read failure. Handing it to [SportsClient.makeOrder] is the only way to
/// book; the venue's own `resHash` is never accepted anywhere.
class SportsSlotReference {
  const SportsSlotReference._(this._id);

  final String _id;
}

/// Opaque withdrawal handle for one reservation of this Client's latest
/// reservation read.
///
/// A handle exists only for a row the venue printed with a cancellation
/// control. It is spent by one [SportsClient.cancelReservation] call.
class SportsReservationReference {
  const SportsReservationReference._(this._id);

  final String _id;
}

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
    required this.bookable,
  });

  /// The slot's own identifier, as the venue wrote it.
  final String resId;

  /// The slot's single-purpose booking reference.
  ///
  /// It is an opaque service value: display it or store it as an opaque string,
  /// never parse it or render it as user-facing text. It is **not** a booking
  /// handle: only [bookable] can name this slot to the venue.
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

  /// The handle that names this slot back to the venue when booking it, or
  /// null when the venue does not offer it for online booking.
  ///
  /// It is minted by Rust from that one read: the venue's booking hash, the
  /// venue and item identifiers, the date and the cost all stay behind it, and
  /// any newer read of the venue replaces the whole set.
  final SportsSlotReference? bookable;
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
  /// An order carries this number and never one a caller supplies, so a caller
  /// cannot make the venue contact a third party.
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
    required this.withdrawable,
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

  /// The handle that names this reservation back to the venue when withdrawing
  /// it, or null when the venue printed the row without a cancellation control.
  ///
  /// An absent handle is the venue's own statement about that reservation
  /// rather than a read failure.
  final SportsReservationReference? withdrawable;
}

/// The booking form's own image challenge, as the venue rendered it.
///
/// The bytes are the venue's own rendering; Rust verified the declared type is
/// a bounded raster image before returning them, so a login page or an error
/// document answered with HTTP 200 is an error rather than an image.
class SportsCaptcha {
  SportsCaptcha({required this.contentType, required Uint8List bytes})
      : bytes = Uint8List.fromList(bytes);

  /// The venue's declared image type, when it declared one.
  final String? contentType;

  /// A caller-owned copy of the validated image bytes.
  final Uint8List bytes;
}

/// Sports-venue availability, reservation records, and the venue's own state
/// changes (`体育场馆`).
///
/// Both reads are live and account-bound: a proven INFO/WebVPN session is
/// required, and nothing here is served from cache.
///
/// The venue's three state changes are modelled: the order form's image
/// challenge, the order itself, and the withdrawal. Each is dispatched exactly
/// once and an outcome the venue did not confirm is reported as
/// `outcome_unconfirmed` and never replayed, because a state change whose
/// answer was lost may already be in effect. Paying is not modelled: the
/// funding-settlement chain that follows an order cannot be reached here, so no
/// call can move money.
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
  ///
  /// This read is also what makes an order possible: a successful read replaces
  /// every [SportsSlotReference] this Client handed out before, because the
  /// previous list no longer describes the venue.
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
  ///
  /// A successful read replaces every [SportsReservationReference] this Client
  /// handed out before.
  Future<ReadResult<List<SportsReservationRecord>>> records() => _sdkCall(
        () async => _sportsRecords(await _handle.sportsRecordsResult()),
      );

  /// Reads the booking form's own image challenge.
  ///
  /// It is an ordinary read on the already-proved venue session, so it may be
  /// repeated: a person whose first image was unreadable asks for another.
  Future<SportsCaptcha> captcha() => _sdkCall(() async {
        final value = await _handle.sportsCaptcha();
        return SportsCaptcha(contentType: value.contentType, bytes: value.bytes);
      });

  /// Places one order for a slot from this Client's latest [resources] read.
  ///
  /// [slot] must be a [SportsResource.bookable] of that read; the venue's own
  /// booking hash, the venue and item identifiers, the date and the cost all
  /// come from the same read. The contact number is the one the venue itself
  /// reported for this account, so a caller cannot make the venue call a third
  /// party.
  ///
  /// [captcha] is a person's own transcription of a [captcha] image. Nothing
  /// here invents, guesses, re-reads or retries one: each attempt is a distinct
  /// order attempt.
  ///
  /// Dispatched exactly once, and the handle is spent: an answer reported as
  /// unconfirmed is **never** resolved by calling this again, because an order
  /// whose answer was lost may already be in effect. Read [records] to learn
  /// what the account now holds.
  Future<void> makeOrder(SportsSlotReference slot, String captcha) => _sdkCall(
        () => _handle.sportsMakeOrder(
          slotReferenceId: slot._id,
          captcha: captcha,
        ),
      );

  /// Withdraws one reservation from this Client's latest [records] read.
  ///
  /// The handle is spent exactly as in [makeOrder]. No client has ever observed
  /// this route's refusal wording, so a readable answer that is not affirmative
  /// is reported as unconfirmed rather than as a refusal.
  Future<void> cancelReservation(SportsReservationReference reservation) =>
      _sdkCall(
        () => _handle.sportsCancelReservation(
          reservationReferenceId: reservation._id,
        ),
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
                bookable: slot.selector == null
                    ? null
                    : SportsSlotReference._(slot.selector!),
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
              withdrawable: record.selector == null
                  ? null
                  : SportsReservationReference._(record.selector!),
            ),
          )
          .toList(growable: false),
      metadata: _readMetadata(value.metadata),
    );
