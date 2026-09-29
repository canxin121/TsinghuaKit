//! Account-bound laundry and water reads.
//!
//! Both sub-domains are third-party services with no campus account binding, so
//! this module owns exactly one decision for each: which vendor origin the
//! read will talk to.  The adapters themselves live in
//! [`crate::washer_read`] and [`crate::water_read`], and each keeps its own
//! Cookie-free transport on the shared request gate.
//!
//! Two rules shape this file:
//!
//! * **A read is account-independent, so it is not gated on a campus login.**
//!   Neither vendor knows the campus account, and neither request carries a
//!   campus credential.  Requiring a proven INFO session here would be a
//!   binding that does not exist, so the reads run without one and are reported
//!   as third-party reads rather than as reads of the user's own account.
//! * **Nothing is cached and nothing is persisted.**  Device state changes
//!   minute by minute and the vendor's own snapshot time is reported verbatim,
//!   so a stored copy would be wrong as soon as it was written.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use serde::{Deserialize, Serialize};

use crate::washer_read::{
    Washer, WasherAdapter, WasherBuilding, WasherBuildingGroup, WasherError, WasherProvider,
    WasherRoom, WasherRoomRead, WasherStatus,
};
use crate::water_read::{WaterAdapter, WaterError, WaterUser};

/// One laundry building as the bridge and SDK report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaundryBuilding {
    pub id: String,
    pub name: String,
    /// The vendor's stable machine key (`jieli` / `haile` / `xiaolan`).
    pub provider: String,
}

/// One group of laundry buildings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaundryBuildingGroup {
    pub key: String,
    pub label: String,
    pub buildings: Vec<LaundryBuilding>,
}

/// One washing machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaundryMachine {
    pub name: String,
    pub kind: String,
    pub room: String,
    pub status: String,
    pub eta_minutes: Option<u32>,
}

/// One laundry room and its machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaundryRoom {
    pub name: String,
    pub machines: Vec<LaundryMachine>,
}

/// Everything that can stop a laundry read.
#[derive(Debug, thiserror::Error)]
pub enum LaundryError {
    #[error("laundry vendor is not one this client reads")]
    UnknownProvider,

    #[error("laundry read failed")]
    Vendor(#[from] WasherError),
}

impl LaundryError {
    /// The stable, non-secret reason code for this failure.
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::UnknownProvider => "laundry_provider",
            Self::Vendor(error) => error.diagnostic_code(),
        }
    }
}

/// Reads the building groups one laundry vendor offers.
///
/// The vendor key is the caller's own choice among the three the deployment
/// uses; anything else is refused before a request is built.
pub async fn read_laundry_buildings(
    provider: &str,
) -> Result<Vec<LaundryBuildingGroup>, LaundryError> {
    let provider = WasherProvider::parse(provider).ok_or(LaundryError::UnknownProvider)?;
    let adapter = WasherAdapter::for_provider(provider)?;
    let groups = adapter.read_buildings().await?;
    Ok(groups.into_iter().map(group_dto).collect())
}

/// Reads one building's rooms and machines from one laundry vendor.
pub async fn read_laundry_rooms(
    provider: &str,
    building_id: &str,
) -> Result<LaundryRoomsReport, LaundryError> {
    let provider = WasherProvider::parse(provider).ok_or(LaundryError::UnknownProvider)?;
    let adapter = WasherAdapter::for_provider(provider)?;
    let read = adapter.read_rooms(building_id).await?;
    Ok(rooms_dto(provider, read))
}

/// One building's rooms, plus what the vendor did not answer for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaundryRoomsReport {
    /// The vendor this read came from.
    pub provider: String,
    pub rooms: Vec<LaundryRoom>,
    /// Vendor categories that answered with a failure.  A non-empty list means
    /// the read is incomplete in exactly the way it says, rather than silently
    /// short.
    pub failed_categories: Vec<String>,
    /// The vendor's own snapshot time, when it reports one.
    pub fetched_at_unix: Option<i64>,
}

fn group_dto(group: WasherBuildingGroup) -> LaundryBuildingGroup {
    LaundryBuildingGroup {
        key: group.key.to_owned(),
        label: group.label.to_owned(),
        buildings: group
            .buildings
            .into_iter()
            .map(|building: WasherBuilding| LaundryBuilding {
                id: building.id,
                name: building.name,
                provider: building.provider.key().to_owned(),
            })
            .collect(),
    }
}

fn rooms_dto(provider: WasherProvider, read: WasherRoomRead) -> LaundryRoomsReport {
    LaundryRoomsReport {
        provider: provider.key().to_owned(),
        rooms: read
            .rooms
            .into_iter()
            .map(|room: WasherRoom| LaundryRoom {
                name: room.name,
                machines: room
                    .washers
                    .into_iter()
                    .map(|washer: Washer| LaundryMachine {
                        name: washer.name,
                        kind: washer.kind,
                        room: washer.room,
                        status: washer.status.key().to_owned(),
                        eta_minutes: washer.eta_minutes,
                    })
                    .collect(),
            })
            .collect(),
        failed_categories: read.failed_categories,
        fetched_at_unix: read.fetched_at_unix,
    }
}

/// The laundry machine states a caller can rely on.
pub const LAUNDRY_STATUSES: [(&str, &str); 6] = [
    (WasherStatus::Idle.key(), "空闲"),
    (WasherStatus::Working.key(), "使用中"),
    (WasherStatus::Error.key(), "故障"),
    (WasherStatus::Offline.key(), "离线"),
    (WasherStatus::Standby.key(), "待机"),
    (WasherStatus::Unknown.key(), "未知"),
];

/// The laundry vendors this client reads, with their display labels.
pub const LAUNDRY_PROVIDERS: [(&str, &str); 3] = [
    (WasherProvider::Jieli.key(), "杰力洗衣"),
    (WasherProvider::Haile.key(), "海乐生活"),
    (WasherProvider::Xiaolan.key(), "小蓝洗衣"),
];

/// Everything that can stop a water account lookup.
#[derive(Debug, thiserror::Error)]
pub enum WaterLookupError {
    #[error("water lookup failed")]
    Vendor(WaterError),
}

impl WaterLookupError {
    /// The stable, non-secret reason code for this failure.
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Vendor(error) => error.diagnostic_code(),
        }
    }
}

/// Looks up the vendor's record for one delivery number.
///
/// The number is the caller's own input, so a refused value costs no request.
/// The record is the vendor's own, and it is returned as the vendor holds it.
pub async fn read_water_user(delivery_id: &str) -> Result<WaterUser, WaterLookupError> {
    let adapter = WaterAdapter::new().map_err(WaterLookupError::Vendor)?;
    adapter
        .read_user(delivery_id)
        .await
        .map_err(WaterLookupError::Vendor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_laundry_provider_key_is_closed_over_the_three_deployments() {
        for (key, _) in LAUNDRY_PROVIDERS {
            assert!(WasherProvider::parse(key).is_some(), "{key}");
        }
        assert!(
            LAUNDRY_PROVIDERS
                .iter()
                .all(|(key, label)| !key.is_empty() && !label.is_empty())
        );
    }

    #[test]
    fn laundry_status_labels_cover_every_state_the_parsers_can_produce() {
        let mut keys: Vec<&str> = LAUNDRY_STATUSES.iter().map(|(key, _)| *key).collect();
        let expected = [
            WasherStatus::Idle.key(),
            WasherStatus::Working.key(),
            WasherStatus::Error.key(),
            WasherStatus::Offline.key(),
            WasherStatus::Standby.key(),
            WasherStatus::Unknown.key(),
        ];
        keys.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }

    #[test]
    fn a_rooms_report_preserves_the_vendors_partial_answer() {
        let report = rooms_dto(
            WasherProvider::Haile,
            WasherRoomRead {
                rooms: vec![WasherRoom {
                    name: "海乐生活".to_owned(),
                    washers: vec![Washer {
                        name: "W-01".to_owned(),
                        kind: "洗衣机".to_owned(),
                        room: "海乐生活".to_owned(),
                        status: WasherStatus::Idle,
                        eta_minutes: None,
                    }],
                }],
                failed_categories: vec!["01".to_owned()],
                fetched_at_unix: None,
            },
        );
        assert_eq!(report.provider, "haile");
        assert_eq!(report.failed_categories, vec!["01".to_owned()]);
        assert_eq!(report.rooms[0].machines[0].status, "idle");
        assert_eq!(report.rooms[0].machines[0].kind, "洗衣机");
    }

    #[tokio::test]
    async fn an_unknown_laundry_provider_is_refused_without_a_request() {
        // No vendor origin is built for a key this client does not read, so
        // the refusal cannot cost a network round trip.
        assert!(matches!(
            read_laundry_buildings("other").await,
            Err(LaundryError::UnknownProvider)
        ));
        assert!(matches!(
            read_laundry_rooms("", "51").await,
            Err(LaundryError::UnknownProvider)
        ));
    }

    #[test]
    fn diagnostic_codes_are_prefixed_and_distinct_per_boundary() {
        assert_eq!(
            LaundryError::UnknownProvider.diagnostic_code(),
            "laundry_provider"
        );
        let water = WaterLookupError::Vendor(WaterError::InvalidDeliveryId);
        assert_eq!(water.diagnostic_code(), "water_delivery_id");
    }
}
