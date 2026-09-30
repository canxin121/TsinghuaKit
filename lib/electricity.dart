/// Dormitory electricity balance and payment-history reads, plus the dormitory
/// account's own password reset.
///
/// The reset dispatches exactly once and is never retried; an answer the
/// service did not confirm is reported as `outcome_unconfirmed` rather than
/// failed, because the change may already be in effect.
library;

export 'tsinghua_kit.dart'
    show
        ElectricityClient,
        ElectricityPaymentHistory,
        ElectricityPaymentRecord,
        ElectricityRemainder,
        ReadResult,
        TsinghuaKitClient,
        TsinghuaKitException;
