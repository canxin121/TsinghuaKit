//! Read-only dormitory laundry status (`洗衣机查询`).
//!
//! Three third-party vendors serve the campus laundry rooms, and none of them
//! is a campus-hosted service: the deployment reaches `api.cleverschool.cn`
//! (杰力), `yshz-user.haier-ioc.com` (海乐生活) and `wash-ltd-thu.aajax.top`
//! (小蓝) directly.  There is no WebVPN mapping and no campus account binding
//! for any of the three, so this module is a deliberately separate read
//! boundary rather than an extension of the INFO roaming allowlist:
//!
//! * It never receives the identity/INFO transport.  The adapter builds its own
//!   Cookie-free [`CampusHttpTransport`], so no campus Cookie can reach a
//!   third-party host even though the shared transport type is reused.
//! * Every request still travels through that transport, so it shares the
//!   process-wide request gate, the pacing policy and the exclusive-dispatch
//!   rule for POSTs.  No request is issued through `reqwest` directly.
//! * Request bodies are fixed by the vendors' observed contracts and carry no
//!   account value.  The only caller-supplied input is a building id, which is
//!   bounded before use; the two Haier search points are constants.
//! * The profile has no write representation at all: it reads building lists
//!   and device status and nothing else.
//!
//! Three service behaviours shape the parsers:
//!
//! * Jieli reports a device's state as **Chinese status text** whose blank-
//!   separated segments each carry one fact ("工作中 剩余30分钟").  The
//!   observed reading treats any text it does not recognize as a fault state
//!   rather than as "unknown", so an unrecognized wording is reported as
//!   `Error` — the same as the reference client reports it.  The module does
//!   not soften that into silence, because a machine that stopped reporting a
//!   recognizable state is exactly what a student needs to see.
//! * Jieli's and Haier's answers can be a **partial failure**: one Haier
//!   category failing leaves the other two usable, and the observed reading
//!   skips the failed one rather than failing the whole read.  This module
//!   keeps that behaviour and reports the skipped categories in the read
//!   result, so a missing category is visible instead of looking like a room
//!   that has no dryers.
//! * A field the observed contract always carries — Jieli's `floorName`, or a
//!   device name — is required.  A response missing one has changed shape, and
//!   is reported as [`WasherError::UnexpectedDeployment`] rather than being
//!   skipped into a shorter list.
//! * The campus app's own `/Api/JieliWashers` location enrichment is **not**
//!   part of this module: that endpoint lives on the App-specific
//!   `app.cs.tsinghua.edu.cn` backend, which is outside this boundary.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{fmt, time::Duration};

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::transport::{CampusHttpTransport, TransportError};

/// Jieli's service origin.  It is a third-party host with no WebVPN mapping.
pub const JIELI_ORIGIN: &str = "https://api.cleverschool.cn/";
/// Jieli's building-group endpoint.  Its body is the fixed empty object.
pub const JIELI_TOWER_PATH: &str = "/washapi4/device/tower";
/// Jieli's device-status endpoint for one building.
pub const JIELI_STATUS_PATH: &str = "/washapi4/device/status";

/// Haier's service origin.
pub const HAILE_ORIGIN: &str = "https://yshz-user.haier-ioc.com/";
/// Haier's nearby-position search endpoint.
pub const HAILE_NEAR_POSITION_PATH: &str = "/position/nearPosition";
/// Haier's per-category device listing endpoint.
pub const HAILE_DEVICE_DETAIL_PATH: &str = "/position/deviceDetailPage";

/// Xiaolan's service origin for the Tsinghua deployment.
pub const XIAOLAN_ORIGIN: &str = "https://wash-ltd-thu.aajax.top/";
/// Xiaolan's building list endpoint.
pub const XIAOLAN_LIST_PATH: &str = "/buildings/list";
/// Xiaolan's per-building endpoint prefix.
pub const XIAOLAN_BUILDING_PATH_PREFIX: &str = "/buildings/";

/// Xiaolan's organization key inside its own response envelope.  It is the
/// deployment's public identifier, not a credential.
const XIAOLAN_ORGANIZATION: &str = "67ce4044ba854c556508830e";

/// Xiaolan's single observed room label fallback.
const XIAOLAN_ROOM_FALLBACK: &str = "洗衣房";
/// Haier returns every device under one pseudo-room; this is its label.
const HAILE_ROOM_LABEL: &str = "海乐生活";

/// The two geographic search points whose nearby positions cover the campus.
const HAILE_SEARCH_POINTS: [(f64, f64); 2] = [(116.32697, 40.00281), (116.3424247, 40.0313472)];
/// The page size one Haier nearby-position search asks for.
const HAILE_POSITION_PAGE_SIZE: u32 = 50;
/// The page size one Haier device-category listing asks for.
const HAILE_DEVICE_PAGE_SIZE: u32 = 100;
/// Haier's three device categories, in the observed order.
const HAILE_CATEGORIES: [(&str, &str); 3] = [("00", "洗衣机"), ("01", "洗鞋机"), ("02", "烘干机")];

/// The longest JSON body this module will parse.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// The most buildings one read will report.
const MAX_BUILDINGS: usize = 4_096;
/// The most devices one read will report.
const MAX_WASHERS: usize = 8_192;
/// The longest text field kept from a vendor response.
const MAX_TEXT_CHARS: usize = 128;
/// The longest building id this module will put into a request body or path.
const MAX_BUILDING_ID: usize = 64;

/// Which vendor a building or room belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasherProvider {
    Jieli,
    Haile,
    Xiaolan,
}

impl WasherProvider {
    /// The stable machine key used by callers and by the bridge DTO.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Jieli => "jieli",
            Self::Haile => "haile",
            Self::Xiaolan => "xiaolan",
        }
    }

    /// Parses one machine key.  Only the three observed vendors are accepted.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "jieli" => Some(Self::Jieli),
            "haile" => Some(Self::Haile),
            "xiaolan" => Some(Self::Xiaolan),
            _ => None,
        }
    }

    /// The production origin for this vendor.
    pub const fn origin(self) -> &'static str {
        match self {
            Self::Jieli => JIELI_ORIGIN,
            Self::Haile => HAILE_ORIGIN,
            Self::Xiaolan => XIAOLAN_ORIGIN,
        }
    }
}

/// One device's state, normalized across the three vendors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasherStatus {
    /// Free to use.
    Idle,
    /// Running, with an estimated remaining time when the vendor reports one.
    Working,
    /// Reporting a fault, or reporting a state this module does not recognize.
    Error,
    /// The vendor says the device is offline.
    Offline,
    /// Powered but not in a run.
    Standby,
    /// The vendor's own state value is outside its documented set.
    Unknown,
}

impl WasherStatus {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Error => "error",
            Self::Offline => "offline",
            Self::Standby => "standby",
            Self::Unknown => "unknown",
        }
    }
}

/// One building a vendor offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasherBuilding {
    pub name: String,
    pub id: String,
    pub provider: WasherProvider,
}

/// One vendor-defined group of buildings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasherBuildingGroup {
    /// Stable key for the group (`zijing`, `nanqu`, `shuangqing`, `other`,
    /// `haile`, `xiaolan`).
    pub key: &'static str,
    /// The group's display label.
    pub label: &'static str,
    pub buildings: Vec<WasherBuilding>,
}

/// One device in one room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Washer {
    /// The vendor's device name or machine code.
    pub name: String,
    /// The vendor's own device type text.
    pub kind: String,
    /// The room or floor the device stands in.
    pub room: String,
    pub status: WasherStatus,
    /// Remaining minutes, when the vendor reports them.
    pub eta_minutes: Option<u32>,
}

/// One room and the devices in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasherRoom {
    pub name: String,
    pub washers: Vec<Washer>,
}

/// One building's rooms plus what the vendor did not answer for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasherRoomRead {
    pub rooms: Vec<WasherRoom>,
    /// Haier categories the vendor answered with a failure code.  Each entry is
    /// the category's own key, so a missing device kind is visible rather than
    /// looking like a room that has none.
    pub failed_categories: Vec<String>,
    /// The vendor's own snapshot time, when it reports one.
    pub fetched_at_unix: Option<i64>,
}

/// Everything that can stop a laundry read.
#[derive(Debug, Error)]
pub enum WasherError {
    #[error("laundry service origin is invalid")]
    InvalidBaseUrl,

    #[error("laundry request failed")]
    Transport(#[source] TransportError),

    #[error("laundry request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("laundry response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("laundry response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("laundry response is not a JSON object")]
    NotJson,

    #[error("the laundry building id is not one this client will send")]
    InvalidBuildingId,

    #[error("the laundry vendor reported a business failure")]
    BusinessFailure,
}

impl WasherError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "washer_config",
            Self::Transport(_) => "washer_network",
            Self::HttpStatus { .. } => "washer_http",
            Self::UnexpectedOrigin => "washer_origin",
            Self::UnexpectedDeployment => "washer_template",
            Self::NotJson => "washer_not_json",
            Self::InvalidBuildingId => "washer_building_id",
            Self::BusinessFailure => "washer_business",
        }
    }
}

/// Configuration for the read-only laundry adapter.
///
/// The configuration carries one origin, because one adapter instance reads
/// exactly one vendor.  Production callers reach a vendor only through
/// [`WasherAdapter::for_provider`], so the origin is the vendor's own constant
/// and never a caller-supplied host.
#[derive(Clone)]
pub struct WasherAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl WasherAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, WasherError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/laundry", Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, WasherError> {
        let base_url = Url::parse(base_url).map_err(|_| WasherError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(WasherError::InvalidBaseUrl);
        }
        Ok(Self {
            base_url,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, WasherError> {
        // A dedicated, Cookie-free transport: this boundary must never carry a
        // campus session to a third-party host.  Requests still pass through
        // the shared process-wide request gate.
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(WasherError::Transport)
    }
}

impl fmt::Debug for WasherAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WasherAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Read-only laundry client for one vendor.
pub struct WasherAdapter {
    provider: WasherProvider,
    base_url: Url,
    transport: CampusHttpTransport,
}

impl WasherAdapter {
    /// Builds the adapter for one vendor at that vendor's own origin.
    ///
    /// This is the only production entry point, so a caller cannot aim the
    /// adapter at a host of its own choosing.
    pub fn for_provider(provider: WasherProvider) -> Result<Self, WasherError> {
        Self::try_with_transport(
            provider,
            Url::parse(provider.origin()).map_err(|_| WasherError::InvalidBaseUrl)?,
        )
    }

    /// Builds the adapter for one vendor against a caller-supplied origin,
    /// creating its own Cookie-free transport.  Loopback fixtures use this.
    pub fn try_with_transport(
        provider: WasherProvider,
        base_url: Url,
    ) -> Result<Self, WasherError> {
        let config = WasherAdapterConfig::new(base_url.as_str())?;
        Self::from_config(provider, config)
    }

    fn from_config(
        provider: WasherProvider,
        config: WasherAdapterConfig,
    ) -> Result<Self, WasherError> {
        let transport = config.transport()?;
        Ok(Self {
            provider,
            base_url: config.base_url,
            transport,
        })
    }

    pub fn provider(&self) -> WasherProvider {
        self.provider
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Reads the vendor's building groups.
    pub async fn read_buildings(&self) -> Result<Vec<WasherBuildingGroup>, WasherError> {
        match self.provider {
            WasherProvider::Jieli => self.read_jieli_buildings().await,
            WasherProvider::Haile => self.read_haile_buildings().await,
            WasherProvider::Xiaolan => self.read_xiaolan_buildings().await,
        }
    }

    /// Reads one building's rooms and devices.
    pub async fn read_rooms(&self, building_id: &str) -> Result<WasherRoomRead, WasherError> {
        let building_id = building_id_for_request(building_id)?;
        match self.provider {
            WasherProvider::Jieli => self.read_jieli_rooms(building_id).await,
            WasherProvider::Haile => self.read_haile_rooms(building_id).await,
            WasherProvider::Xiaolan => self.read_xiaolan_rooms(building_id).await,
        }
    }

    async fn read_jieli_buildings(&self) -> Result<Vec<WasherBuildingGroup>, WasherError> {
        let body = self
            .post_json(JIELI_TOWER_PATH, &Value::Object(Map::new()))
            .await?;
        let root = parse_object(&body)?;
        reject_jieli_failure(&root)?;
        let data = array_field(&root, "data")?;
        let mut groups = [
            WasherBuildingGroup {
                key: "zijing",
                label: "紫荆",
                buildings: Vec::new(),
            },
            WasherBuildingGroup {
                key: "nanqu",
                label: "南区",
                buildings: Vec::new(),
            },
            WasherBuildingGroup {
                key: "shuangqing",
                label: "双清",
                buildings: Vec::new(),
            },
            WasherBuildingGroup {
                key: "other",
                label: "其他",
                buildings: Vec::new(),
            },
        ];
        let mut total = 0usize;
        for entry in data {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let Some(name) = text_field(entry, "text") else {
                continue;
            };
            let Some(id) = text_field(entry, "value") else {
                continue;
            };
            // The vendor uses "0" as its own "no building" placeholder.
            if id == "0" {
                continue;
            }
            let index = if name.contains("紫荆") {
                0
            } else if name.contains("南区") {
                1
            } else if name.contains("双清") {
                2
            } else {
                3
            };
            groups[index].buildings.push(WasherBuilding {
                name,
                id,
                provider: WasherProvider::Jieli,
            });
            total += 1;
            if total > MAX_BUILDINGS {
                return Err(WasherError::UnexpectedDeployment);
            }
        }
        for group in &mut groups {
            group
                .buildings
                .sort_by(|left, right| compare_names(&left.name, &right.name));
        }
        Ok(groups.into_iter().collect())
    }

    async fn read_jieli_rooms(&self, building_id: &str) -> Result<WasherRoomRead, WasherError> {
        let mut payload = Map::new();
        payload.insert("towerKey".to_owned(), Value::String(building_id.to_owned()));
        let body = self
            .post_json(JIELI_STATUS_PATH, &Value::Object(payload))
            .await?;
        let root = parse_object(&body)?;
        reject_jieli_failure(&root)?;
        let data = array_field(&root, "data")?;
        let mut rooms: Vec<WasherRoom> = Vec::new();
        let mut total = 0usize;
        for entry in data {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            // The observed contract always carries the room a device stands in
            // and the device's own code.  A response without them has changed
            // shape: dropping those entries would silently shorten the room.
            let room_name =
                text_field(entry, "floorName").ok_or(WasherError::UnexpectedDeployment)?;
            let device_code =
                text_field(entry, "macUnionCode").ok_or(WasherError::UnexpectedDeployment)?;
            let (kind, name) = split_device_code(&device_code);
            let (status, eta_minutes) = jieli_status(entry.get("status"));
            total += 1;
            if total > MAX_WASHERS {
                return Err(WasherError::UnexpectedDeployment);
            }
            let washer = Washer {
                name,
                kind,
                room: room_name.clone(),
                status,
                eta_minutes,
            };
            match rooms.iter_mut().find(|room| room.name == room_name) {
                Some(room) => room.washers.push(washer),
                None => rooms.push(WasherRoom {
                    name: room_name,
                    washers: vec![washer],
                }),
            }
        }
        for room in &mut rooms {
            room.washers
                .sort_by(|left, right| compare_names(&left.name, &right.name));
        }
        rooms.sort_by(|left, right| compare_names(&left.name, &right.name));
        Ok(WasherRoomRead {
            rooms,
            failed_categories: Vec::new(),
            fetched_at_unix: None,
        })
    }

    async fn read_haile_buildings(&self) -> Result<Vec<WasherBuildingGroup>, WasherError> {
        let mut buildings: Vec<WasherBuilding> = Vec::new();
        for (lng, lat) in HAILE_SEARCH_POINTS {
            let mut payload = Map::new();
            payload.insert("lng".to_owned(), number_value(lng));
            payload.insert("lat".to_owned(), number_value(lat));
            payload.insert("page".to_owned(), Value::Number(1.into()));
            payload.insert(
                "pageSize".to_owned(),
                Value::Number(HAILE_POSITION_PAGE_SIZE.into()),
            );
            let body = self
                .post_json(HAILE_NEAR_POSITION_PATH, &Value::Object(payload))
                .await?;
            let root = parse_object(&body)?;
            // A search point outside the vendor's coverage answers with a
            // failure; the observed reading skips that point and keeps the
            // other one, because one covered point is a complete answer.
            if number_field(&root, "code") != Some(0) {
                continue;
            }
            let Some(items) = nested_items(&root) else {
                continue;
            };
            for item in items {
                let name = text_field(item, "name").ok_or(WasherError::UnexpectedDeployment)?;
                let id = scalar_field(item, "id").ok_or(WasherError::UnexpectedDeployment)?;
                if !name.contains("清华") || name.contains("中学") {
                    continue;
                }
                if buildings.iter().any(|building| building.id == id) {
                    continue;
                }
                buildings.push(WasherBuilding {
                    name,
                    id,
                    provider: WasherProvider::Haile,
                });
                if buildings.len() > MAX_BUILDINGS {
                    return Err(WasherError::UnexpectedDeployment);
                }
            }
        }
        buildings.sort_by(|left, right| compare_names(&left.name, &right.name));
        Ok(vec![WasherBuildingGroup {
            key: "haile",
            label: "海乐生活",
            buildings,
        }])
    }

    async fn read_haile_rooms(&self, building_id: &str) -> Result<WasherRoomRead, WasherError> {
        let mut washers: Vec<Washer> = Vec::new();
        let mut failed_categories: Vec<String> = Vec::new();
        for (category, kind) in HAILE_CATEGORIES {
            let mut payload = Map::new();
            payload.insert(
                "positionId".to_owned(),
                Value::String(building_id.to_owned()),
            );
            payload.insert(
                "categoryCode".to_owned(),
                Value::String(category.to_owned()),
            );
            payload.insert("page".to_owned(), Value::Number(1.into()));
            payload.insert("floorCode".to_owned(), Value::String(String::new()));
            payload.insert(
                "pageSize".to_owned(),
                Value::Number(HAILE_DEVICE_PAGE_SIZE.into()),
            );
            let body = self
                .post_json(HAILE_DEVICE_DETAIL_PATH, &Value::Object(payload))
                .await?;
            let root = parse_object(&body)?;
            if number_field(&root, "code") != Some(0) {
                failed_categories.push(category.to_owned());
                continue;
            }
            let Some(items) = nested_items(&root) else {
                failed_categories.push(category.to_owned());
                continue;
            };
            for item in items {
                let name = text_field(item, "name").ok_or(WasherError::UnexpectedDeployment)?;
                let status = match number_field(item, "state") {
                    Some(1) => WasherStatus::Idle,
                    Some(2) => WasherStatus::Working,
                    Some(3) => WasherStatus::Error,
                    _ => WasherStatus::Unknown,
                };
                washers.push(Washer {
                    name,
                    kind: kind.to_owned(),
                    room: HAILE_ROOM_LABEL.to_owned(),
                    status,
                    eta_minutes: None,
                });
                if washers.len() > MAX_WASHERS {
                    return Err(WasherError::UnexpectedDeployment);
                }
            }
        }
        washers.sort_by(|left, right| compare_names(&left.name, &right.name));
        let rooms = if washers.is_empty() {
            Vec::new()
        } else {
            vec![WasherRoom {
                name: HAILE_ROOM_LABEL.to_owned(),
                washers,
            }]
        };
        Ok(WasherRoomRead {
            rooms,
            failed_categories,
            fetched_at_unix: None,
        })
    }

    async fn read_xiaolan_buildings(&self) -> Result<Vec<WasherBuildingGroup>, WasherError> {
        let (buildings, _) = self.read_xiaolan_envelope(XIAOLAN_LIST_PATH).await?;
        let mut result: Vec<WasherBuilding> = Vec::new();
        for (key, value) in buildings {
            let Some(building) = value.as_object() else {
                continue;
            };
            let id = text_field(building, "buildingId").unwrap_or_else(|| key.clone());
            let name = text_field(building, "name").unwrap_or_else(|| id.clone());
            result.push(WasherBuilding {
                name,
                id,
                provider: WasherProvider::Xiaolan,
            });
            if result.len() > MAX_BUILDINGS {
                return Err(WasherError::UnexpectedDeployment);
            }
        }
        result.sort_by(|left, right| compare_names(&left.name, &right.name));
        Ok(vec![WasherBuildingGroup {
            key: "xiaolan",
            label: "小蓝洗衣",
            buildings: result,
        }])
    }

    async fn read_xiaolan_rooms(&self, building_id: &str) -> Result<WasherRoomRead, WasherError> {
        let path = format!("{XIAOLAN_BUILDING_PATH_PREFIX}{building_id}");
        let (buildings, fetched_at_unix) = self.read_xiaolan_envelope(&path).await?;
        let building = buildings
            .get(building_id)
            .and_then(Value::as_object)
            .ok_or(WasherError::UnexpectedDeployment)?;
        let facilities = building
            .get("facilities")
            .and_then(Value::as_array)
            .ok_or(WasherError::UnexpectedDeployment)?;
        let mut rooms: Vec<WasherRoom> = Vec::new();
        let mut total = 0usize;
        for facility in facilities {
            let Some(facility) = facility.as_object() else {
                continue;
            };
            let store = facility.get("store").and_then(Value::as_object);
            let detail = facility.get("storeDetail").and_then(Value::as_object);
            let floor = store
                .and_then(|store| text_field(store, "floor"))
                .unwrap_or_default();
            let room_name = detail
                .and_then(|detail| text_field(detail, "name"))
                .or_else(|| store.and_then(|store| text_field(store, "opStoreName")))
                .unwrap_or_else(|| {
                    if floor.is_empty() {
                        XIAOLAN_ROOM_FALLBACK.to_owned()
                    } else {
                        format!("{XIAOLAN_ROOM_FALLBACK} {floor}")
                    }
                });
            let devices = facility
                .get("devices")
                .and_then(Value::as_array)
                .ok_or(WasherError::UnexpectedDeployment)?;
            let mut washers: Vec<Washer> = Vec::new();
            for device in devices {
                let Some(device) = device.as_object() else {
                    continue;
                };
                let device_id =
                    text_field(device, "deviceId").ok_or(WasherError::UnexpectedDeployment)?;
                let name = text_field(device, "deviceCode").unwrap_or_else(|| device_id.clone());
                let kind = match text_field(device, "type").as_deref() {
                    Some("1") => "洗衣机",
                    Some("2") => "烘干机",
                    Some("3") => "洗烘一体机",
                    Some("4") => "洗鞋机",
                    _ => "其他",
                };
                let state = device.get("deviceState").and_then(Value::as_object);
                let status = xiaolan_status(state);
                let eta_minutes = state
                    .and_then(|state| state.get("inUseBit"))
                    .and_then(Value::as_object)
                    .and_then(|bit| bit.get("estimatedCompleteTime"))
                    .and_then(remaining_minutes);
                total += 1;
                if total > MAX_WASHERS {
                    return Err(WasherError::UnexpectedDeployment);
                }
                washers.push(Washer {
                    name,
                    kind: kind.to_owned(),
                    room: room_name.clone(),
                    status,
                    eta_minutes,
                });
            }
            washers.sort_by(|left, right| compare_names(&left.name, &right.name));
            rooms.push(WasherRoom {
                name: room_name,
                washers,
            });
        }
        rooms.sort_by(|left, right| compare_names(&left.name, &right.name));
        Ok(WasherRoomRead {
            rooms,
            failed_categories: Vec::new(),
            fetched_at_unix,
        })
    }

    /// Reads Xiaolan's own envelope: its organization object and buildings.
    async fn read_xiaolan_envelope(
        &self,
        path: &str,
    ) -> Result<(Map<String, Value>, Option<i64>), WasherError> {
        let body = self.get(path).await?;
        let root = parse_object(&body)?;
        let organization = root
            .get(XIAOLAN_ORGANIZATION)
            .and_then(Value::as_object)
            .ok_or(WasherError::UnexpectedDeployment)?;
        let buildings = organization
            .get("buildings")
            .and_then(Value::as_object)
            .ok_or(WasherError::UnexpectedDeployment)?;
        let fetched_at_unix = organization.get("fetchedAt").and_then(snapshot_seconds);
        Ok((buildings.clone(), fetched_at_unix))
    }

    async fn get(&self, path: &str) -> Result<String, WasherError> {
        let endpoint = self.endpoint(path)?;
        let request = self
            .transport
            .client()
            .get(endpoint)
            .build()
            .map_err(TransportError::Request)
            .map_err(WasherError::Transport)?;
        self.execute(request).await
    }

    async fn post_json(&self, path: &str, payload: &Value) -> Result<String, WasherError> {
        let endpoint = self.endpoint(path)?;
        let request = self
            .transport
            .client()
            .post(endpoint)
            .json(payload)
            .build()
            .map_err(TransportError::Request)
            .map_err(WasherError::Transport)?;
        self.execute(request).await
    }

    async fn execute(&self, request: reqwest::Request) -> Result<String, WasherError> {
        let expected_path = request.url().path().to_owned();
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(|error| WasherError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| WasherError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(WasherError::UnexpectedDeployment);
        }
        if !same_origin(&self.base_url, &final_url)
            || location
                .as_deref()
                .and_then(|value| final_url.join(value).ok())
                .is_some_and(|target| !same_origin(&self.base_url, &target))
        {
            return Err(WasherError::UnexpectedOrigin);
        }
        if status != StatusCode::OK {
            return Err(WasherError::HttpStatus { status });
        }
        if final_url.path() != expected_path {
            return Err(WasherError::UnexpectedDeployment);
        }
        if !is_json_content_type(content_type.as_deref()) || looks_like_html(&body) {
            return Err(WasherError::NotJson);
        }
        Ok(body)
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, WasherError> {
        if !relative_path.starts_with('/')
            || relative_path.contains("://")
            || relative_path.contains(['?', '#'])
            || relative_path.contains("..")
            || relative_path.chars().any(char::is_control)
        {
            return Err(WasherError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{relative_path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }
}

impl fmt::Debug for WasherAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WasherAdapter")
            .field("provider", &self.provider.key())
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .finish()
    }
}

/// Bounds one caller-supplied building id before it can enter a body or path.
fn building_id_for_request(building_id: &str) -> Result<&str, WasherError> {
    if building_id.is_empty()
        || building_id.len() > MAX_BUILDING_ID
        || !building_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(WasherError::InvalidBuildingId);
    }
    Ok(building_id)
}

/// Splits Jieli's `"<type> <name>"` device code.
///
/// A code without the separating space is kept whole in both halves rather
/// than dropped: the vendor's device list is the answer, and a changed code
/// format must not empty a room.
fn split_device_code(code: &str) -> (String, String) {
    match code.split_once(' ') {
        Some((kind, name)) => (kind.trim().to_owned(), name.trim().to_owned()),
        None => (code.trim().to_owned(), code.trim().to_owned()),
    }
}

/// Reads Jieli's Chinese status text into a state and a remaining time.
///
/// The observed reading starts from a fault state and only leaves it when a
/// segment names a recognized state, so unrecognized (or newly worded) text is
/// reported as a fault rather than hidden as `unknown`.
fn jieli_status(value: Option<&Value>) -> (WasherStatus, Option<u32>) {
    let Some(text) = value.and_then(Value::as_str) else {
        return (WasherStatus::Error, None);
    };
    let mut status = WasherStatus::Error;
    let mut eta_minutes = None;
    for part in text.split_whitespace() {
        if part.contains("剩余") {
            eta_minutes = first_number(part).and_then(|value| u32::try_from(value).ok());
        } else if part.contains("更新") {
            continue;
        } else if part.contains("待机") {
            status = WasherStatus::Idle;
        } else if part.contains("工作") || part.contains("运转") {
            status = WasherStatus::Working;
        }
    }
    (status, eta_minutes)
}

/// Reads Xiaolan's own device-state object.
///
/// The vendor's flag semantics are followed exactly: `isOnline` 0 means
/// offline, a non-zero `fault` means a fault, and any other `isOnline` value
/// leaves the state unknown rather than guessed.
fn xiaolan_status(state: Option<&Map<String, Value>>) -> WasherStatus {
    let Some(state) = state else {
        return WasherStatus::Unknown;
    };
    let is_online = number_field(state, "isOnline");
    if is_online == Some(0) {
        return WasherStatus::Offline;
    }
    if state.get("fault").is_some_and(is_truthy) {
        return WasherStatus::Error;
    }
    if is_online != Some(1) {
        return WasherStatus::Unknown;
    }
    match number_field(state, "runState") {
        Some(7) => WasherStatus::Idle,
        Some(5) => WasherStatus::Working,
        Some(1) => WasherStatus::Standby,
        _ => WasherStatus::Unknown,
    }
}

/// Reads a vendor fault flag, which one deployment reports as a number, a
/// boolean, or a string.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !matches!(value.trim(), "" | "0" | "false"),
        _ => false,
    }
}

/// Reads one JSON scalar into a bounded string.
fn scalar_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    match object.get(key)? {
        Value::String(value) => bounded(value),
        Value::Number(number) => bounded(&number.to_string()),
        _ => None,
    }
}

/// Reads one JSON string field into a bounded string.
fn text_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    scalar_field(object, key)
}

/// Reads one JSON numeric field.
fn number_field(object: &Map<String, Value>, key: &str) -> Option<i64> {
    match object.get(key)? {
        Value::Number(number) => number.as_i64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

/// Reads one JSON object's `data.items` entries.
fn nested_items(root: &Map<String, Value>) -> Option<Vec<&Map<String, Value>>> {
    let items = root
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)?;
    Some(items.iter().filter_map(Value::as_object).collect())
}

fn number_value(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// Rejects Jieli's business-failure envelope.  The vendor signals failure by
/// the presence of an `errorCode` field rather than by a truthy value, so its
/// presence is what this checks.
fn reject_jieli_failure(root: &Map<String, Value>) -> Result<(), WasherError> {
    match root.get("errorCode") {
        None | Some(Value::Null) => Ok(()),
        Some(_) => Err(WasherError::BusinessFailure),
    }
}

fn array_field<'a>(root: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>, WasherError> {
    root.get(key)
        .and_then(Value::as_array)
        .ok_or(WasherError::UnexpectedDeployment)
}

fn parse_object(body: &str) -> Result<Map<String, Value>, WasherError> {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    if trimmed.is_empty() || looks_like_html(trimmed) {
        return Err(WasherError::NotJson);
    }
    let root: Value = serde_json::from_str(trimmed).map_err(|_| WasherError::NotJson)?;
    match root {
        Value::Object(object) => Ok(object),
        _ => Err(WasherError::NotJson),
    }
}

fn bounded(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.chars().take(MAX_TEXT_CHARS).collect())
}

/// Returns the first run of ASCII digits in `text` as an integer.
fn first_number(text: &str) -> Option<i64> {
    let digits: String = text
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Reads a vendor timestamp as Unix seconds.
fn snapshot_seconds(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => chrono::DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|parsed| parsed.timestamp())
            .or_else(|| text.trim().parse().ok()),
        _ => None,
    }
}

/// Reads a Xiaolan completion timestamp as remaining whole minutes.
///
/// A timestamp in the past means the run has ended, so nothing is reported
/// rather than a negative duration presented as a wait.
fn remaining_minutes(value: &Value) -> Option<u32> {
    let text = match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let completion = chrono::DateTime::parse_from_rfc3339(&text)
        .ok()
        .map(|parsed| parsed.timestamp())
        .or_else(|| text.parse::<i64>().ok())?;
    let remaining = completion.checked_sub(chrono::Utc::now().timestamp())?;
    (remaining > 0).then(|| u32::try_from(remaining.div_euclid(60)).unwrap_or(u32::MAX))
}

/// Compares two device or building names, ordering a leading number
/// numerically so `10号楼` follows `2号楼` rather than preceding it.
fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (first_number(left), first_number(right)) {
        (Some(left_number), Some(right_number)) => match left_number.cmp(&right_number) {
            Ordering::Equal => left.cmp(right),
            other => other,
        },
        _ => left.cmp(right),
    }
}

fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("application/json") || mime.eq_ignore_ascii_case("text/json")
    })
}

fn looks_like_html(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<!doctype")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<HTML")
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn normalize_base_url(mut base_url: Url) -> Result<Url, WasherError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        return Err(WasherError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = if path.is_empty() {
        "/".to_owned()
    } else {
        format!("{path}/")
    };
    base_url.set_path(&path);
    Ok(base_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_test_support::{FixtureServer, Reply};

    fn adapter(provider: WasherProvider, server: &FixtureServer) -> WasherAdapter {
        let base = Url::parse(server.base()).expect("fixture base");
        WasherAdapter::try_with_transport(provider, base).expect("adapter")
    }

    #[tokio::test]
    async fn jieli_buildings_group_and_sort_numerically() {
        let server = FixtureServer::new(vec![Reply::json(
            r#"{"data":[
                {"text":"紫荆10号楼","value":"52"},
                {"text":"紫荆2号楼","value":"51"},
                {"text":"南区29号楼","value":"60"},
                {"text":"双清公寓","value":"70"},
                {"text":"家属区","value":"80"},
                {"text":"占位","value":"0"}
            ]}"#,
        )]);
        let adapter = adapter(WasherProvider::Jieli, &server);
        let groups = adapter.read_buildings().await.expect("buildings");
        assert_eq!(groups.len(), 4);
        let zijing: Vec<&str> = groups[0]
            .buildings
            .iter()
            .map(|building| building.name.as_str())
            .collect();
        assert_eq!(zijing, vec!["紫荆2号楼", "紫荆10号楼"]);
        assert_eq!(groups[0].key, "zijing");
        assert_eq!(groups[0].buildings[0].id, "51");
        assert_eq!(groups[1].buildings[0].name, "南区29号楼");
        assert_eq!(groups[2].key, "shuangqing");
        assert_eq!(groups[3].buildings[0].name, "家属区");
        // The vendor's "0" placeholder never becomes a selectable building.
        assert!(
            groups
                .iter()
                .all(|group| group.buildings.iter().all(|building| building.id != "0"))
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /washapi4/device/tower "));
    }

    #[tokio::test]
    async fn jieli_business_failure_is_not_an_empty_list() {
        let server = FixtureServer::new(vec![Reply::json(r#"{"errorCode":1,"errorMsg":"busy"}"#)]);
        let adapter = adapter(WasherProvider::Jieli, &server);
        assert!(matches!(
            adapter.read_buildings().await,
            Err(WasherError::BusinessFailure)
        ));
    }

    #[tokio::test]
    async fn jieli_rooms_read_status_text_and_keep_unrecognized_as_fault() {
        let server = FixtureServer::new(vec![Reply::json(
            r#"{"data":[
                {"floorName":"3层","macUnionCode":"洗衣机 03-1","status":"工作中 剩余30分钟 更新:12:00"},
                {"floorName":"3层","macUnionCode":"洗衣机 03-2","status":"待机"},
                {"floorName":"4层","macUnionCode":"烘干机 04-1","status":"维修中"},
                {"floorName":"4层","macUnionCode":"烘干机04-2","status":"运转 剩余5分钟"}
            ]}"#,
        )]);
        let adapter = adapter(WasherProvider::Jieli, &server);
        let read = adapter.read_rooms("51").await.expect("rooms");
        assert_eq!(read.rooms.len(), 2);
        assert!(read.failed_categories.is_empty());
        let third = &read.rooms[0];
        assert_eq!(third.name, "3层");
        assert_eq!(third.washers[0].status, WasherStatus::Working);
        assert_eq!(third.washers[0].eta_minutes, Some(30));
        assert_eq!(third.washers[0].kind, "洗衣机");
        assert_eq!(third.washers[0].name, "03-1");
        assert_eq!(third.washers[0].room, "3层");
        assert_eq!(third.washers[1].status, WasherStatus::Idle);
        // The rooms are ordered too, not just the devices inside them.
        let fourth = &read.rooms[1];
        assert_eq!(fourth.name, "4层");
        // A wording this module does not recognize is a fault, not silence.
        assert_eq!(fourth.washers[0].status, WasherStatus::Error);
        assert_eq!(fourth.washers[1].status, WasherStatus::Working);
        assert_eq!(fourth.washers[1].eta_minutes, Some(5));
        // A code without the separating space is kept whole, not dropped.
        assert_eq!(fourth.washers[1].kind, "烘干机04-2");
        // The building id is bounded before it enters the body.
        assert!(matches!(
            adapter.read_rooms("51/../evil").await,
            Err(WasherError::InvalidBuildingId)
        ));
        assert!(matches!(
            adapter.read_rooms("").await,
            Err(WasherError::InvalidBuildingId)
        ));
    }

    #[tokio::test]
    async fn jieli_device_without_a_room_is_a_changed_deployment() {
        let server = FixtureServer::new(vec![Reply::json(
            r#"{"data":[{"macUnionCode":"洗衣机 03-1","status":"待机"}]}"#,
        )]);
        let adapter = adapter(WasherProvider::Jieli, &server);
        assert!(matches!(
            adapter.read_rooms("51").await,
            Err(WasherError::UnexpectedDeployment)
        ));
    }

    #[tokio::test]
    async fn haile_buildings_filter_and_deduplicate_across_search_points() {
        let server = FixtureServer::new(vec![
            Reply::json(r#"{"code":0,"data":{"items":[{"id":7,"name":"清华大学紫荆公寓"}]}}"#),
            Reply::json(
                r#"{"code":0,"data":{"items":[
                    {"id":7,"name":"清华大学紫荆公寓"},
                    {"id":8,"name":"清华大学附属中学"},
                    {"id":9,"name":"北京大学宿舍"}
                ]}}"#,
            ),
        ]);
        let adapter = adapter(WasherProvider::Haile, &server);
        let groups = adapter.read_buildings().await.expect("buildings");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "haile");
        assert_eq!(groups[0].buildings.len(), 1);
        assert_eq!(groups[0].buildings[0].id, "7");
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn haile_rooms_report_the_failed_category() {
        let server = FixtureServer::new(vec![
            Reply::json(r#"{"code":0,"data":{"items":[{"name":"W-01","state":1}]}}"#),
            Reply::json(r#"{"code":500,"message":"busy"}"#),
            Reply::json(r#"{"code":0,"data":{"items":[{"name":"D-01","state":2}]}}"#),
        ]);
        let adapter = adapter(WasherProvider::Haile, &server);
        let read = adapter.read_rooms("7").await.expect("rooms");
        assert_eq!(read.failed_categories, vec!["01".to_owned()]);
        assert_eq!(read.rooms.len(), 1);
        let kinds: Vec<&str> = read.rooms[0]
            .washers
            .iter()
            .map(|washer| washer.kind.as_str())
            .collect();
        assert!(kinds.contains(&"洗衣机"));
        assert!(kinds.contains(&"烘干机"));
        assert!(!kinds.contains(&"洗鞋机"));
        assert_eq!(server.requests().len(), 3);
    }

    /// The fixtures below spell out the organization key the vendor uses, so
    /// the JSON stays a single readable line instead of brace escaping.
    #[tokio::test]
    async fn xiaolan_reads_rooms_and_devices_from_its_own_envelope() {
        let list = r#"{"67ce4044ba854c556508830e": {"buildings": {"b1": {"buildingId": "b1", "name": "紫荆22号楼"}}, "fetchedAt": 1735689600}}"#;
        let detail = r#"{"67ce4044ba854c556508830e": {"fetchedAt": 1735689600, "buildings": {"b1": {"facilities": [{"store": {"storeId": "s1", "floor": "2"}, "storeDetail": {"name": "2层洗衣房"}, "devices": [{"deviceId": "d1", "deviceCode": "A1", "type": "1", "deviceState": {"isOnline": 1, "fault": 0, "runState": 7}}, {"deviceId": "d2", "deviceCode": "A2", "type": "2", "deviceState": {"isOnline": 1, "fault": 0, "runState": 5, "inUseBit": {"estimatedCompleteTime": "2099-01-01T00:00:00+08:00"}}}, {"deviceId": "d3", "deviceCode": "A3", "type": "4", "deviceState": {"isOnline": 0, "fault": 0, "runState": 7}}, {"deviceId": "d4", "deviceCode": "A4", "type": "3", "deviceState": {"isOnline": 1, "fault": 1, "runState": 1}}, {"deviceId": "d5", "type": "9", "deviceState": {"isOnline": 1, "fault": 0, "runState": 1}}]}]}}}}"#;
        let server = FixtureServer::new(vec![Reply::json(list), Reply::json(detail)]);
        let adapter = adapter(WasherProvider::Xiaolan, &server);
        let groups = adapter.read_buildings().await.expect("buildings");
        assert_eq!(groups[0].key, "xiaolan");
        assert_eq!(groups[0].buildings[0].name, "紫荆22号楼");
        assert_eq!(groups[0].buildings[0].id, "b1");
        let read = adapter.read_rooms("b1").await.expect("rooms");
        assert_eq!(read.rooms.len(), 1);
        assert_eq!(read.rooms[0].name, "2层洗衣房");
        assert_eq!(read.fetched_at_unix, Some(1735689600));
        assert!(read.failed_categories.is_empty());
        let by_name = |name: &str| {
            read.rooms[0]
                .washers
                .iter()
                .find(|washer| washer.name == name)
                .expect("device")
        };
        assert_eq!(by_name("A1").status, WasherStatus::Idle);
        assert_eq!(by_name("A1").kind, "洗衣机");
        assert_eq!(by_name("A1").room, "2层洗衣房");
        assert_eq!(by_name("A2").status, WasherStatus::Working);
        assert_eq!(by_name("A2").kind, "烘干机");
        assert!(by_name("A2").eta_minutes.is_some());
        assert_eq!(by_name("A3").status, WasherStatus::Offline);
        assert_eq!(by_name("A3").kind, "洗鞋机");
        assert_eq!(by_name("A4").status, WasherStatus::Error);
        assert_eq!(by_name("A4").kind, "洗烘一体机");
        // A device with no code still appears, under its own id.
        assert_eq!(by_name("d5").kind, "其他");
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn xiaolan_unknown_building_is_a_changed_deployment() {
        let detail = r#"{"67ce4044ba854c556508830e": {"fetchedAt": 1735689600, "buildings": {"other": {"facilities": []}}}}"#;
        let server = FixtureServer::new(vec![Reply::json(detail)]);
        let adapter = adapter(WasherProvider::Xiaolan, &server);
        assert!(matches!(
            adapter.read_rooms("b1").await,
            Err(WasherError::UnexpectedDeployment)
        ));
    }

    #[tokio::test]
    async fn xiaolan_facility_without_a_device_list_is_a_changed_deployment() {
        let detail = r#"{"67ce4044ba854c556508830e": {"buildings": {"b1": {"facilities": [{"store": {"floor": "2"}}]}}}}"#;
        let server = FixtureServer::new(vec![Reply::json(detail)]);
        let adapter = adapter(WasherProvider::Xiaolan, &server);
        assert!(matches!(
            adapter.read_rooms("b1").await,
            Err(WasherError::UnexpectedDeployment)
        ));
    }

    #[tokio::test]
    async fn xiaolan_listing_without_the_organization_object_is_a_changed_deployment() {
        let server = FixtureServer::new(vec![Reply::json(r#"{"other":{}}"#)]);
        let adapter = adapter(WasherProvider::Xiaolan, &server);
        assert!(matches!(
            adapter.read_buildings().await,
            Err(WasherError::UnexpectedDeployment)
        ));
    }

    #[tokio::test]
    async fn an_html_page_is_never_an_empty_result() {
        let server = FixtureServer::new(vec![Reply::html("<html><body>login</body></html>")]);
        let adapter = adapter(WasherProvider::Jieli, &server);
        assert!(matches!(
            adapter.read_buildings().await,
            Err(WasherError::NotJson)
        ));
    }

    #[tokio::test]
    async fn a_non_success_status_is_not_an_empty_result() {
        let server = FixtureServer::new(vec![Reply {
            status: 503,
            headers: "Content-Type: application/json\r\n".into(),
            body: r#"{"data":[]}"#.into(),
        }]);
        let adapter = adapter(WasherProvider::Jieli, &server);
        assert!(matches!(
            adapter.read_buildings().await,
            Err(WasherError::HttpStatus {
                status: StatusCode::SERVICE_UNAVAILABLE
            })
        ));
    }

    #[test]
    fn an_origin_is_never_accepted_with_credentials_or_a_foreign_shape() {
        for rejected in [
            "ftp://api.cleverschool.cn/",
            "https://user:secret@api.cleverschool.cn/",
            "https://api.cleverschool.cn/?token=1",
            "https://api.cleverschool.cn/#fragment",
            "not a url",
        ] {
            assert!(
                matches!(
                    WasherAdapterConfig::new(rejected),
                    Err(WasherError::InvalidBaseUrl)
                ),
                "{rejected} must be refused"
            );
        }
        assert!(WasherAdapterConfig::new("https://api.cleverschool.cn").is_ok());
    }

    #[test]
    fn a_relative_path_cannot_leave_the_vendor_origin() {
        let config = WasherAdapterConfig::new("https://api.cleverschool.cn/").expect("config");
        let transport = config.transport().expect("transport");
        let adapter = WasherAdapter {
            provider: WasherProvider::Jieli,
            base_url: config.base_url().clone(),
            transport,
        };
        for rejected in [
            "washapi4/device/tower",
            "/washapi4/../secret",
            "/washapi4/device/tower?x=1",
            "/washapi4/device/tower#frag",
            "/washapi4/://evil",
            "/washapi4/device/\u{7}tower",
        ] {
            assert!(
                matches!(adapter.endpoint(rejected), Err(WasherError::InvalidBaseUrl)),
                "{rejected} must be refused"
            );
        }
        let accepted = adapter
            .endpoint("/washapi4/device/tower")
            .expect("endpoint");
        assert_eq!(
            accepted.as_str(),
            "https://api.cleverschool.cn/washapi4/device/tower"
        );
    }

    #[test]
    fn provider_keys_round_trip_and_reject_unknown_values() {
        for provider in [
            WasherProvider::Jieli,
            WasherProvider::Haile,
            WasherProvider::Xiaolan,
        ] {
            assert_eq!(WasherProvider::parse(provider.key()), Some(provider));
            // None of the three vendors is reached over plain HTTP.
            assert!(provider.origin().starts_with("https://"));
        }
        assert_eq!(WasherProvider::parse("other"), None);
        assert_eq!(WasherProvider::parse(""), None);
    }

    #[test]
    fn status_keys_are_stable() {
        assert_eq!(WasherStatus::Idle.key(), "idle");
        assert_eq!(WasherStatus::Working.key(), "working");
        assert_eq!(WasherStatus::Error.key(), "error");
        assert_eq!(WasherStatus::Offline.key(), "offline");
        assert_eq!(WasherStatus::Standby.key(), "standby");
        assert_eq!(WasherStatus::Unknown.key(), "unknown");
    }

    #[test]
    fn a_future_completion_time_becomes_remaining_minutes_and_a_past_one_does_not() {
        assert!(remaining_minutes(&Value::String("2099-01-01T00:00:00+08:00".into())).is_some());
        assert!(remaining_minutes(&Value::String("2000-01-01T00:00:00+08:00".into())).is_none());
        assert!(remaining_minutes(&Value::Null).is_none());
        assert!(remaining_minutes(&Value::String("not a time".into())).is_none());
    }

    #[test]
    fn diagnostic_codes_are_distinct_per_failure_class() {
        let codes = [
            WasherError::InvalidBaseUrl.diagnostic_code(),
            WasherError::UnexpectedOrigin.diagnostic_code(),
            WasherError::UnexpectedDeployment.diagnostic_code(),
            WasherError::NotJson.diagnostic_code(),
            WasherError::InvalidBuildingId.diagnostic_code(),
            WasherError::BusinessFailure.diagnostic_code(),
        ];
        let mut unique = codes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len());
        assert!(codes.iter().all(|code| code.starts_with("washer_")));
    }

    #[test]
    fn debug_output_never_carries_a_body_or_a_query() {
        let config = WasherAdapterConfig::new("https://api.cleverschool.cn/").expect("config");
        let rendered = format!("{config:?}");
        assert!(rendered.contains("api.cleverschool.cn"));
        assert!(!rendered.contains("token"));
    }
}
