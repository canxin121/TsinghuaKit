/// Flutter-facing API for the TsinghuaKit Rust SDK.
///
/// Rust owns authentication state, service requests, local network profiles,
/// validation, and error classification. This library adapts those operations
/// to Dart without exposing protocol implementation types.
library;

import 'dart:typed_data';

import 'src/rust/frb_generated.dart' show RustLib;
import 'src/second_factor_method.dart'
    show
        SecondFactorMethod,
        secondFactorMethodFromServer,
        secondFactorMethodToBridge;
import 'src/rust/sdk_api.dart' as native;

export 'src/second_factor_method.dart' show SecondFactorMethod;
part 'src/read.dart';
part 'src/library.dart';
part 'src/classrooms.dart';
part 'src/campus_card.dart';
part 'src/electricity.dart';
part 'src/learn.dart';
part 'src/registrar_calendar.dart';
part 'src/news.dart';
part 'src/service_hall.dart';
part 'src/self_service.dart';

/// A structured, redacted failure returned by the Rust SDK.
///
/// `service`, `code`, `retryAfterMs`, and `diagnosticId` are stable fields.
/// No raw URL, response text, or credential is included. The generated FFI
/// error type stays private to this package.
class TsinghuaKitException implements Exception {
  const TsinghuaKitException._({
    required this.service,
    required this.code,
    required this.retryAfterMs,
    required this.diagnosticId,
  });

  final String service;
  final String code;
  final BigInt? retryAfterMs;
  final String diagnosticId;

  @override
  String toString() => 'TsinghuaKitException($service:$code)';
}

Future<T> _sdkCall<T>(Future<T> Function() operation) async {
  try {
    return await operation();
  } on native.SdkErrorDto catch (error) {
    throw TsinghuaKitException._(
      service: error.service,
      code: error.code,
      retryAfterMs: error.retryAfterMs,
      diagnosticId: error.diagnosticId,
    );
  }
}

/// Initializes the bridge and creates Clients backed by the public Rust SDK.
abstract final class TsinghuaKit {
  static Future<void>? _initialization;

  /// Initializes the FFI library once for this process.
  static Future<void> initialize() => _initialization ??= _initialize();

  static Future<void> _initialize() async {
    try {
      await RustLib.init();
    } catch (_) {
      // A transient library-loading problem should not poison every later
      // Client construction for the lifetime of the application process.
      _initialization = null;
      rethrow;
    }
  }

  /// Suggests a login-stage preference from the supported student-id rule.
  /// Returns `null` when the input cannot be classified. The caller's explicit
  /// choice remains authoritative, and no Client or network request is needed.
  static Future<LoginStage?> suggestLoginStage(String username) async {
    await initialize();
    final value = await native.suggestIdentityLoginStage(username: username);
    return value == null ? null : _loginStageFromNative(value);
  }

  /// Creates one single-runtime Client. Construction performs no login or
  /// network request. Network profiles are memory-only by default.
  static Future<TsinghuaKitClient> createClient({
    ClientCachePersistence cache = const ClientCachePersistence.memoryOnly(),
    AuthCredentialPersistence authCredentials =
        const AuthCredentialPersistence.memoryOnly(),
    NetworkProfilePersistence networkProfiles =
        const NetworkProfilePersistence.memoryOnly(),
    IdentitySessionPersistence identitySession =
        const IdentitySessionPersistence.memoryOnly(),
  }) async {
    await initialize();
    final cacheRoot = switch (cache) {
      MemoryOnlyClientCachePersistence() => null,
      DirectoryClientCachePersistence(:final root) => root,
    };
    final (authCredentialRoot, authCredentialNamespace) =
        switch (authCredentials) {
      MemoryOnlyAuthCredentialPersistence() => (null, null),
      EncryptedDirectoryAuthCredentialPersistence(
        :final root,
        :final namespace,
      ) =>
        (root, namespace),
    };
    final (storageRoot, applicationNamespace) = switch (networkProfiles) {
      MemoryOnlyNetworkProfilePersistence() => (null, null),
      EncryptedDirectoryNetworkProfilePersistence(
        :final root,
        :final namespace
      ) =>
        (root, namespace),
    };
    final (identitySessionRoot, identitySessionNamespace) =
        switch (identitySession) {
      MemoryOnlyIdentitySessionPersistence() => (null, null),
      EncryptedDirectoryIdentitySessionPersistence(
        :final root,
        :final namespace,
      ) =>
        (root, namespace),
    };
    final handle = await _sdkCall(
      () => native.ClientHandle.newInstance(
        cacheRoot: cacheRoot,
        credentialStorageRoot: authCredentialRoot,
        credentialStorageNamespace: authCredentialNamespace,
        profileStorageRoot: storageRoot,
        applicationNamespace: applicationNamespace,
        identitySessionRoot: identitySessionRoot,
        identitySessionNamespace: identitySessionNamespace,
      ),
    );
    return TsinghuaKitClient._(handle);
  }
}

/// Persistence choices for non-secret service read caches.
///
/// Service caches remain memory-only by default. They are independent from
/// Identity's optional encrypted session snapshot and local network profiles.
sealed class ClientCachePersistence {
  const ClientCachePersistence();

  /// Keeps read caches only for this Client's lifetime.
  const factory ClientCachePersistence.memoryOnly() =
      MemoryOnlyClientCachePersistence;

  /// Persists read caches beneath this absolute app-private directory.
  const factory ClientCachePersistence.directory({required String root}) =
      DirectoryClientCachePersistence;
}

final class MemoryOnlyClientCachePersistence extends ClientCachePersistence {
  const MemoryOnlyClientCachePersistence();
}

final class DirectoryClientCachePersistence extends ClientCachePersistence {
  const DirectoryClientCachePersistence({required this.root});

  /// Absolute private directory selected by the host application.
  final String root;
}

/// Persistence choices for credentials the user explicitly chooses to
/// remember in either Auth domain.
///
/// This store is separate from Auth session snapshots, service caches, and
/// NetworkProfiles. Saved SelfService credentials only start an explicit
/// captcha flow; they never bypass captcha or create a restored session.
sealed class AuthCredentialPersistence {
  const AuthCredentialPersistence();

  /// Keeps Auth passwords only in memory.
  const factory AuthCredentialPersistence.memoryOnly() =
      MemoryOnlyAuthCredentialPersistence;

  /// Stores explicitly remembered credentials in an encrypted private
  /// directory. The encryption key is stored beside the ciphertext; this is
  /// not equivalent to an operating-system Keychain.
  const factory AuthCredentialPersistence.encryptedDirectory({
    required String root,
    required String namespace,
  }) = EncryptedDirectoryAuthCredentialPersistence;
}

final class MemoryOnlyAuthCredentialPersistence
    extends AuthCredentialPersistence {
  const MemoryOnlyAuthCredentialPersistence();
}

final class EncryptedDirectoryAuthCredentialPersistence
    extends AuthCredentialPersistence {
  const EncryptedDirectoryAuthCredentialPersistence({
    required this.root,
    required this.namespace,
  });

  /// Absolute app-private directory.
  final String root;

  /// Stable application namespace, not an account name.
  final String namespace;
}

/// Persistence choices for the Identity-bound shared session snapshot.
///
/// This is separate from SelfService account state and from local network
/// fill profiles. The default keeps Auth sessions only in memory. Auth
/// password storage is separately configured through AuthCredentialPersistence
/// and requires a per-login opt-in.
sealed class IdentitySessionPersistence {
  const IdentitySessionPersistence();

  /// Keeps Identity cookies only for the owning Client lifetime.
  const factory IdentitySessionPersistence.memoryOnly() =
      MemoryOnlyIdentitySessionPersistence;

  /// Explicitly stores an encrypted, device-bound Identity snapshot under a
  /// host-selected app-private directory. Service caches use their own
  /// ClientCachePersistence policy. This backend is not an OS Keychain; its
  /// encryption key is stored beside the encrypted data. Auth passwords are
  /// stored by the separate AuthCredentialPersistence policy.
  const factory IdentitySessionPersistence.encryptedDirectory({
    required String root,
    required String namespace,
  }) = EncryptedDirectoryIdentitySessionPersistence;
}

/// Default memory-only Identity session storage.
final class MemoryOnlyIdentitySessionPersistence
    extends IdentitySessionPersistence {
  const MemoryOnlyIdentitySessionPersistence();
}

/// Explicit opt-in to the Rust encrypted Identity snapshot backend.
final class EncryptedDirectoryIdentitySessionPersistence
    extends IdentitySessionPersistence {
  const EncryptedDirectoryIdentitySessionPersistence({
    required this.root,
    required this.namespace,
  });

  /// Absolute private application-data directory selected by the host.
  final String root;

  /// Stable application namespace. It is not an account name.
  final String namespace;
}

/// Persistence choices for local network fill profiles.
///
/// These profiles are separate from the Identity and SelfService Auth
/// domains. Enabling persistence never saves or restores an Auth session.
sealed class NetworkProfilePersistence {
  const NetworkProfilePersistence();

  /// Keeps profiles only for the owning Client lifetime.
  const factory NetworkProfilePersistence.memoryOnly() =
      MemoryOnlyNetworkProfilePersistence;

  /// Uses the opt-in Unix encrypted-directory backend for profile fill data.
  ///
  /// The backend is not an OS Keychain. It is unavailable on non-Unix
  /// platforms, and its encryption key is stored alongside the encrypted
  /// profile file in the private directory.
  const factory NetworkProfilePersistence.encryptedDirectory({
    required String root,
    required String namespace,
  }) = EncryptedDirectoryNetworkProfilePersistence;
}

/// Default memory-only local profile storage.
final class MemoryOnlyNetworkProfilePersistence
    extends NetworkProfilePersistence {
  const MemoryOnlyNetworkProfilePersistence();
}

/// Explicit opt-in to the local Unix encrypted-directory profile store.
final class EncryptedDirectoryNetworkProfilePersistence
    extends NetworkProfilePersistence {
  const EncryptedDirectoryNetworkProfilePersistence({
    required this.root,
    required this.namespace,
  });

  /// Absolute private directory selected by the host application.
  final String root;

  /// Stable app namespace that prevents accidental profile-store sharing.
  final String namespace;
}

/// One Rust Client shared by both Auth domains and all local profile methods.
class TsinghuaKitClient {
  TsinghuaKitClient._(native.ClientHandle handle)
      : _handle = handle,
        auth = AuthClient._(handle),
        network = NetworkClient._(handle),
        registrar = RegistrarClient._(handle),
        calendar = CalendarClient._(handle),
        library = LibraryClient._(handle),
        classrooms = ClassroomsClient._(handle),
        campusCard = CampusCardClient._(handle),
        electricity = ElectricityClient._(handle),
        learn = LearnClient._(handle),
        news = NewsClient._(handle),
        serviceHall = ServiceHallClient._(handle),
        selfService = SelfServiceClient._(handle);

  final native.ClientHandle _handle;

  /// Authentication operations for Identity and SelfService.
  final AuthClient auth;

  /// Local campus-network profile operations.
  final NetworkClient network;

  /// Academic schedule, grades, and examination reads.
  final RegistrarClient registrar;

  /// Learn term dates and published school-calendar images.
  final CalendarClient calendar;

  /// Library locations, opening windows, seat availability, and sockets.
  final LibraryClient library;

  /// Classroom building directories and weekly availability.
  final ClassroomsClient classrooms;

  /// Campus-card account and bounded transaction reads.
  final CampusCardClient campusCard;

  /// Dorm-electricity remainder and payment history.
  final ElectricityClient electricity;

  /// Learn courses, announcements, assignments, files, and discussions.
  final LearnClient learn;

  /// Read-only INFO news catalog, pages, details, favorites, and subscriptions.
  final NewsClient news;

  /// Online service-hall workflow reads.
  final ServiceHallClient serviceHall;

  /// Read-only business data for the independent SelfService account.
  /// Login and logout for this account live under [auth].selfService.
  final SelfServiceClient selfService;

  /// Releases this Client's Rust runtime, session state, temporary cache, and
  /// any local profile-store lock. Call when the owning application scope ends.
  ///
  /// Disposing is idempotent. This client and all its domain facades must not
  /// be used afterward.
  void dispose() {
    if (_handle.isDisposed) return;
    _handle.dispose();
  }

  /// Whether [dispose] has already released the native Client handle.
  bool get isDisposed => _handle.isDisposed;
}

/// The independent authentication entry point for the Client.
class AuthClient {
  AuthClient._(this._handle)
      : identity = IdentityAuthClient._(_handle),
        selfService = SelfServiceAuthClient._(_handle);

  final native.ClientHandle _handle;

  /// Unified Identity authentication.
  final IdentityAuthClient identity;

  /// Independent USEREG SelfService authentication.
  final SelfServiceAuthClient selfService;

  /// Returns both account slots. Local network connection status is separate.
  Future<AuthStatus> status() =>
      _sdkCall(() async => _authStatus(await _handle.authStatus()));

  /// Explicitly logs out both Auth domains. Local network profiles and current
  /// operating-system Wi-Fi settings are unaffected.
  Future<AuthStatus> logoutAll() =>
      _sdkCall(() async => _authStatus(await _handle.logoutAll()));
}

/// Unified Identity login and second-factor interaction.
class IdentityAuthClient {
  IdentityAuthClient._(this._handle);

  final native.ClientHandle _handle;

  /// Returns the current login or service second-factor interaction, if any.
  /// This query does not submit a code or start another authentication flow.
  Future<IdentityLoginResult?> interaction() => _sdkCall(() async {
        final value = await _handle.identityInteraction();
        return value == null ? null : _identityResult(value);
      });

  /// Revalidates an Identity snapshot restored from explicitly configured
  /// session storage. This method may perform a bounded read-only request; it
  /// never submits a saved password. With no restored session it is a local
  /// no-op. SelfService state is not promoted by this Identity check.
  Future<AuthStatus> revalidateRestoredSession() => _sdkCall(
        () async => _authStatus(
          await _handle.identityRevalidateRestoredSession(),
        ),
      );

  /// Starts one explicit Identity login attempt.
  ///
  /// [rememberCredentials] defaults to false. When true, the Client must
  /// have [AuthCredentialPersistence] configured; Rust saves the password in
  /// the Identity credential namespace only after authentication succeeds.
  /// Identity session snapshots are a separate policy. Cross-process session
  /// recovery requires both policies, and the password is never written to
  /// the cookie snapshot. SelfService credentials use the other namespace.
  Future<IdentityLoginResult> login({
    required String username,
    required String password,
    LoginStage stage = LoginStage.auto,
    bool trustDevice = false,
    bool rememberCredentials = false,
  }) =>
      _sdkCall(() async {
        final result = await _handle.loginIdentity(
          username: username,
          password: password,
          stage: _loginStage(stage),
          trustDevice: trustDevice,
          rememberCredentials: rememberCredentials,
        );
        return _identityResult(result);
      });

  /// Requests one code for the active Identity challenge.
  Future<IdentityLoginResult> sendCode(SecondFactorMethod method) => _sdkCall(
        () async => _identityResult(
          await _handle.sendIdentityCode(
              method: secondFactorMethodToBridge(method)),
        ),
      );

  /// Submits one user-entered code. Ambiguous submissions are not replayed.
  Future<IdentityLoginResult> submitCode({
    required SecondFactorMethod method,
    required String code,
  }) =>
      _sdkCall(
        () async => _identityResult(
          await _handle.submitIdentityCode(
            method: secondFactorMethodToBridge(method),
            code: code,
          ),
        ),
      );

  /// Logs out Identity and its derived service proofs. A selected SelfService
  /// account remains visible as expired because its shared access route ends.
  Future<AuthStatus> logout() =>
      _sdkCall(() async => _authStatus(await _handle.logoutIdentity()));
}

/// Explicit image-captcha login for the independent SelfService account.
class SelfServiceAuthClient {
  SelfServiceAuthClient._(this._handle);

  final native.ClientHandle _handle;

  /// Returns the in-memory captcha phase without making a request.
  Future<SelfServiceLoginPhase> phase() => _sdkCall(
        () async => _selfServiceLoginPhase(
          await _handle.selfServiceLoginPhase(),
        ),
      );

  /// Begins one login attempt and returns the validated image for display.
  Future<SelfServiceCaptcha> startLogin({
    required String username,
    required String password,
    bool rememberCredentials = false,
  }) =>
      _sdkCall(
        () async => _captcha(
          await _handle.startSelfServiceLogin(
            username: username,
            password: password,
            rememberCredentials: rememberCredentials,
          ),
        ),
      );

  /// Starts the captcha flow using this account's saved password. Rust keeps
  /// the password private and still requires explicit captcha completion.
  Future<SelfServiceCaptcha> startSavedLogin({required String username}) =>
      _sdkCall(() async => _captcha(
            await _handle.startSavedSelfServiceLogin(username: username),
          ));

  /// Deletes this account's saved SelfService password without logging out.
  Future<void> forgetSavedCredentials({required String username}) =>
      _sdkCall(() => _handle.forgetSavedSelfServiceCredentials(
            username: username,
          ));

  /// Explicitly refreshes the active image captcha. The password is not
  /// resubmitted.
  Future<SelfServiceCaptcha> refreshCaptcha() =>
      _sdkCall(() async => _captcha(await _handle.refreshSelfServiceCaptcha()));

  /// Submits the human-entered captcha and optional SMS code once.
  Future<SelfServiceLoginResult> submitCaptcha({
    required String answer,
    String? smsCode,
  }) =>
      _sdkCall(() async {
        final result = await _handle.submitSelfServiceCaptcha(
          answer: answer,
          smsCode: smsCode,
        );
        return SelfServiceLoginResult(
          status: _authStatus(result.status),
          requiresInteraction: result.requiresInteraction,
          credentialsSaved: result.credentialsSaved,
        );
      });

  /// Cancels only the unfinished SelfService challenge.
  Future<void> cancelLogin() =>
      _sdkCall(() => _handle.cancelSelfServiceLogin());

  /// Logs out only SelfService and returns both account states. Identity and
  /// local network profiles remain unchanged.
  Future<AuthStatus> logout() =>
      _sdkCall(() async => _authStatus(await _handle.logoutSelfService()));
}

/// Local campus-network profile operations. These do not create an Auth
/// account. Portal actions are explicit, while system Wi-Fi/EAP stays owned by
/// the operating system.
class NetworkClient {
  NetworkClient._(native.ClientHandle handle)
      : _handle = handle,
        profiles = NetworkProfilesClient._(handle);

  final native.ClientHandle _handle;

  /// Saved local Portal/EAP fill profiles.
  final NetworkProfilesClient profiles;

  /// Reads only whether the TUNet portal registered the current local IPv4.
  /// This does not report general connectivity or Tsinghua Secure status.
  Future<PortalObservation> observePortalStatus() => _sdkCall(() async {
        final value = await _handle.networkPortalObservation();
        return PortalObservation(
          registration: switch (value.registration) {
            native.PortalAddressRegistrationDto.registered =>
              PortalAddressRegistration.registered,
            native.PortalAddressRegistrationDto.notRegistered =>
              PortalAddressRegistration.notRegistered,
            native.PortalAddressRegistrationDto.unknown =>
              PortalAddressRegistration.unknown,
          },
          observedAt: DateTime.parse(value.observedAtUtc).toUtc(),
        );
      });

  /// Explicitly attempts the saved profile's TUNet Portal connection. When
  /// [password] is supplied, Rust uses it for this attempt only and never
  /// saves it. Otherwise Rust uses only a password previously saved with the
  /// profile. A `systemWifiEap` profile is rejected as unsupported. [profile]
  /// must come from this Client's current `profiles.prepareFill()` result, so
  /// edits, deletion, or another Client invalidate the connection reference.
  Future<PortalConnectionResult> connectPortal({
    required PreparedNetworkProfile profile,
    String? password,
  }) =>
      _sdkCall(() async {
        final result = await _handle.networkConnectPortal(
          prepared: profile._handle,
          password: password,
        );
        return PortalConnectionResult(
          state: _portalConnectionState(result.state),
          observedAt: DateTime.parse(result.observedAtUtc).toUtc(),
        );
      });

  /// Explicitly disconnects the TUNet target proven by this same Client.
  /// The capability is process-local and is consumed by the attempt. This
  /// does not disconnect Wi-Fi or a Tsinghua Secure system profile.
  Future<PortalConnectionResult> disconnectPortal() => _sdkCall(() async {
        final result = await _handle.networkDisconnectPortal();
        return PortalConnectionResult(
          state: _portalConnectionState(result.state),
          observedAt: DateTime.parse(result.observedAtUtc).toUtc(),
        );
      });
}

/// CRUD and explicit form preparation for local network profiles.
class NetworkProfilesClient {
  NetworkProfilesClient._(this._handle);

  final native.ClientHandle _handle;

  /// Lists profile labels and methods. Account names and passwords are not
  /// included in this ordinary list result.
  Future<List<NetworkProfile>> list() => _sdkCall(
        () async => (await _handle.networkProfiles())
            .map(_profile)
            .toList(growable: false),
      );

  /// Saves one local profile. Password persistence occurs only when explicitly
  /// supplied. No network request or connection attempt is made.
  Future<NetworkProfile> save({
    required String label,
    required String username,
    required NetworkAccessMethod method,
    String? passwordToSave,
  }) =>
      _sdkCall(
        () async => _profile(
          await _handle.saveNetworkProfile(
            label: label,
            username: username,
            method: _networkMethod(method),
            password: passwordToSave,
          ),
        ),
      );

  /// Replaces the selected profile values and invalidates earlier fill handles.
  Future<NetworkProfile> update({
    required String id,
    required String label,
    required String username,
    required NetworkAccessMethod method,
    String? passwordToSave,
  }) =>
      _sdkCall(
        () async => _profile(
          await _handle.updateNetworkProfile(
            id: id,
            label: label,
            username: username,
            method: _networkMethod(method),
            password: passwordToSave,
          ),
        ),
      );

  /// Removes a local profile without changing Auth or current network state.
  Future<bool> delete(String id) =>
      _sdkCall(() => _handle.deleteNetworkProfile(id: id));

  /// Prepares non-secret account fields for one explicit application-form fill.
  Future<PreparedNetworkProfile> prepareFill(String id) => _sdkCall(
        () async => PreparedNetworkProfile._(
          _handle,
          await _handle.prepareNetworkProfileFill(id: id),
        ),
      );
}

/// Non-secret profile data returned by ordinary list/save/update operations.
class NetworkProfile {
  const NetworkProfile({
    required this.id,
    required this.label,
    required this.method,
    required this.hasSavedPassword,
  });

  final String id;
  final String label;
  final NetworkAccessMethod method;
  final bool hasSavedPassword;
}

/// A Client- and profile-version-bound prepared form.
class PreparedNetworkProfile {
  PreparedNetworkProfile._(this._client, this._handle);

  final native.ClientHandle _client;
  final native.PreparedNetworkProfile _handle;

  /// Returns the account name only after explicit form preparation.
  Future<NetworkProfileFill> formFields() =>
      _sdkCall(() async => _fill(await _handle.formFields()));

  /// Requests the saved password from Rust for this current Client/profile
  /// revision. The returned handle keeps the Rust-owned secret zeroizing until
  /// the UI explicitly exposes it to a text field.
  Future<NetworkProfilePassword?> passwordForExplicitFill() =>
      _sdkCall(() async {
        final value = await _client.networkProfilePasswordForFill(
          prepared: _handle,
        );
        return value == null ? null : NetworkProfilePassword._(value);
      });

  /// Checks whether the profile has changed since this form was prepared.
  Future<bool> isCurrent() =>
      _sdkCall(() => _client.isCurrentNetworkProfileFill(prepared: _handle));
}

/// Form fields returned only after the user explicitly prepares a profile.
class NetworkProfileFill {
  const NetworkProfileFill({
    required this.id,
    required this.label,
    required this.username,
    required this.method,
    required this.hasSavedPassword,
  });

  final String id;
  final String label;
  final String username;
  final NetworkAccessMethod method;
  final bool hasSavedPassword;
}

/// Rust-owned password handle for a user-requested fill operation.
class NetworkProfilePassword {
  NetworkProfilePassword._(this._handle);

  final native.NetworkProfilePasswordHandle _handle;

  /// Copies the password into a UI field. Callers should clear that field when
  /// the form closes or the selected profile changes, and never log or persist
  /// the returned string.
  Future<String> exposeForExplicitFormFill() =>
      _sdkCall(() => _handle.exposeForForm());
}

/// Identity's academic-stage choice for a single login.
enum LoginStage { auto, undergraduate, graduate }

/// A finite state for each independent Auth domain.
enum AccountState {
  signedOut,
  restoredUnverified,
  authenticated,
  needsInteraction,
  authenticating,
  expired,
  unknown,
}

/// Redacted state for one Auth account domain.
class AccountStatus {
  const AccountStatus(this.state, {this.username});

  final AccountState state;

  /// The account name associated with this Auth domain, when one is known.
  /// It is presentation data only and never proves that the session is valid.
  final String? username;
}

/// The separate unified Identity and SelfService account states.
class AuthStatus {
  const AuthStatus({required this.identity, required this.selfService});
  final AccountStatus identity;
  final AccountStatus selfService;
}

/// Result of an Identity login or one explicit second-factor step.
class IdentityLoginResult {
  const IdentityLoginResult({
    required this.status,
    required this.requiresInteraction,
    required this.methods,
    this.maskedPhone,
  });

  final AuthStatus status;
  final bool requiresInteraction;
  final List<SecondFactorMethod> methods;
  final String? maskedPhone;
}

/// Validated image challenge from the SelfService service.
class SelfServiceCaptcha {
  const SelfServiceCaptcha({required this.contentType, required this.bytes});
  final String contentType;
  final Uint8List bytes;
}

/// Result of one SelfService captcha submission.
class SelfServiceLoginResult {
  const SelfServiceLoginResult({
    required this.status,
    required this.requiresInteraction,
    required this.credentialsSaved,
  });
  final AuthStatus status;
  final bool requiresInteraction;
  final bool credentialsSaved;
}

/// The current phase of one explicit SelfService captcha interaction.
enum SelfServiceLoginPhase {
  captchaReady,
  refreshRequired,
  restartRequired,
  unknown,
}

/// A local campus-network profile purpose. `Portal` credentials must never be
/// sent to the system Wi-Fi adapter, and `SystemWifiEap` credentials must never
/// be submitted to the TUNet portal.
enum NetworkAccessMethod { portal, systemWifiEap, unknown }

/// Narrow evidence about the current TUNet portal registration query.
enum PortalAddressRegistration { registered, notRegistered, unknown }

/// State positively confirmed by an explicit TUNet Portal operation.
enum PortalConnectionState { connected, disconnected, unknown }

class PortalObservation {
  const PortalObservation(
      {required this.registration, required this.observedAt});

  final PortalAddressRegistration registration;

  /// UTC time when Rust interpreted the portal response.
  final DateTime observedAt;
}

class PortalConnectionResult {
  const PortalConnectionResult({required this.state, required this.observedAt});

  final PortalConnectionState state;

  /// UTC time when Rust interpreted the verified operation result.
  final DateTime observedAt;
}

AccountState _accountState(native.AccountStateDto value) => switch (value) {
      native.AccountStateDto.signedOut => AccountState.signedOut,
      native.AccountStateDto.restoredUnverified =>
        AccountState.restoredUnverified,
      native.AccountStateDto.authenticated => AccountState.authenticated,
      native.AccountStateDto.needsInteraction => AccountState.needsInteraction,
      native.AccountStateDto.authenticating => AccountState.authenticating,
      native.AccountStateDto.expired => AccountState.expired,
      native.AccountStateDto.unknown => AccountState.unknown,
    };

AuthStatus _authStatus(native.AuthStatusDto value) => AuthStatus(
      identity: AccountStatus(
        _accountState(value.identity.state),
        username: value.identity.username,
      ),
      selfService: AccountStatus(
        _accountState(value.selfService.state),
        username: value.selfService.username,
      ),
    );

SelfServiceLoginPhase _selfServiceLoginPhase(
  native.SelfServiceLoginPhaseDto value,
) =>
    switch (value) {
      native.SelfServiceLoginPhaseDto.captchaReady =>
        SelfServiceLoginPhase.captchaReady,
      native.SelfServiceLoginPhaseDto.refreshRequired =>
        SelfServiceLoginPhase.refreshRequired,
      native.SelfServiceLoginPhaseDto.restartRequired =>
        SelfServiceLoginPhase.restartRequired,
      native.SelfServiceLoginPhaseDto.unknown => SelfServiceLoginPhase.unknown,
    };

IdentityLoginResult _identityResult(native.IdentityLoginResultDto value) =>
    IdentityLoginResult(
      status: _authStatus(value.status),
      requiresInteraction: value.requiresInteraction,
      methods: value.methods
          .map(secondFactorMethodFromServer)
          .toList(growable: false),
      maskedPhone: value.maskedPhone,
    );

native.LoginStageDto _loginStage(LoginStage value) => switch (value) {
      LoginStage.auto => native.LoginStageDto.auto,
      LoginStage.undergraduate => native.LoginStageDto.undergraduate,
      LoginStage.graduate => native.LoginStageDto.graduate,
    };

LoginStage _loginStageFromNative(native.LoginStageDto value) => switch (value) {
      native.LoginStageDto.auto => LoginStage.auto,
      native.LoginStageDto.undergraduate => LoginStage.undergraduate,
      native.LoginStageDto.graduate => LoginStage.graduate,
    };

native.NetworkAccessMethodDto _networkMethod(NetworkAccessMethod value) =>
    switch (value) {
      NetworkAccessMethod.portal => native.NetworkAccessMethodDto.portal,
      NetworkAccessMethod.systemWifiEap =>
        native.NetworkAccessMethodDto.systemWifiEap,
      NetworkAccessMethod.unknown => native.NetworkAccessMethodDto.unknown,
    };

NetworkAccessMethod _networkMethodFromNative(
  native.NetworkAccessMethodDto value,
) =>
    switch (value) {
      native.NetworkAccessMethodDto.portal => NetworkAccessMethod.portal,
      native.NetworkAccessMethodDto.systemWifiEap =>
        NetworkAccessMethod.systemWifiEap,
      native.NetworkAccessMethodDto.unknown => NetworkAccessMethod.unknown,
    };

PortalConnectionState _portalConnectionState(
  native.PortalConnectionStateDto value,
) =>
    switch (value) {
      native.PortalConnectionStateDto.connected =>
        PortalConnectionState.connected,
      native.PortalConnectionStateDto.disconnected =>
        PortalConnectionState.disconnected,
      native.PortalConnectionStateDto.unknown => PortalConnectionState.unknown,
    };

NetworkProfile _profile(native.NetworkProfileDto value) => NetworkProfile(
      id: value.id,
      label: value.label,
      method: _networkMethodFromNative(value.method),
      hasSavedPassword: value.hasSavedPassword,
    );

NetworkProfileFill _fill(native.NetworkProfileFillDto value) =>
    NetworkProfileFill(
      id: value.id,
      label: value.label,
      username: value.username,
      method: _networkMethodFromNative(value.method),
      hasSavedPassword: value.hasSavedPassword,
    );

SelfServiceCaptcha _captcha(native.SelfServiceCaptchaDto value) =>
    SelfServiceCaptcha(contentType: value.contentType, bytes: value.bytes);
