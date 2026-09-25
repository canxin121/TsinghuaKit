//! Local campus-network profiles and observations.
//!
//! This module does not represent a third authentication account. A saved
//! profile is local input for an explicitly requested connection attempt. It
//! distinguishes portal credentials from system-managed 802.1X credentials;
//! the latter can only be applied through a host platform adapter and OS
//! permission.

use std::{fmt, path::PathBuf};

use chrono::{DateTime, Utc};
use zeroize::Zeroizing;

use crate::error::{Error, ErrorCode, Service};
use uuid::Uuid;

/// A stable, opaque identifier for a locally stored network profile.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkProfileId(Uuid);

impl NetworkProfileId {
    /// Creates an identifier for a new profile.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parses a previously returned profile identifier.
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(value).map(Self)
    }

    /// Returns the portable text form of this identifier.
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl Default for NetworkProfileId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NetworkProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NetworkProfileId").field(&self.0).finish()
    }
}

/// The intended method associated with a local network profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NetworkAccessMethod {
    /// The campus portal protocol (srun/TUNet), submitted only after an
    /// explicit connection request.
    Portal,
    /// System-managed Wi-Fi enterprise authentication such as Tsinghua
    /// Secure. A profile using this method must never be submitted to the
    /// campus portal connector; applying it requires a capable host adapter
    /// and operating-system authorization.
    SystemWifiEap,
}

impl NetworkAccessMethod {
    pub(crate) fn storage_name(self) -> &'static str {
        match self {
            Self::Portal => "portal",
            Self::SystemWifiEap => "system_wifi_eap",
        }
    }

    pub(crate) fn from_storage_name(value: &str) -> Option<Self> {
        match value {
            "portal" => Some(Self::Portal),
            "system_wifi_eap" => Some(Self::SystemWifiEap),
            _ => None,
        }
    }
}

/// Controls whether local connection profiles survive the owning Client.
///
/// The default is memory-only. `EncryptedDirectory` is an explicit opt-in to
/// a Rust-managed encrypted file under a host-selected private directory. Its
/// key is protected by local filesystem permissions; this is not equivalent
/// to an operating-system keychain and does not protect against the same OS
/// user, an administrator, or a process with access to the application data.
/// Persistent file mode is currently supported on Unix targets; other targets
/// return `Unsupported` rather than silently weakening permissions.
#[derive(Clone)]
pub enum NetworkProfileStoragePolicy {
    /// Keep profile metadata and optional passwords only in Rust memory.
    MemoryOnly,
    /// Persist profiles in an encrypted file scoped to a host application
    /// namespace. The selected root must be an application-private directory.
    EncryptedDirectory { root: PathBuf, namespace: String },
    /// Persist profiles in an encrypted file using a key supplied by the
    /// host's operating-system credential store. Unlike `EncryptedDirectory`,
    /// the key is never written beside the ciphertext. This remains available
    /// only on Unix targets where the store can enforce private file modes.
    KeychainEncryptedDirectory {
        root: PathBuf,
        namespace: String,
        key: Zeroizing<[u8; 32]>,
    },
}

impl NetworkProfileStoragePolicy {
    /// Constructs an explicit encrypted-file policy for one host application.
    ///
    /// The namespace should be a stable reverse-DNS application identifier,
    /// such as `org.example.campus-app`; it must not contain an account name.
    /// Constructing this value performs no I/O. Storage is opened when the
    /// Client is built.
    pub fn encrypted_directory(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
    ) -> Result<Self, Error> {
        let root = root.into();
        let namespace = namespace.into();
        if !root.is_absolute()
            || root.components().count() < 2
            || namespace.is_empty()
            || namespace.len() > 128
            || namespace == "."
            || namespace == ".."
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        {
            return Err(Error::new(Service::Network, ErrorCode::InvalidInput));
        }
        Ok(Self::EncryptedDirectory { root, namespace })
    }

    /// Constructs encrypted file storage whose key comes from the host's
    /// operating-system credential store. `key` must be a random 32-byte key;
    /// callers should keep it in Keychain/Keystore/Credential Manager and pass
    /// it only while creating the owning Client.
    pub fn keychain_encrypted_directory(
        root: impl Into<PathBuf>,
        namespace: impl Into<String>,
        key: Vec<u8>,
    ) -> Result<Self, Error> {
        let key = Zeroizing::new(key);
        let root = root.into();
        let namespace = namespace.into();
        if !root.is_absolute()
            || root.components().count() < 2
            || namespace.is_empty()
            || namespace.len() > 128
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
            || key.len() != 32
        {
            return Err(Error::new(Service::Network, ErrorCode::InvalidInput));
        }
        let key: [u8; 32] = key
            .as_slice()
            .try_into()
            .map_err(|_| Error::new(Service::Network, ErrorCode::InvalidInput))?;
        Ok(Self::KeychainEncryptedDirectory {
            root,
            namespace,
            key: Zeroizing::new(key),
        })
    }
}

impl Default for NetworkProfileStoragePolicy {
    fn default() -> Self {
        Self::MemoryOnly
    }
}

impl fmt::Debug for NetworkProfileStoragePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MemoryOnly => f.write_str("NetworkProfileStoragePolicy::MemoryOnly"),
            Self::EncryptedDirectory { .. } => f
                .debug_struct("NetworkProfileStoragePolicy::EncryptedDirectory")
                .field("configured", &true)
                .finish(),
            Self::KeychainEncryptedDirectory { .. } => f
                .debug_struct("NetworkProfileStoragePolicy::KeychainEncryptedDirectory")
                .field("configured", &true)
                .finish(),
        }
    }
}

/// User-entered information for one local network profile.
///
/// Password storage is explicit. The value is held in Rust memory and is
/// zeroized when dropped; it is never serialized or included in `Debug`.
pub struct NetworkProfileInput {
    label: String,
    username: String,
    method: NetworkAccessMethod,
    password_to_save: Option<Zeroizing<String>>,
}

impl NetworkProfileInput {
    /// Creates a profile input without saving a password.
    pub fn new(
        label: impl Into<String>,
        username: impl Into<String>,
        method: NetworkAccessMethod,
    ) -> Result<Self, Error> {
        let label = label.into();
        let username = username.into();
        if !valid_profile_text(&label, 80) || !valid_profile_text(&username, 128) {
            return Err(invalid_network_profile_input());
        }
        Ok(Self {
            label: label.trim().to_owned(),
            username: username.trim().to_owned(),
            method,
            password_to_save: None,
        })
    }

    /// Explicitly opts in to keeping this profile's password in the owning
    /// Client's local profile store.
    pub fn save_password(mut self, password: impl Into<String>) -> Result<Self, Error> {
        let password = Zeroizing::new(password.into());
        if password.is_empty() || password.len() > 4096 || password.chars().any(char::is_control) {
            return Err(invalid_network_profile_input());
        }
        self.password_to_save = Some(password);
        Ok(self)
    }

    pub(crate) fn into_parts(
        mut self,
    ) -> (
        String,
        String,
        NetworkAccessMethod,
        Option<Zeroizing<String>>,
    ) {
        (
            std::mem::take(&mut self.label),
            std::mem::take(&mut self.username),
            self.method,
            self.password_to_save.take(),
        )
    }
}

impl fmt::Debug for NetworkProfileInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkProfileInput")
            .field("label_present", &!self.label.is_empty())
            .field("username_present", &!self.username.is_empty())
            .field("method", &self.method)
            .field("password_to_save", &self.password_to_save.is_some())
            .finish()
    }
}

fn valid_profile_text(value: &str, max_bytes: usize) -> bool {
    let value = value.trim();
    !value.is_empty() && value.len() <= max_bytes && value.chars().all(|ch| !ch.is_control())
}

fn invalid_network_profile_input() -> Error {
    Error::new(Service::Network, ErrorCode::InvalidInput)
}

/// A non-secret view of one saved local connection profile.
#[derive(Clone, PartialEq, Eq)]
pub struct NetworkProfileSummary {
    id: NetworkProfileId,
    label: String,
    username: String,
    method: NetworkAccessMethod,
    has_saved_password: bool,
    revision: u64,
}

impl NetworkProfileSummary {
    pub(crate) fn new(
        id: NetworkProfileId,
        label: String,
        username: String,
        method: NetworkAccessMethod,
        has_saved_password: bool,
        revision: u64,
    ) -> Self {
        Self {
            id,
            label,
            username,
            method,
            has_saved_password,
            revision,
        }
    }

    /// Returns the profile handle accepted by local profile operations.
    pub fn id(&self) -> NetworkProfileId {
        self.id
    }

    /// Returns the user-selected display label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the username associated with this local network profile for
    /// explicit display or application-form filling.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Returns the connection mechanism used by this profile.
    pub fn method(&self) -> NetworkAccessMethod {
        self.method
    }

    /// Indicates whether Rust has a password stored for this profile.
    /// The password itself is never returned by this summary.
    pub fn has_saved_password(&self) -> bool {
        self.has_saved_password
    }

    /// Returns the profile version used to reject stale fill references.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Form-ready profile data with no password material.
///
/// The summary can be used to fill an application form. It does not prove
/// network connectivity and is not an Auth session.
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedNetworkInput {
    owner: Uuid,
    profile_id: NetworkProfileId,
    revision: u64,
    summary: NetworkProfileSummary,
}

impl PreparedNetworkInput {
    pub(crate) fn new(
        owner: Uuid,
        profile_id: NetworkProfileId,
        revision: u64,
        summary: NetworkProfileSummary,
    ) -> Self {
        Self {
            owner,
            profile_id,
            revision,
            summary,
        }
    }

    /// Returns the non-secret fields that an application form can display or
    /// fill. The saved password remains inside Rust.
    pub fn summary(&self) -> &NetworkProfileSummary {
        &self.summary
    }

    pub(crate) fn belongs_to(&self, owner: Uuid, profile: &StoredNetworkProfile) -> bool {
        self.owner == owner && self.profile_id == profile.id && self.revision == profile.revision
    }
}

impl fmt::Debug for PreparedNetworkInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedNetworkInput")
            .field("profile_id", &self.profile_id)
            .field("revision", &self.revision)
            .field("summary", &self.summary)
            .finish()
    }
}

/// A saved password explicitly requested for form filling.
///
/// This value is intentionally not cloneable or serializable. It keeps the
/// secret in a zeroizing Rust buffer; callers must explicitly request
/// [`expose_for_form`](Self::expose_for_form) when populating a UI field.
pub struct NetworkProfilePassword {
    value: Zeroizing<String>,
}

impl NetworkProfilePassword {
    pub(crate) fn new(value: String) -> Self {
        Self {
            value: Zeroizing::new(value),
        }
    }

    /// Exposes the secret text for the user's explicitly requested form fill.
    ///
    /// The returned string is borrowed from this zeroizing Rust value. A host
    /// UI may create its own copy when passing the value to a text field.
    pub fn expose_for_form(&self) -> &str {
        self.value.as_str()
    }
}

impl fmt::Debug for NetworkProfilePassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkProfilePassword")
            .field("present", &!self.value.is_empty())
            .finish()
    }
}

/// What the TUNet portal reported for the local IPv4 address queried by Rust.
///
/// This is specifically a portal registration result. It does not prove that
/// the device has general Internet access, and `NotRegistered` does not imply
/// that a system-managed Wi-Fi connection such as Tsinghua Secure is offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PortalAddressRegistration {
    /// The portal positively confirmed this address as online.
    Registered,
    /// The portal positively confirmed this address as not online.
    NotRegistered,
    /// The response did not prove either portal state.
    Unknown,
}

/// A time-bounded observation of the TUNet portal's state for the local IPv4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalObservation {
    registration: PortalAddressRegistration,
    observed_at: DateTime<Utc>,
}

impl PortalObservation {
    pub(crate) fn verified(
        registration: PortalAddressRegistration,
        observed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            registration,
            observed_at,
        }
    }

    /// Returns the portal registration state reported for the local address.
    pub fn registration(&self) -> PortalAddressRegistration {
        self.registration
    }

    /// Returns when the portal response was interpreted, in UTC.
    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }
}

/// A local profile record owned by one engine Client.
#[derive(Clone)]
pub(crate) struct StoredNetworkProfile {
    pub(crate) id: NetworkProfileId,
    pub(crate) label: String,
    pub(crate) username: String,
    pub(crate) method: NetworkAccessMethod,
    pub(crate) password: Option<Zeroizing<String>>,
    pub(crate) revision: u64,
}

impl StoredNetworkProfile {
    pub(crate) fn summary(&self) -> NetworkProfileSummary {
        NetworkProfileSummary::new(
            self.id,
            self.label.clone(),
            self.username.clone(),
            self.method,
            self.password.is_some(),
            self.revision,
        )
    }
}

impl fmt::Debug for StoredNetworkProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredNetworkProfile")
            .field("id", &self.id)
            .field("label_present", &!self.label.is_empty())
            .field("username_present", &!self.username.is_empty())
            .field("method", &self.method)
            .field("password_saved", &self.password.is_some())
            .field("revision", &self.revision)
            .finish()
    }
}

impl fmt::Debug for NetworkProfileSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkProfileSummary")
            .field("id", &self.id)
            .field("label_present", &!self.label.is_empty())
            .field("username_saved", &!self.username.is_empty())
            .field("method", &self.method)
            .field("has_saved_password", &self.has_saved_password)
            .field("revision", &self.revision)
            .finish()
    }
}

/// The scope covered by a current network observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NetworkObservationScope {
    /// The result proves the state of one local physical interface/address.
    LocalInterface,
    /// The result proves only that the current HTTP request origin is reachable.
    RequestOrigin,
}

/// The observed connectivity state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NetworkReachability {
    /// The stated observation scope has current positive evidence.
    Online,
    /// The stated observation scope has current negative evidence.
    Offline,
    /// The available evidence does not prove either state.
    Unknown,
}

/// Whether an observed portal account is related to a selected profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NetworkAccountRelation {
    /// The portal explicitly matched the selected profile's account.
    MatchesProfile,
    /// The portal explicitly reported a different account.
    DifferentAccount,
    /// The evidence did not establish an account relationship.
    Unconfirmed,
    /// This observation did not involve a portal account.
    NotApplicable,
}

/// A timestamped, scope-limited observation of the current network.
///
/// Values are created by Rust after a read-only proof; consumers cannot
/// construct an `Online` observation and submit it as authority for a
/// connection operation.
#[derive(Clone, PartialEq, Eq)]
pub struct NetworkObservation {
    reachability: NetworkReachability,
    scope: NetworkObservationScope,
    access_method: Option<NetworkAccessMethod>,
    observed_at: DateTime<Utc>,
    local_ipv4: Option<String>,
    account_relation: NetworkAccountRelation,
}

impl NetworkObservation {
    pub(crate) fn verified(
        reachability: NetworkReachability,
        scope: NetworkObservationScope,
        access_method: Option<NetworkAccessMethod>,
        observed_at: DateTime<Utc>,
        local_ipv4: Option<String>,
        account_relation: NetworkAccountRelation,
    ) -> Self {
        Self {
            reachability,
            scope,
            access_method,
            observed_at,
            local_ipv4,
            account_relation,
        }
    }

    #[cfg(test)]
    fn verified_for_test(
        reachability: NetworkReachability,
        scope: NetworkObservationScope,
        access_method: Option<NetworkAccessMethod>,
        observed_at: DateTime<Utc>,
        local_ipv4: Option<String>,
        account_relation: NetworkAccountRelation,
    ) -> Self {
        Self::verified(
            reachability,
            scope,
            access_method,
            observed_at,
            local_ipv4,
            account_relation,
        )
    }

    /// Returns the proven connectivity state.
    pub fn reachability(&self) -> NetworkReachability {
        self.reachability
    }

    /// Returns the scope of the evidence.
    pub fn scope(&self) -> NetworkObservationScope {
        self.scope
    }

    /// Returns the connection method only when the current observation proved
    /// it. Reachability by itself cannot identify TUNet, Tsinghua Secure, or a
    /// different network path.
    pub fn access_method(&self) -> Option<NetworkAccessMethod> {
        self.access_method
    }

    /// Returns when the evidence was obtained in UTC.
    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }

    /// Returns the local IPv4 address when a local-interface probe proved it.
    pub fn local_ipv4(&self) -> Option<&str> {
        self.local_ipv4.as_deref()
    }

    /// Returns the portal-account relation established by this observation.
    pub fn account_relation(&self) -> NetworkAccountRelation {
        self.account_relation
    }
}

impl fmt::Debug for NetworkObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkObservation")
            .field("reachability", &self.reachability)
            .field("scope", &self.scope)
            .field("access_method", &self.access_method)
            .field("observed_at", &self.observed_at)
            .field("local_address_observed", &self.local_ipv4.is_some())
            .field("account_relation", &self.account_relation)
            .finish()
    }
}

/// An identifier for one connection attempt whose result is not yet proven.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkOperationId(Uuid);

impl fmt::Debug for NetworkOperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NetworkOperationId(<redacted>)")
    }
}

/// The result of an explicitly requested network connection attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NetworkConnectOutcome {
    /// This operation established and verified the connection.
    Connected(NetworkObservation),
    /// A current observation proved that a connection was already present.
    AlreadyOnline(NetworkObservation),
    /// The one-shot submission may have reached the portal, but its outcome
    /// is unclear. The operation must be observed before another submission.
    OutcomeUnconfirmed { operation: NetworkOperationId },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_refactor_network_profile_storage_rejects_unscoped_namespaces() {
        for namespace in ["", ".", "..", "../other", "account@example.edu"] {
            assert!(
                NetworkProfileStoragePolicy::encrypted_directory("/tmp/tsinghua-kit", namespace)
                    .is_err(),
                "namespace should be rejected"
            );
        }

        assert!(
            NetworkProfileStoragePolicy::encrypted_directory("relative/path", "org.example.app")
                .is_err()
        );
        assert!(NetworkProfileStoragePolicy::encrypted_directory("/", "org.example.app").is_err());
    }

    #[test]
    fn backend_refactor_network_profile_storage_debug_redacts_location() {
        let policy = NetworkProfileStoragePolicy::encrypted_directory(
            "/private/application/data",
            "org.example.campus-app",
        )
        .unwrap();
        let rendered = format!("{policy:?}");

        assert!(rendered.contains("configured: true"));
        assert!(!rendered.contains("/private/application/data"));
        assert!(!rendered.contains("org.example.campus-app"));
    }

    #[test]
    fn backend_refactor_network_profile_input_debug_redacts_saved_password() {
        let input = NetworkProfileInput::new(
            "Private network label",
            "private-network-user",
            NetworkAccessMethod::Portal,
        )
        .unwrap()
        .save_password("private-network-password")
        .unwrap();
        let rendered = format!("{input:?}");

        assert!(rendered.contains("password_to_save: true"));
        assert!(!rendered.contains("Private network label"));
        assert!(!rendered.contains("private-network-user"));
        assert!(!rendered.contains("private-network-password"));
    }

    #[test]
    fn backend_refactor_profile_id_round_trips_without_exposing_a_credential() {
        let id = NetworkProfileId::new();
        let parsed = NetworkProfileId::parse(&id.as_str()).unwrap();
        assert_eq!(id, parsed);

        let profile = NetworkProfileSummary::new(
            id,
            "Dorm Wi-Fi portal".into(),
            "network-user".into(),
            NetworkAccessMethod::Portal,
            true,
            3,
        );
        let debug = format!("{profile:?}");
        assert!(!debug.contains("network-user"));
        assert!(!debug.contains("Dorm Wi-Fi portal"));
        assert!(profile.has_saved_password());

        let eap_profile = NetworkProfileSummary::new(
            NetworkProfileId::new(),
            "Tsinghua Secure".into(),
            "eap-user".into(),
            NetworkAccessMethod::SystemWifiEap,
            true,
            1,
        );
        assert_eq!(eap_profile.method(), NetworkAccessMethod::SystemWifiEap);
        assert!(!format!("{eap_profile:?}").contains("eap-user"));
    }

    #[test]
    fn backend_refactor_network_profile_password_debug_is_redacted() {
        let secret = NetworkProfilePassword::new("private-network-password".into());
        let rendered = format!("{secret:?}");

        assert!(rendered.contains("present: true"));
        assert!(!rendered.contains("private-network-password"));
    }

    #[test]
    fn backend_refactor_request_origin_observation_does_not_claim_local_network_state() {
        let observation = NetworkObservation::verified_for_test(
            NetworkReachability::Online,
            NetworkObservationScope::RequestOrigin,
            None,
            Utc::now(),
            None,
            NetworkAccountRelation::NotApplicable,
        );

        assert_eq!(observation.reachability(), NetworkReachability::Online);
        assert_eq!(observation.scope(), NetworkObservationScope::RequestOrigin);
        assert_eq!(observation.access_method(), None);
        assert_eq!(observation.local_ipv4(), None);
    }

    #[test]
    fn backend_refactor_system_wifi_method_requires_explicit_observation_evidence() {
        let observation = NetworkObservation::verified_for_test(
            NetworkReachability::Online,
            NetworkObservationScope::LocalInterface,
            Some(NetworkAccessMethod::SystemWifiEap),
            Utc::now(),
            None,
            NetworkAccountRelation::NotApplicable,
        );

        assert_eq!(
            observation.access_method(),
            Some(NetworkAccessMethod::SystemWifiEap)
        );
        assert_eq!(
            observation.account_relation(),
            NetworkAccountRelation::NotApplicable
        );
    }
}
