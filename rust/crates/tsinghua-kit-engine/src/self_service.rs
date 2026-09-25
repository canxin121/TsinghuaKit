//! Typed, read-only data belonging to the independent USEREG account.
//!
//! Authentication for this service is managed through
//! [`crate::auth::AuthDomain::SelfService`]. These values never imply a
//! TUNet portal session or a system Wi-Fi/EAP connection.

use std::fmt;

use uuid::Uuid;

use crate::api::runtime::{UseregAccountDto, UseregBalanceDto, UseregDeviceDto};

/// Account details returned by the authenticated SelfService account page.
///
/// Contact data and the real name are intentionally omitted from `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountProfile {
    username: String,
    contact_email: String,
    contact_phone: String,
    contact_landline: String,
    real_name: String,
    status: String,
    user_group: String,
    location: String,
    allowed_devices: u32,
}

impl AccountProfile {
    pub(crate) fn from_runtime(value: UseregAccountDto) -> Self {
        Self {
            username: value.username,
            contact_email: value.contact_email,
            contact_phone: value.contact_phone,
            contact_landline: value.contact_landline,
            real_name: value.real_name,
            status: value.status,
            user_group: value.user_group,
            location: value.location,
            allowed_devices: value.allowed_devices,
        }
    }

    /// Returns the username proved by the SelfService account read.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Returns the server-masked contact email, if one was supplied.
    pub fn contact_email(&self) -> &str {
        &self.contact_email
    }

    /// Returns the server-masked contact phone, if one was supplied.
    pub fn contact_phone(&self) -> &str {
        &self.contact_phone
    }

    /// Returns the server-masked landline, if one was supplied.
    pub fn contact_landline(&self) -> &str {
        &self.contact_landline
    }

    /// Returns the real name displayed by the authenticated service.
    pub fn real_name(&self) -> &str {
        &self.real_name
    }

    /// Returns the account status as displayed by the service.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Returns the account group as displayed by the service.
    pub fn user_group(&self) -> &str {
        &self.user_group
    }

    /// Returns the account location as displayed by the service.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Returns the service-reported device limit.
    pub fn allowed_devices(&self) -> u32 {
        self.allowed_devices
    }
}

impl fmt::Debug for AccountProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountProfile")
            .field("account_present", &!self.username.is_empty())
            .field("contact_email_present", &!self.contact_email.is_empty())
            .field("contact_phone_present", &!self.contact_phone.is_empty())
            .field(
                "contact_landline_present",
                &!self.contact_landline.is_empty(),
            )
            .field("real_name_present", &!self.real_name.is_empty())
            .field("status_present", &!self.status.is_empty())
            .field("user_group_present", &!self.user_group.is_empty())
            .field("location_present", &!self.location.is_empty())
            .field("allowed_devices", &self.allowed_devices)
            .finish()
    }
}

/// A currently online device reported by the SelfService account.
///
/// Protocol identifiers are not included. Addresses and login time are
/// exposed only through explicit getters and are omitted from `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct OnlineDevice {
    reference: DeviceRef,
    ipv4: String,
    ipv6: String,
    logged_at: String,
    authorization: String,
    mac_suffix: String,
}

impl OnlineDevice {
    pub(crate) fn from_runtime(value: UseregDeviceDto, reference: DeviceRef) -> Self {
        Self {
            reference,
            ipv4: value.ip4,
            ipv6: value.ip6,
            logged_at: value.logged_at,
            authorization: value.auth_permission,
            mac_suffix: value.mac_suffix,
        }
    }

    /// Returns the opaque handle for an explicit disconnect request.
    ///
    /// The handle is valid only for the client and device-list snapshot that
    /// produced this row. Reading the list again or changing the SelfService
    /// session makes the old handle stale.
    pub fn device_ref(&self) -> &DeviceRef {
        &self.reference
    }

    /// Returns the reported IPv4 address.
    pub fn ipv4(&self) -> &str {
        &self.ipv4
    }

    /// Returns the reported IPv6 address, if available.
    pub fn ipv6(&self) -> &str {
        &self.ipv6
    }

    /// Returns the login time displayed by the service.
    pub fn logged_at(&self) -> &str {
        &self.logged_at
    }

    /// Returns the authorization description displayed by the service.
    pub fn authorization(&self) -> &str {
        &self.authorization
    }

    /// Returns the masked hardware address suffix supplied by the service.
    pub fn mac_suffix(&self) -> &str {
        &self.mac_suffix
    }
}

impl fmt::Debug for OnlineDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OnlineDevice")
            .field("device_ref", &self.reference)
            .field("ipv4_present", &!self.ipv4.is_empty())
            .field("ipv6_present", &!self.ipv6.is_empty())
            .field("logged_at_present", &!self.logged_at.is_empty())
            .field("authorization_present", &!self.authorization.is_empty())
            .field("mac_suffix_present", &!self.mac_suffix.is_empty())
            .finish()
    }
}

/// An opaque, snapshot-bound reference to one SelfService device.
///
/// Callers can retain and pass this value to `disconnect_device`, but cannot
/// construct a target from an arbitrary row index, portal identifier, or MAC
/// address. It is valid only for its originating Client and current device
/// list.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceRef {
    client_id: Uuid,
    list_generation: u64,
    index: u32,
}

impl DeviceRef {
    pub(crate) fn new(client_id: Uuid, list_generation: u64, index: u32) -> Self {
        Self {
            client_id,
            list_generation,
            index,
        }
    }

    pub(crate) fn belongs_to(&self, client_id: Uuid) -> bool {
        self.client_id == client_id
    }

    pub(crate) fn selector(&self) -> (u64, u32) {
        (self.list_generation, self.index)
    }
}

impl fmt::Debug for DeviceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceRef")
            .field("opaque", &true)
            .finish()
    }
}

/// Usage and balance values from the SelfService home page.
///
/// Values remain service-formatted strings because the portal can change
/// units and presentation without changing the HTML field shape.
#[derive(Clone, PartialEq, Eq)]
pub struct UsageBalance {
    product_name: String,
    used_bytes: String,
    used_seconds: String,
    account_balance: String,
    settlement_date: String,
}

impl UsageBalance {
    pub(crate) fn from_runtime(value: UseregBalanceDto) -> Self {
        Self {
            product_name: value.product_name,
            used_bytes: value.used_bytes,
            used_seconds: value.used_seconds,
            account_balance: value.account_balance,
            settlement_date: value.settlement_date,
        }
    }

    /// Returns the service product name.
    pub fn product_name(&self) -> &str {
        &self.product_name
    }

    /// Returns the service-formatted data-usage value.
    pub fn used_bytes(&self) -> &str {
        &self.used_bytes
    }

    /// Returns the service-formatted duration value.
    pub fn used_seconds(&self) -> &str {
        &self.used_seconds
    }

    /// Returns the service-formatted remaining balance.
    pub fn account_balance(&self) -> &str {
        &self.account_balance
    }

    /// Returns the settlement date displayed by the service.
    pub fn settlement_date(&self) -> &str {
        &self.settlement_date
    }
}

impl fmt::Debug for UsageBalance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UsageBalance")
            .field("product_name_present", &!self.product_name.is_empty())
            .field(
                "usage_present",
                &(!self.used_bytes.is_empty() || !self.used_seconds.is_empty()),
            )
            .field("balance_present", &!self.account_balance.is_empty())
            .field("settlement_date_present", &!self.settlement_date.is_empty())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_device_debug_output_omits_personal_fields() {
        let account = AccountProfile::from_runtime(UseregAccountDto {
            username: "account-example".into(),
            contact_email: "masked-email-example".into(),
            contact_phone: "masked-phone-example".into(),
            contact_landline: "masked-landline-example".into(),
            real_name: "person-example".into(),
            status: "active-example".into(),
            user_group: "group-example".into(),
            location: "location-example".into(),
            allowed_devices: 2,
        });
        let device = OnlineDevice::from_runtime(
            UseregDeviceDto {
                index: 0,
                ip4: "192.0.2.18".into(),
                ip6: "2001:db8::18".into(),
                logged_at: "time-example".into(),
                auth_permission: "permission-example".into(),
                mac_suffix: "mac-suffix-example".into(),
            },
            DeviceRef::new(Uuid::new_v4(), 1, 0),
        );
        let debug = format!("{account:?} {device:?}");

        for private_value in [
            "account-example",
            "masked-email-example",
            "masked-phone-example",
            "masked-landline-example",
            "person-example",
            "location-example",
            "192.0.2.18",
            "2001:db8::18",
            "mac-suffix-example",
        ] {
            assert!(!debug.contains(private_value));
        }
        assert!(debug.contains("DeviceRef { opaque: true }"));
    }

    #[test]
    fn device_reference_debug_does_not_reveal_its_owner_or_selector() {
        let owner = Uuid::parse_str("12345678-1234-5678-9abc-def012345678").unwrap();
        let reference = DeviceRef::new(owner, 9_876_543_210, u32::MAX);
        let debug = format!("{reference:?}");

        assert_eq!(debug, "DeviceRef { opaque: true }");
        assert!(!debug.contains(&owner.to_string()));
        assert!(!debug.contains("9876543210"));
        assert!(!debug.contains("4294967295"));
    }
}
