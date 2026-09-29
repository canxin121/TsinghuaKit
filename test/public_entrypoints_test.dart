import 'package:flutter_test/flutter_test.dart';
import 'package:tsinghua_kit/assessment.dart' as assessment;
import 'package:tsinghua_kit/auth.dart' as auth;
import 'package:tsinghua_kit/bank.dart' as bank;
import 'package:tsinghua_kit/campus_card.dart' as campus_card;
import 'package:tsinghua_kit/classrooms.dart' as classrooms;
import 'package:tsinghua_kit/core.dart' as core;
import 'package:tsinghua_kit/course_score.dart' as course_score;
import 'package:tsinghua_kit/electricity.dart' as electricity;
import 'package:tsinghua_kit/invoice.dart' as invoice;
import 'package:tsinghua_kit/laundry.dart' as laundry;
import 'package:tsinghua_kit/learn.dart' as learn;
import 'package:tsinghua_kit/library.dart' as library_api;
import 'package:tsinghua_kit/network.dart' as network;
import 'package:tsinghua_kit/news.dart' as news;
import 'package:tsinghua_kit/overview.dart' as overview;
import 'package:tsinghua_kit/physical_exam.dart' as physical_exam;
import 'package:tsinghua_kit/program.dart' as program;
import 'package:tsinghua_kit/read.dart' as read;
import 'package:tsinghua_kit/registrar_calendar.dart' as calendar;
import 'package:tsinghua_kit/self_service.dart' as self_service;
import 'package:tsinghua_kit/service_hall.dart' as service_hall;
import 'package:tsinghua_kit/sports.dart' as sports;
import 'package:tsinghua_kit/water.dart' as water;

T? _publicType<T>() => null;

void main() {
  test('domain entrypoints expose curated stable types', () {
    expect(_publicType<core.TsinghuaKit>(), isNull);
    expect(_publicType<core.TsinghuaKitException>(), isNull);
    expect(
      core.ClientCachePersistence.memoryOnly(),
      isA<core.MemoryOnlyClientCachePersistence>(),
    );
    expect(
      _publicType<core.DirectoryClientCachePersistence>(),
      isNull,
    );
    expect(
      core.IdentitySessionPersistence.memoryOnly(),
      isA<core.MemoryOnlyIdentitySessionPersistence>(),
    );
    expect(
      core.IdentitySessionPersistence.jsonDirectory(
        root: '/app/private',
        namespace: 'org.example.app',
      ),
      isA<core.JsonDirectoryIdentitySessionPersistence>(),
    );
    expect(
      core.NetworkProfilePersistence.jsonDirectory(
        root: '/app/private',
        namespace: 'org.example.app',
      ),
      isA<core.JsonDirectoryNetworkProfilePersistence>(),
    );
    expect(
      auth.AuthCredentialPersistence.memoryOnly(),
      isA<auth.MemoryOnlyAuthCredentialPersistence>(),
    );
    expect(
      auth.AuthCredentialPersistence.jsonDirectory(
        root: '/app/private',
        namespace: 'org.example.app',
      ),
      isA<auth.JsonDirectoryAuthCredentialPersistence>(),
    );
    expect(auth.SecondFactorMethod.sms.name, 'sms');
    expect(network.NetworkAccessMethod.systemWifiEap.name, 'systemWifiEap');
    expect(network.PortalConnectionState.connected.name, 'connected');
    expect(_publicType<network.PortalConnectionResult>(), isNull);
    expect(service_hall.ServiceHallTaskView.phases.name, 'phases');
    expect(_publicType<service_hall.ServiceHallDirectory>(), isNull);
    expect(news.NewsCatalogCoverage.complete.name, 'complete');
    // The news writes consume the same opaque references the reads hand out,
    // and every reference type has a private constructor, so a caller can only
    // obtain one from this Client's own reads.  A type-level assertion is the
    // strongest check that fits without an instance.
    expect(_publicType<news.NewsArticleReference>(), isNull);
    expect(_publicType<news.NewsSubscriptionReference>(), isNull);
    expect(_publicType<news.NewsChannelReference>(), isNull);
    expect(_publicType<news.NewsSourceReference>(), isNull);
    expect(learn.LearnHomeworkState.pending.name, 'pending');
    expect(calendar.AcademicStage.undergraduate.name, 'undergraduate');
    expect(library_api.LibraryDay.today.name, 'today');
    expect(classrooms.ClassroomSlotStatus.unknown.name, 'unknown');
    expect(campus_card.CampusCardTransactionType.any.name, 'any');
    expect(_publicType<self_service.SelfServiceClient>(), isNull);
    expect(_publicType<auth.SelfServiceAuthClient>(), isNull);
    expect(_publicType<electricity.ElectricityClient>(), isNull);
    expect(_publicType<read.ReadResult<int>>(), isNull);
    expect(_publicType<overview.OverviewClient>(), isNull);
    expect(_publicType<overview.DailyOverview>(), isNull);
    expect(overview.OverviewScheduleKind.course.name, 'course');
    expect(_publicType<physical_exam.PhysicalExamClient>(), isNull);
    expect(_publicType<physical_exam.PhysicalExamItem>(), isNull);
    expect(_publicType<physical_exam.PhysicalExamReport>(), isNull);
    expect(_publicType<program.ProgramClient>(), isNull);
    expect(_publicType<program.ProgramCompletion>(), isNull);
    expect(program.ProgramCourseState.completed.name, 'completed');
    expect(program.ProgramCourseSetKind.excluded.name, 'excluded');
    expect(_publicType<assessment.AssessmentClient>(), isNull);
    expect(_publicType<assessment.AssessmentList>(), isNull);
    expect(_publicType<assessment.AssessmentItem>(), isNull);
    expect(_publicType<assessment.AssessmentForm>(), isNull);
    expect(_publicType<assessment.AssessmentQuestion>(), isNull);
    expect(_publicType<assessment.AssessmentPerson>(), isNull);
    expect(_publicType<assessment.AssessmentAnswers>(), isNull);
    expect(_publicType<assessment.AssessmentPersonAnswers>(), isNull);
    expect(_publicType<assessment.AssessmentQuestionAnswer>(), isNull);
    expect(assessment.AssessmentClient.minScore, 1);
    expect(assessment.AssessmentClient.maxScore, 7);
    // A display question is a copy of service values, not a body: the answer
    // types a caller builds carry scores and comments only.
    expect(
      assessment.AssessmentQuestionAnswer(score: 7, comment: '讲得很好'),
      isA<assessment.AssessmentQuestionAnswer>(),
    );
    final answers = assessment.AssessmentAnswers(
      referenceId: 'row-handle',
      score: 7,
      teachers: [
        assessment.AssessmentPersonAnswers(
          questions: [assessment.AssessmentQuestionAnswer(score: 7)],
        ),
      ],
      assistants: const [],
    );
    expect(answers.referenceId, 'row-handle');
    expect(answers.teachers.single.questions.single.comment, isNull);
    expect(_publicType<invoice.InvoiceClient>(), isNull);
    expect(_publicType<invoice.InvoicePage>(), isNull);
    expect(_publicType<invoice.InvoiceRecord>(), isNull);
    expect(_publicType<invoice.InvoiceDocument>(), isNull);
    expect(invoice.InvoiceClient.maxPage, 1000);
    expect(invoice.InvoiceClient.pageSize, 20);
    expect(_publicType<bank.BankClient>(), isNull);
    expect(_publicType<bank.BankPaymentLedger>(), isNull);
    expect(_publicType<bank.BankReceiptMonth>(), isNull);
    expect(_publicType<bank.BankReceipt>(), isNull);
    expect(bank.BankLedger.main.name, 'main');
    expect(bank.BankLedger.foundation.name, 'foundation');
    expect(_publicType<bank.GraduateIncomePage>(), isNull);
    expect(_publicType<bank.GraduateIncomeRecord>(), isNull);
    expect(_publicType<course_score.CourseScoreClient>(), isNull);
    expect(_publicType<course_score.CourseScore>(), isNull);
    expect(
      course_score.CourseScore(name: '课程', credit: 2, grade: 'A', empty: false),
      isA<course_score.CourseScore>(),
    );
    expect(_publicType<laundry.LaundryClient>(), isNull);
    expect(_publicType<laundry.LaundryBuildingGroup>(), isNull);
    expect(_publicType<laundry.LaundryBuilding>(), isNull);
    expect(_publicType<laundry.LaundryRoom>(), isNull);
    expect(_publicType<laundry.LaundryMachine>(), isNull);
    expect(_publicType<laundry.LaundryRoomsReport>(), isNull);
    expect(_publicType<laundry.LaundryProviderOption>(), isNull);
    // The machine states are the closed set the Rust parsers can produce.
    expect(
      laundry.LaundryStatus.values.map((value) => value.name).toList(),
      ['idle', 'working', 'error', 'offline', 'standby', 'unknown'],
    );
    // A building carries the vendor key it belongs to, so a building id can
    // only be read back through the vendor it came from.
    expect(
      const laundry.LaundryBuilding(
        id: '51',
        name: '紫荆1号楼',
        provider: 'jieli',
      ),
      isA<laundry.LaundryBuilding>(),
    );
    // A partial read says what it is missing rather than arriving short.
    final rooms = laundry.LaundryRoomsReport(
      provider: 'haile',
      rooms: const [],
      failedCategories: const ['01'],
      fetchedAt: null,
    );
    expect(rooms.provider, 'haile');
    expect(rooms.failedCategories, ['01']);
    expect(rooms.rooms, isEmpty);
    expect(_publicType<water.WaterClient>(), isNull);
    expect(_publicType<water.WaterUser>(), isNull);
    expect(_publicType<water.WaterBrandOption>(), isNull);
    expect(
      const water.WaterUser(name: '合成住户', address: '紫荆公寓1号楼101'),
      isA<water.WaterUser>(),
    );
    expect(_publicType<sports.SportsClient>(), isNull);
    expect(_publicType<sports.SportsResource>(), isNull);
    expect(_publicType<sports.SportsResources>(), isNull);
    expect(_publicType<sports.SportsReservationRecord>(), isNull);
    // A venue's own cost token is carried verbatim: the service supplies no
    // unit, so no currency conversion may be implied here.
    const slot = sports.SportsResource(
      resId: '3998000',
      resHash: 'HASH-OPAQUE',
      timeSession: '20:00-21:00',
      fieldName: '1号场',
      overlaySize: null,
      canNetBook: true,
      cost: '20',
      bookId: null,
      locked: false,
      userType: null,
      paymentStatus: null,
    );
    expect(slot.cost, '20');
    expect(slot.bookId, isNull);
    // The slot list is unmodifiable, so a caller cannot present a venue's
    // answer as something the service did not send.
    final resources = sports.SportsResources(
      count: 1,
      init: 1,
      phone: null,
      data: const [slot],
    );
    expect(resources.data, hasLength(1));
    expect(
      () => resources.data.add(slot),
      throwsUnsupportedError,
    );
    // A paid row and an unpaid row are both plain reservation records.
    const record = sports.SportsReservationRecord(
      name: '气膜馆羽毛球场',
      field: '1号场',
      time: '2026-09-30 20:00-21:00',
      price: '20',
      method: '已支付',
      bookTimestamp: null,
      bookId: null,
      payId: null,
    );
    expect(record.method, '已支付');
  });
}
