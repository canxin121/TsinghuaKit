/// 清紫源泉 bottled-water delivery account lookup.
///
/// The vendor runs its own service with no campus-account binding, so this is a
/// third-party read: the caller supplies the room's own delivery number and
/// receives the vendor's record. The vendor's ordering endpoint places a real
/// order and is deliberately not exposed.
library;

export 'tsinghua_kit.dart'
    show WaterBrandOption, WaterClient, WaterUser, TsinghuaKitClient;
