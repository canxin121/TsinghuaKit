/// Authentication APIs for the independent Identity and SelfService accounts.
///
/// Create the owning client with `TsinghuaKit.createClient()` from `core.dart`;
/// both account domains then share that one client.
library;

export 'tsinghua_kit.dart'
    show
        AccountState,
        AccountStatus,
        AuthCredentialPersistence,
        AuthClient,
        AuthStatus,
        EncryptedDirectoryIdentitySessionPersistence,
        EncryptedDirectoryAuthCredentialPersistence,
        IdentityAuthClient,
        IdentityLoginResult,
        IdentitySessionPersistence,
        LoginStage,
        MemoryOnlyIdentitySessionPersistence,
        MemoryOnlyAuthCredentialPersistence,
        PlatformSecureStorageAuthCredentialPersistence,
        PlatformSecureStorageIdentitySessionPersistence,
        SecondFactorMethod,
        SelfServiceAuthClient,
        SelfServiceCaptcha,
        SelfServiceLoginPhase,
        SelfServiceLoginResult,
        TsinghuaKitClient;
