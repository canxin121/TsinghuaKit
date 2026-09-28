part of '../tsinghua_kit.dart';

/// One physical-education test item: the recorded measurement and the
/// service's own score for it. Both halves are optional and independent.
class PhysicalExamItem {
  const PhysicalExamItem({
    required this.key,
    required this.measurement,
    required this.score,
  });

  /// The item's stable English name, e.g. `vital_capacity`.
  final String key;

  /// The raw value the service recorded, unchanged.
  final String? measurement;

  /// The service's own score for this item.
  final String? score;
}

/// A validated physical-education test report.
class PhysicalExamReport {
  PhysicalExamReport({
    required this.noResult,
    required this.exemption,
    required this.exemptionReason,
    required this.total,
    required this.standardScore,
    required this.bonusScore,
    required this.longRunBonusScore,
    required this.height,
    required this.weight,
    required this.physicalEducationGrade,
    required this.referenceTotal,
    required this.referenceTotalLabel,
    required List<PhysicalExamItem> items,
  }) : items = List.unmodifiable(items);

  /// True when the service answered that this account has no score yet.
  /// Every other field is then empty; this is a valid empty state, not an
  /// error, and never a record filled with zeros.
  final bool noResult;

  final String? exemption;
  final String? exemptionReason;

  /// The service's own total.
  final String? total;

  final String? standardScore;
  final String? bonusScore;
  final String? longRunBonusScore;
  final String? height;
  final String? weight;
  final String? physicalEducationGrade;

  /// A locally recomputed reference value, **not** a service score.
  ///
  /// Rust applies the fixed weights the App has always used to the service's
  /// own item scores. Always show it next to [referenceTotalLabel]; never
  /// present it as a result the service reported. It is null when the service
  /// reported no score at all, or when a reported score could not be read as
  /// a number.
  final double? referenceTotal;

  /// The mandatory label for [referenceTotal].
  final String referenceTotalLabel;

  final List<PhysicalExamItem> items;
}

/// Physical-education test results through the shared Rust Client.
class PhysicalExamClient {
  PhysicalExamClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the validated physical-education report.
  Future<ReadResult<PhysicalExamReport>> result() => _sdkCall(
        () async => _physicalExamResult(await _handle.physicalExamResult()),
      );
}

ReadResult<PhysicalExamReport> _physicalExamResult(
  native.PhysicalExamResultDto value,
) =>
    ReadResult(
      data: PhysicalExamReport(
        noResult: value.data.noResult,
        exemption: value.data.exemption,
        exemptionReason: value.data.exemptionReason,
        total: value.data.total,
        standardScore: value.data.standardScore,
        bonusScore: value.data.bonusScore,
        longRunBonusScore: value.data.longRunBonusScore,
        height: value.data.height,
        weight: value.data.weight,
        physicalEducationGrade: value.data.physicalEducationGrade,
        referenceTotal: value.data.referenceTotal,
        referenceTotalLabel: value.data.referenceTotalLabel,
        items: value.data.items
            .map(
              (item) => PhysicalExamItem(
                key: item.key,
                measurement: item.measurement,
                score: item.score,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
