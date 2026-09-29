/// Dormitory laundry rooms served by the three third-party vendors the
/// deployment uses.
///
/// These vendors have no campus-account binding, so these reads are
/// account-independent: they need no proven campus session and report the
/// vendor's own snapshot time. Only building lists and device status are
/// reachable; no ordering, payment, or other write is modelled.
library;

export 'tsinghua_kit.dart'
    show
        LaundryBuilding,
        LaundryBuildingGroup,
        LaundryClient,
        LaundryMachine,
        LaundryProviderOption,
        LaundryRoom,
        LaundryRoomsReport,
        LaundryStatus,
        TsinghuaKitClient;
