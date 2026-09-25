part of '../tsinghua_kit.dart';

/// Cache behavior for an explicit service-hall read.
enum ServiceHallReadPolicy { cacheOnly, preferFreshCache, refresh }

/// One fixed read-only view from the online service hall.
enum ServiceHallTaskView { completed, drafts, copies, phases, unknown }

/// One read-only task shown in the online service hall.
class ServiceHallTask {
  const ServiceHallTask({
    required this.title,
    required this.status,
    required this.step,
    required this.applicationTime,
    this.progressPercent,
  });

  final String title;
  final String status;
  final String step;

  /// The source's displayed application/start time; it is not a deadline.
  final String applicationTime;

  final int? progressPercent;
}

/// Pending service-hall tasks and the homepage's separately reported count.
class ServiceHallPending {
  ServiceHallPending({
    required List<ServiceHallTask> tasks,
    required this.reportedPendingCount,
    required this.coverage,
  }) : tasks = List.unmodifiable(tasks);

  final List<ServiceHallTask> tasks;

  /// The homepage count may exclude items merged from the returned task list.
  final int reportedPendingCount;

  final ReadCoverage coverage;
}

/// One display-only entry in the online service catalogue.
class ServiceHallService {
  const ServiceHallService({
    required this.name,
    required this.department,
    required this.kind,
    required this.inOpenPeriod,
  });

  final String name;
  final String department;
  final String? kind;
  final bool? inOpenPeriod;
}

/// The service directory and the source's separately reported total.
class ServiceHallDirectory {
  ServiceHallDirectory({
    required List<ServiceHallService> services,
    required this.reportedTotal,
    required this.coverage,
  }) : services = List.unmodifiable(services);

  final List<ServiceHallService> services;
  final int reportedTotal;
  final ReadCoverage coverage;
}

/// Opaque, Client-bound selector for phase details.
///
/// Create one by reading [ServiceHallTask.phaseReference]. The identifier is
/// private to the Dart facade and does not expose the school's selector.
class ServiceHallPhaseReference {
  const ServiceHallPhaseReference._(this._id);

  final String _id;
}

/// One item in a completed, draft, copied, or phased workflow view.
class ServiceHallWorkflowTask {
  const ServiceHallWorkflowTask({
    required this.title,
    required this.status,
    required this.step,
    required this.applicationTime,
    required this.progressPercent,
    this.phaseReference,
  });

  final String title;
  final String status;
  final String step;

  /// The source's displayed application/start time; it is not a deadline.
  final String applicationTime;

  final int? progressPercent;
  final ServiceHallPhaseReference? phaseReference;
}

/// One read-only service-hall task view and its reported total.
class ServiceHallTaskList {
  ServiceHallTaskList({
    required this.view,
    required List<ServiceHallWorkflowTask> tasks,
    required this.reportedTotal,
    required this.coverage,
  }) : tasks = List.unmodifiable(tasks);

  final ServiceHallTaskView view;
  final List<ServiceHallWorkflowTask> tasks;
  final int reportedTotal;
  final ReadCoverage coverage;
}

/// One service item shown within a workflow phase.
class ServiceHallPhaseItem {
  const ServiceHallPhaseItem({required this.name, required this.state});

  final String name;
  final String state;
}

/// One verified stage of a multi-step workflow.
class ServiceHallPhaseStep {
  ServiceHallPhaseStep({
    required this.order,
    required this.name,
    required this.state,
    required List<ServiceHallPhaseItem> items,
  }) : items = List.unmodifiable(items);

  final String order;
  final String name;
  final String state;
  final List<ServiceHallPhaseItem> items;
}

/// Verified, read-only stages for a selected workflow.
class ServiceHallPhaseDetails {
  ServiceHallPhaseDetails({required List<ServiceHallPhaseStep> steps})
      : steps = List.unmodifiable(steps);

  final List<ServiceHallPhaseStep> steps;
}

/// Read-only online service-hall operations backed by this Client's Runtime.
class ServiceHallClient {
  ServiceHallClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads pending items using the caller-selected cache policy.
  ///
  /// This uses the same Rust Client, account state, transport, and request
  /// gate as the other SDK services. The result retains source, freshness,
  /// observation time, and completeness; failures remain typed exceptions.
  Future<ReadResult<ServiceHallPending>> pending({
    required ServiceHallReadPolicy policy,
  }) =>
      _sdkCall(() async => _pendingResult(await _handle.serviceHallPending(
            policy: _serviceHallPolicyDto(policy),
          )));

  /// Reads the complete service catalogue with source and completeness data.
  Future<ReadResult<ServiceHallDirectory>> services({
    required ServiceHallReadPolicy policy,
  }) =>
      _sdkCall(() async => _serviceHallDirectoryResult(
            await _handle.serviceHallServices(
              policy: _serviceHallPolicyDto(policy),
            ),
          ));

  /// Reads completed items, drafts, copies, or multi-stage workflows.
  ///
  /// Only a complete phase view supplies [ServiceHallPhaseReference] values.
  /// They become invalid when their Client's phase list is refreshed.
  Future<ReadResult<ServiceHallTaskList>> tasks({
    required ServiceHallTaskView view,
    required ServiceHallReadPolicy policy,
  }) =>
      _sdkCall(() async => _serviceHallTaskListResult(
            await _handle.serviceHallTasks(
              view: _serviceHallTaskViewDto(view),
              policy: _serviceHallPolicyDto(policy),
            ),
          ));

  /// Reads stage details from a task reference produced by this Client.
  Future<ReadResult<ServiceHallPhaseDetails>> phaseDetails({
    required ServiceHallPhaseReference reference,
    required ServiceHallReadPolicy policy,
  }) =>
      _sdkCall(() async => _serviceHallPhaseDetailsResult(
            await _handle.serviceHallPhaseDetails(
              referenceId: reference._id,
              policy: _serviceHallPolicyDto(policy),
            ),
          ));
}

native.ServiceHallReadPolicyDto _serviceHallPolicyDto(
  ServiceHallReadPolicy value,
) =>
    switch (value) {
      ServiceHallReadPolicy.cacheOnly =>
        native.ServiceHallReadPolicyDto.cacheOnly,
      ServiceHallReadPolicy.preferFreshCache =>
        native.ServiceHallReadPolicyDto.preferFreshCache,
      ServiceHallReadPolicy.refresh => native.ServiceHallReadPolicyDto.refresh,
    };

native.ServiceHallTaskViewDto _serviceHallTaskViewDto(
  ServiceHallTaskView value,
) =>
    switch (value) {
      ServiceHallTaskView.completed => native.ServiceHallTaskViewDto.completed,
      ServiceHallTaskView.drafts => native.ServiceHallTaskViewDto.drafts,
      ServiceHallTaskView.copies => native.ServiceHallTaskViewDto.copies,
      ServiceHallTaskView.phases => native.ServiceHallTaskViewDto.phases,
      ServiceHallTaskView.unknown => native.ServiceHallTaskViewDto.unknown,
    };

ServiceHallTaskView _serviceHallTaskView(
  native.ServiceHallTaskViewDto value,
) =>
    switch (value) {
      native.ServiceHallTaskViewDto.completed => ServiceHallTaskView.completed,
      native.ServiceHallTaskViewDto.drafts => ServiceHallTaskView.drafts,
      native.ServiceHallTaskViewDto.copies => ServiceHallTaskView.copies,
      native.ServiceHallTaskViewDto.phases => ServiceHallTaskView.phases,
      native.ServiceHallTaskViewDto.unknown => ServiceHallTaskView.unknown,
    };

ReadResult<ServiceHallPending> _pendingResult(
  native.ServiceHallPendingResultDto value,
) =>
    ReadResult(
      data: ServiceHallPending(
        tasks: value.data.tasks
            .map(
              (task) => ServiceHallTask(
                title: task.title,
                status: task.status,
                step: task.step,
                applicationTime: task.applicationTime,
                progressPercent: task.progressPercent,
              ),
            )
            .toList(growable: false),
        reportedPendingCount: value.data.reportedPendingCount,
        coverage: _readCoverage(value.data.coverage),
      ),
      metadata: ReadMetadata(
        source: _readSource(value.metadata.source),
        freshness: _cacheFreshness(value.metadata.freshness),
        observedAt: DateTime.parse(value.metadata.observedAtUtc).toUtc(),
        coverage: _readCoverage(value.metadata.coverage),
        refreshFailureCode: value.metadata.refreshFailureCode,
      ),
    );

ReadResult<ServiceHallDirectory> _serviceHallDirectoryResult(
  native.ServiceHallDirectoryResultDto value,
) =>
    ReadResult(
      data: ServiceHallDirectory(
        services: value.data.services
            .map(
              (service) => ServiceHallService(
                name: service.name,
                department: service.department,
                kind: service.kind,
                inOpenPeriod: service.inOpenPeriod,
              ),
            )
            .toList(growable: false),
        reportedTotal: value.data.reportedTotal,
        coverage: _readCoverage(value.data.coverage),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<ServiceHallTaskList> _serviceHallTaskListResult(
  native.ServiceHallTaskListResultDto value,
) =>
    ReadResult(
      data: ServiceHallTaskList(
        view: _serviceHallTaskView(value.data.view),
        tasks: value.data.tasks
            .map(
              (task) => ServiceHallWorkflowTask(
                title: task.title,
                status: task.status,
                step: task.step,
                applicationTime: task.applicationTime,
                progressPercent: task.progressPercent,
                phaseReference: task.phaseReferenceId == null
                    ? null
                    : ServiceHallPhaseReference._(task.phaseReferenceId!),
              ),
            )
            .toList(growable: false),
        reportedTotal: value.data.reportedTotal,
        coverage: _readCoverage(value.data.coverage),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<ServiceHallPhaseDetails> _serviceHallPhaseDetailsResult(
  native.ServiceHallPhaseDetailsResultDto value,
) =>
    ReadResult(
      data: ServiceHallPhaseDetails(
        steps: value.data.steps
            .map(
              (step) => ServiceHallPhaseStep(
                order: step.order,
                name: step.name,
                state: step.state,
                items: step.items
                    .map(
                      (item) => ServiceHallPhaseItem(
                        name: item.name,
                        state: item.state,
                      ),
                    )
                    .toList(growable: false),
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadMetadata _readMetadata(native.ReadMetadataDto value) => ReadMetadata(
      source: _readSource(value.source),
      freshness: _cacheFreshness(value.freshness),
      observedAt: DateTime.parse(value.observedAtUtc).toUtc(),
      coverage: _readCoverage(value.coverage),
      refreshFailureCode: value.refreshFailureCode,
    );

ReadSource _readSource(native.ReadSourceDto value) => switch (value) {
      native.ReadSourceDto.live => ReadSource.live,
      native.ReadSourceDto.memoryCache => ReadSource.memoryCache,
      native.ReadSourceDto.clientCache => ReadSource.clientCache,
      native.ReadSourceDto.persistentCache => ReadSource.persistentCache,
      native.ReadSourceDto.unknown => ReadSource.unknown,
    };

CacheFreshness _cacheFreshness(native.CacheFreshnessDto value) =>
    switch (value) {
      native.CacheFreshnessDto.fresh => CacheFreshness.fresh,
      native.CacheFreshnessDto.stale => CacheFreshness.stale,
      native.CacheFreshnessDto.notApplicable => CacheFreshness.notApplicable,
      native.CacheFreshnessDto.unknown => CacheFreshness.unknown,
    };

ReadCoverage _readCoverage(native.ReadCoverageDto value) => switch (value) {
      native.ReadCoverageDto.complete => ReadCoverage.complete,
      native.ReadCoverageDto.partialReadLimitReached =>
        ReadCoverage.partialReadLimitReached,
      native.ReadCoverageDto.partialSourceChanged =>
        ReadCoverage.partialSourceChanged,
      native.ReadCoverageDto.partialCompletionUnconfirmed =>
        ReadCoverage.partialCompletionUnconfirmed,
      native.ReadCoverageDto.unknown => ReadCoverage.unknown,
    };
