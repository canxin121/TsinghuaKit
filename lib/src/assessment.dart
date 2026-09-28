part of '../tsinghua_kit.dart';

/// One teaching-evaluation questionnaire the account may currently fill in.
class AssessmentItem {
  const AssessmentItem({
    required this.name,
    required this.evaluated,
    required this.referenceIndex,
  });

  /// The course name exactly as the service rendered it.
  final String name;

  /// True when the service marked this course as already evaluated.
  final bool evaluated;

  /// An opaque index identifying this row's questionnaire inside the Rust
  /// session that produced it. It is **not** a service URL, and it stops
  /// resolving once that session is replaced or invalidated.
  final int referenceIndex;
}

/// The questionnaires the account may currently fill in.
///
/// A closed questionnaire window is reported as a
/// `TsinghuaKitException` with code `not_available`; it is never presented as
/// a valid empty list.
class AssessmentList {
  AssessmentList({required List<AssessmentItem> items})
      : items = List.unmodifiable(items);

  final List<AssessmentItem> items;

  bool get isEmpty => items.isEmpty;

  int get length => items.length;
}

/// Teaching-evaluation questionnaires through the shared Rust Client.
class AssessmentClient {
  AssessmentClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the questionnaires this account may currently fill in.
  Future<ReadResult<AssessmentList>> list() => _sdkCall(
        () async => _assessmentList(await _handle.assessmentListResult()),
      );
}

ReadResult<AssessmentList> _assessmentList(native.AssessmentListResultDto value) =>
    ReadResult(
      data: AssessmentList(
        items: value.data.items
            .map(
              (item) => AssessmentItem(
                name: item.name,
                evaluated: item.evaluated,
                referenceIndex: item.referenceIndex,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
