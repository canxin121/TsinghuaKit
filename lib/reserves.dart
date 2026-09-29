/// The course-reserve textbook collection (`馆藏教参`).
///
/// Both reads are live and account-bound to the INFO/WebVPN session the client
/// already holds, and both are read-only: the service's full-text reader needs
/// a campus identity login this client deliberately does not perform, so an
/// expired session is reported as an authentication failure rather than
/// answered with an empty catalogue.
library;

export 'tsinghua_kit.dart'
    show
        ReadResult,
        ReservesBook,
        ReservesBookDetail,
        ReservesChapter,
        ReservesClient,
        ReservesSearch,
        TsinghuaKitClient;
