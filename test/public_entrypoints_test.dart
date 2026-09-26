import 'package:flutter_test/flutter_test.dart';
import 'package:tsinghua_kit/auth.dart' as auth;
import 'package:tsinghua_kit/campus_card.dart' as campus_card;
import 'package:tsinghua_kit/classrooms.dart' as classrooms;
import 'package:tsinghua_kit/core.dart' as core;
import 'package:tsinghua_kit/electricity.dart' as electricity;
import 'package:tsinghua_kit/learn.dart' as learn;
import 'package:tsinghua_kit/library.dart' as library_api;
import 'package:tsinghua_kit/network.dart' as network;
import 'package:tsinghua_kit/news.dart' as news;
import 'package:tsinghua_kit/read.dart' as read;
import 'package:tsinghua_kit/registrar_calendar.dart' as calendar;
import 'package:tsinghua_kit/self_service.dart' as self_service;
import 'package:tsinghua_kit/service_hall.dart' as service_hall;

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
      auth.AuthCredentialPersistence.memoryOnly(),
      isA<auth.MemoryOnlyAuthCredentialPersistence>(),
    );
    expect(
      auth.AuthCredentialPersistence.encryptedDirectory(
        root: '/app/private',
        namespace: 'org.example.app',
      ),
      isA<auth.EncryptedDirectoryAuthCredentialPersistence>(),
    );
    expect(auth.SecondFactorMethod.sms.name, 'sms');
    expect(network.NetworkAccessMethod.systemWifiEap.name, 'systemWifiEap');
    expect(network.PortalConnectionState.connected.name, 'connected');
    expect(_publicType<network.PortalConnectionResult>(), isNull);
    expect(
      _publicType<network.PlatformSecureStorageNetworkProfilePersistence>(),
      isNull,
    );
    expect(
      _publicType<auth.PlatformSecureStorageIdentitySessionPersistence>(),
      isNull,
    );
    expect(service_hall.ServiceHallTaskView.phases.name, 'phases');
    expect(news.NewsCatalogCoverage.complete.name, 'complete');
    expect(learn.LearnHomeworkState.pending.name, 'pending');
    expect(calendar.AcademicStage.undergraduate.name, 'undergraduate');
    expect(library_api.LibraryDay.today.name, 'today');
    expect(classrooms.ClassroomSlotStatus.unknown.name, 'unknown');
    expect(campus_card.CampusCardTransactionType.any.name, 'any');
    expect(_publicType<self_service.SelfServiceClient>(), isNull);
    expect(_publicType<auth.SelfServiceAuthClient>(), isNull);
    expect(_publicType<electricity.ElectricityClient>(), isNull);
    expect(_publicType<read.ReadResult<int>>(), isNull);
  });
}
