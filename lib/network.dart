/// Local campus-network profiles, TUNet registration evidence, and explicit
/// TUNet Portal operations.
///
/// Network profiles are local inputs, not Auth accounts. System Wi-Fi/EAP
/// connection and configuration remain owned by the operating system.
library;

export 'tsinghua_kit.dart'
    show
        EncryptedDirectoryNetworkProfilePersistence,
        MemoryOnlyNetworkProfilePersistence,
        NetworkAccessMethod,
        NetworkClient,
        NetworkProfile,
        NetworkProfileFill,
        NetworkProfilePassword,
        NetworkProfilePersistence,
        NetworkProfilesClient,
        PortalAddressRegistration,
        PortalConnectionResult,
        PortalConnectionState,
        PortalObservation,
        PreparedNetworkProfile,
        TsinghuaKitClient;
