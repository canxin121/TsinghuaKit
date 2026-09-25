//! Public account and authentication state types.
//!
//! TsinghuaKit manages two independent account domains. Unified Identity and
//! SelfService may use different usernames and credentials. Local campus
//! network profiles are deliberately modeled by [`crate::network`], not as a
//! third authentication domain.

use std::fmt;

/// An account domain managed by TsinghuaKit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AuthDomain {
    /// Tsinghua unified identity authentication.
    Identity,
    /// The independently named USEREG network self-service account.
    SelfService,
}

/// The observed state of one account in the current client.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccountAuthState {
    /// No account is selected for this domain.
    SignedOut,
    /// Local session material was restored and still needs online proof.
    RestoredUnverified,
    /// The account has a current Rust-verified service proof.
    Authenticated,
    /// An explicit user interaction is required to continue.
    NeedsInteraction,
    /// A login or account switch is currently being processed.
    Authenticating,
    /// A formerly selected account is known, but its current session is not
    /// usable. This state is not proof that the saved password is invalid.
    Expired,
}

/// A supported explicit second-factor interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SecondFactorMethod {
    /// A one-time code sent by text message.
    Sms,
    /// A one-time code sent through WeChat.
    Wechat,
    /// A one-time code sent to the registered mobile app.
    Mobile,
    /// A time-based one-time password.
    Totp,
}

/// Current non-secret phase of one explicit SelfService captcha login.
///
/// `RestartRequired` means that no usable challenge remains and the caller
/// must start a new explicit login. It never authorizes replaying a password
/// or captcha answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SelfServiceLoginPhase {
    /// The current captcha can be displayed and submitted by the user.
    CaptchaReady,
    /// The current captcha is no longer usable; the user may request a new one.
    RefreshRequired,
    /// The challenge is absent, expired, or no longer bound to Identity.
    RestartRequired,
}

impl SecondFactorMethod {
    /// Returns the stable identifier used by TsinghuaKit's typed request API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sms => "sms",
            Self::Wechat => "wechat",
            Self::Mobile => "mobile",
            Self::Totp => "totp",
        }
    }

    pub(crate) fn from_backend(value: &str) -> Option<Self> {
        match value {
            "sms" => Some(Self::Sms),
            "wechat" => Some(Self::Wechat),
            "mobile" => Some(Self::Mobile),
            "totp" => Some(Self::Totp),
            _ => None,
        }
    }
}

impl fmt::Debug for AccountAuthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SignedOut => "SignedOut",
            Self::RestoredUnverified => "RestoredUnverified",
            Self::Authenticated => "Authenticated",
            Self::NeedsInteraction => "NeedsInteraction",
            Self::Authenticating => "Authenticating",
            Self::Expired => "Expired",
        })
    }
}

/// Read-only status for one account domain.
///
/// The username is available for explicit UI presentation, but is omitted
/// from `Debug` output. This type cannot be submitted back to TsinghuaKit to
/// create or restore an authenticated session.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountAuthStatus {
    domain: AuthDomain,
    username: Option<String>,
    state: AccountAuthState,
}

impl AccountAuthStatus {
    fn construct(domain: AuthDomain, username: Option<String>, state: AccountAuthState) -> Self {
        Self {
            domain,
            username,
            state,
        }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn new(domain: AuthDomain, username: Option<String>, state: AccountAuthState) -> Self {
        Self::construct(domain, username, state)
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn new(
        domain: AuthDomain,
        username: Option<String>,
        state: AccountAuthState,
    ) -> Self {
        Self::construct(domain, username, state)
    }

    /// Returns which independent account domain this status describes.
    pub fn domain(&self) -> AuthDomain {
        self.domain
    }

    /// Returns the selected account name, if one is known.
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    /// Returns the current verified or pending state.
    pub fn state(&self) -> AccountAuthState {
        self.state
    }
}

impl fmt::Debug for AccountAuthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountAuthStatus")
            .field("domain", &self.domain)
            .field("account_selected", &self.username.is_some())
            .field("state", &self.state)
            .finish()
    }
}

/// The independent Identity and SelfService states owned by one client.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthStatus {
    identity: AccountAuthStatus,
    self_service: AccountAuthStatus,
}

impl AuthStatus {
    fn construct(identity: AccountAuthStatus, self_service: AccountAuthStatus) -> Self {
        assert_eq!(identity.domain(), AuthDomain::Identity);
        assert_eq!(self_service.domain(), AuthDomain::SelfService);
        Self {
            identity,
            self_service,
        }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn new(identity: AccountAuthStatus, self_service: AccountAuthStatus) -> Self {
        Self::construct(identity, self_service)
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn new(identity: AccountAuthStatus, self_service: AccountAuthStatus) -> Self {
        Self::construct(identity, self_service)
    }

    /// Returns the unified Identity account state.
    pub fn identity(&self) -> &AccountAuthStatus {
        &self.identity
    }

    /// Returns the independent network self-service account state.
    pub fn self_service(&self) -> &AccountAuthStatus {
        &self.self_service
    }
}

impl fmt::Debug for AuthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthStatus")
            .field("identity", &self.identity)
            .field("self_service", &self.self_service)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_refactor_identical_usernames_remain_in_separate_account_domains() {
        let status = AuthStatus::new(
            AccountAuthStatus::new(
                AuthDomain::Identity,
                Some("same-name".into()),
                AccountAuthState::Authenticated,
            ),
            AccountAuthStatus::new(
                AuthDomain::SelfService,
                Some("same-name".into()),
                AccountAuthState::SignedOut,
            ),
        );

        assert_eq!(status.identity().domain(), AuthDomain::Identity);
        assert_eq!(status.self_service().domain(), AuthDomain::SelfService);
        assert_eq!(
            status.identity().username(),
            status.self_service().username()
        );
        assert_eq!(status.identity().state(), AccountAuthState::Authenticated);
        assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);
    }

    #[test]
    fn backend_refactor_debug_output_does_not_include_account_names() {
        let status = AccountAuthStatus::new(
            AuthDomain::Identity,
            Some("private-account-name".into()),
            AccountAuthState::Authenticated,
        );

        assert!(!format!("{status:?}").contains("private-account-name"));
    }
}
