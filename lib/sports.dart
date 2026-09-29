/// Sports-venue availability and the account's own reservation records
/// (`体育场馆`).
///
/// Both reads are live and account-bound, and both are read-only: this API
/// cannot order, pay, or cancel, so the venue's captcha and payment routes are
/// unreachable through it.
library;

export 'tsinghua_kit.dart'
    show
        ReadResult,
        SportsClient,
        SportsReservationRecord,
        SportsResource,
        SportsResources,
        TsinghuaKitClient;
