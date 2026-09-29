part of '../tsinghua_kit.dart';

/// One room's own bottled-water delivery record, as the vendor holds it.
class WaterUser {
  const WaterUser({required this.name, required this.address});

  /// The name the vendor holds for this delivery number.
  final String name;

  /// The delivery address the vendor holds for this delivery number.
  final String address;
}

/// One selectable water brand with its display label.
class WaterBrandOption {
  const WaterBrandOption({required this.key, required this.label});

  /// The vendor's own brand identifier.
  final String key;

  final String label;
}

/// 清紫源泉 bottled-water delivery account lookup.
///
/// The vendor runs its own service with no campus-account binding, so this is a
/// third-party read: the caller supplies the room's own delivery number and
/// receives the vendor's record. No campus credential is sent and no proven
/// campus session is required.
///
/// The vendor's ordering endpoint places a real order and is deliberately not
/// exposed here.
class WaterClient {
  WaterClient._(this._handle);

  final native.ClientHandle _handle;

  /// The vendor's brands with the labels it prints for them.
  ///
  /// This asks Rust, so a brand the vendor adds outside the built-in table
  /// stays visible under its own identifier instead of being relabelled or
  /// dropped.
  static Future<List<WaterBrandOption>> brands() async {
    await TsinghuaKit.initialize();
    final values = await native.waterBrands();
    return values
        .map((value) => WaterBrandOption(key: value.key, label: value.label))
        .toList(growable: false);
  }

  /// Looks up the vendor's record for one delivery number.
  ///
  /// The number is the caller's own input; a value this client will not send is
  /// reported as invalid input without any request.
  Future<WaterUser> user({required String deliveryId}) => _sdkCall(
        () async => _waterUser(await _handle.waterUser(deliveryId: deliveryId)),
      );
}

WaterUser _waterUser(native.WaterUserDto value) =>
    WaterUser(name: value.name, address: value.address);
