/// Sports-venue availability, the account's own reservation records, and the
/// venue's own booking and withdrawal (`体育场馆`).
///
/// Both reads are live and account-bound. The three state changes are the
/// order form's image challenge, the order, and the withdrawal; each is
/// dispatched exactly once and an unconfirmed outcome is never replayed.
/// Paying is not modelled, so no call here can move money.
library;

export 'tsinghua_kit.dart'
    show
        ReadResult,
        SportsCaptcha,
        SportsClient,
        SportsReservationRecord,
        SportsReservationReference,
        SportsResource,
        SportsResources,
        SportsSlotReference,
        TsinghuaKitClient;
