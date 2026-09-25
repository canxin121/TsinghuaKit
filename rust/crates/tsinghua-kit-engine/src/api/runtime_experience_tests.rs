//! Local files only. No real account or campus HTTP requests.
use super::super::experience::{ExperiencePreferencesDto, ExperiencePreferencesPayload};
use super::*;

struct Fixture {
    root: PathBuf,
    runtime: CampusRuntime,
    username: String,
}

#[test]
fn backend_repair_autoload_catalog_authorizes_lazy_reads_without_fabricating_service_proofs() {
    let fixture = Fixture::new("fixture-autoload-account");
    let catalog = fixture.runtime.service_catalog();
    for id in [
        "learn",
        "registrar",
        "info",
        "library",
        "classroom",
        "electricity",
        "campus_card",
    ] {
        let entry = catalog
            .services
            .iter()
            .find(|entry| entry.id == id)
            .unwrap();
        assert_eq!(entry.availability, "load_on_demand", "{id}");
        assert!(!entry.capabilities.is_empty());
        assert!(
            entry
                .capabilities
                .iter()
                .all(|capability| capability.key.starts_with("read_")
                    && capability.access == "read"
                    && capability.authentication == "identity_and_service_session")
        );
    }
    assert!(!fixture.runtime.service_session_is_proven(ServiceId::Learn));
    assert!(
        !fixture
            .runtime
            .service_session_is_proven(ServiceId::Library)
    );
    assert!(fixture.runtime.learn_source.is_none());
    for id in ["usereg", "tunet"] {
        let entry = catalog
            .services
            .iter()
            .find(|entry| entry.id == id)
            .unwrap();
        assert_eq!(entry.availability, "requires_independent_login");
        assert!(entry.capabilities.is_empty());
    }
}

#[test]
fn backend_repair_autoload_catalog_keeps_account_stage_and_logout_boundaries() {
    let mut fixture = Fixture::new("fixture-autoload-stage");
    fixture.runtime.stage = AcademicStage::Graduate;
    let catalog = fixture.runtime.service_catalog();
    let registrar = catalog
        .services
        .iter()
        .find(|entry| entry.id == "registrar")
        .unwrap();
    assert!(
        registrar
            .capabilities
            .iter()
            .any(|capability| capability.key == "read_grades")
    );
    assert!(
        !registrar
            .capabilities
            .iter()
            .any(|capability| capability.key == "read_exams")
    );
    fixture.runtime.persistence_root = fixture.root.clone();
    fixture.runtime.logout().unwrap();
    assert!(
        fixture
            .runtime
            .service_catalog()
            .services
            .iter()
            .all(|entry| entry.availability != "load_on_demand")
    );
}

#[test]
fn backend_repair_autoload_catalog_never_adopts_another_accounts_downstream_session() {
    let mut fixture = Fixture::new("fixture-autoload-owner");
    fixture
        .runtime
        .coordinator
        .begin_authentication(ServiceId::Library)
        .unwrap();
    fixture
        .runtime
        .coordinator
        .mark_authenticated(
            ServiceId::Library,
            UserIdentity {
                username: "different-fixture-owner".into(),
                display_name: None,
            },
            None,
            None,
            None,
        )
        .unwrap();
    let catalog = fixture.runtime.service_catalog();
    let library = catalog
        .services
        .iter()
        .find(|entry| entry.id == "library")
        .unwrap();
    assert_ne!(library.availability, "load_on_demand");
    assert!(library.capabilities.is_empty());
}
impl Fixture {
    fn new(username: &str) -> Self {
        let root = std::env::temp_dir().join(format!("thyou-experience-{}", Uuid::new_v4()));
        let runtime = CampusRuntime::new_with_persistence(
            "2026-2027-1".into(),
            false,
            root.join("cache.json").to_string_lossy().into(),
            false,
        )
        .unwrap();
        let mut fixture = Self {
            root,
            runtime,
            username: username.to_owned(),
        };
        fixture.install_identity();
        fixture
    }
    fn install_identity(&mut self) {
        self.runtime
            .coordinator
            .begin_authentication(ServiceId::Identity)
            .unwrap();
        self.runtime
            .coordinator
            .mark_authenticated(
                ServiceId::Identity,
                UserIdentity {
                    username: self.username.clone(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .unwrap();
    }
    fn write_overview(&self, age_minutes: i64) -> (String, PathBuf) {
        let now = Utc::now();
        let date = (now + chrono::Duration::hours(8)).date_naive();
        let observed = now - chrono::Duration::minutes(age_minutes);
        let overview = CampusOverview {
            date,
            generated_at: observed,
            semester: Some("2026-2027-1".into()),
            course_count: 0,
            pending_todo_count: 0,
            completed_todo_count: 0,
            today_schedule: vec![],
            upcoming_todos: vec![],
            next_schedule: None,
        };
        let path = overview_cache_path_for_account_stage(
            &self.runtime.cache_path,
            &self.username,
            self.runtime.stage,
        );
        JsonFileCache::<CampusOverviewCachePayload>::new(
            &path,
            CAMPUS_OVERVIEW_CACHE_SCHEMA_VERSION,
            CAMPUS_OVERVIEW_CACHE_SERVICE,
        )
        .write(CampusOverviewCachePayload {
            account_scope: cache_account_scope(&self.username),
            stage: self.runtime.stage,
            date,
            generated_at: observed,
            overview,
        })
        .unwrap();
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        envelope["saved_at"] = serde_json::Value::String(observed.timestamp_millis().to_string());
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        (date.to_string(), path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn backend_repair_login_defaults_missing_credential_keeps_failure_without_checkbox_instruction() {
    let mut fixture = Fixture::new("fixture-login-default-recovery");
    fixture.runtime.remember_credentials = true;
    fixture.runtime.primary_password = None;
    fixture.runtime.persist_opt_in_credential(&UserIdentity {
        username: fixture.username.clone(),
        display_name: None,
    });
    assert!(!fixture.runtime.credentials_persisted);
    assert_eq!(
        fixture.runtime.credential_storage_warning.as_deref(),
        Some("自动续接凭证未保留，请重新登录")
    );
}

#[test]
fn backend_repair_login_defaults_logout_disables_recovery_and_does_not_unlock_cached_account() {
    let mut fixture = Fixture::new("fixture-login-default-logout");
    fixture.runtime.persistence_root = fixture.root.clone();
    fixture.runtime.remember_credentials = true;
    fixture.runtime.credentials_persisted = true;
    fixture.runtime.credential_username = Some(fixture.username.clone());
    let status = fixture.runtime.logout().unwrap();
    assert_eq!(status.state, "signed_out");
    assert!(!status.automatic_recovery_enabled);
    assert!(!fixture.runtime.remember_credentials);
    assert!(!fixture.runtime.credentials_persisted);
    assert!(fixture.runtime.cache_user_for_read().unwrap().is_none());
    assert!(
        fixture
            .runtime
            .load_private_credential_for_boundary(&fixture.username)
            .is_none()
    );
}

#[test]
fn backend_repair_experience_peek_returns_fresh_and_stale_without_service_establishment() {
    for (age, expected) in [(0, "ready"), (20, "stale")] {
        let mut fixture = Fixture::new("experience-fixture-account");
        let (date, _) = fixture.write_overview(age);
        let result = fixture.runtime.peek_overview(date).unwrap().unwrap();
        assert_eq!(result.source, "cache");
        assert_eq!(result.status, expected);
        assert!(!fixture.runtime.service_session_is_proven(ServiceId::Learn));
        assert!(
            !fixture
                .runtime
                .service_session_is_proven(ServiceId::Registrar)
        );
        assert!(fixture.runtime.learn_source.is_none());
    }
}

#[test]
fn backend_repair_experience_peek_rejects_foreign_owner_and_wrong_day() {
    let mut fixture = Fixture::new("experience-fixture-account");
    let (date, path) = fixture.write_overview(0);
    let wrong_date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .unwrap()
        .succ_opt()
        .unwrap();
    assert!(
        fixture
            .runtime
            .peek_overview(wrong_date.to_string())
            .unwrap()
            .is_none()
    );
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    envelope["payload"]["account_scope"] =
        serde_json::Value::String(cache_account_scope("another-account"));
    std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    assert!(fixture.runtime.peek_overview(date).unwrap().is_none());
}

#[test]
fn backend_repair_experience_peek_corruption_is_a_miss_not_empty_success() {
    let mut fixture = Fixture::new("experience-fixture-account");
    let (date, path) = fixture.write_overview(0);
    std::fs::write(path, b"{broken json").unwrap();
    assert!(fixture.runtime.peek_overview(date).unwrap().is_none());
    assert!(fixture.runtime.peek_overview("not-a-date".into()).is_err());
}

#[test]
fn backend_repair_experience_preferences_roundtrip_and_account_isolation() {
    let mut fixture = Fixture::new("experience-fixture-account");
    let defaults = fixture.runtime.load_experience_preferences().unwrap();
    let mut chosen = defaults.clone();
    chosen.compact = true;
    chosen.preload = false;
    chosen.section_order.reverse();
    fixture
        .runtime
        .save_experience_preferences(chosen.clone())
        .unwrap();
    assert_eq!(
        fixture.runtime.load_experience_preferences().unwrap(),
        chosen
    );
    let cache = fixture
        .runtime
        .experience_preferences_cache(&fixture.username);
    let serialized = std::fs::read_to_string(cache.path()).unwrap();
    assert!(!serialized.contains(&fixture.username));
    assert!(!cache.path().to_string_lossy().contains(&fixture.username));
    fixture
        .runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    fixture
        .runtime
        .coordinator
        .mark_authenticated(
            ServiceId::Identity,
            UserIdentity {
                username: "another-fixture-account".into(),
                display_name: None,
            },
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        fixture.runtime.load_experience_preferences().unwrap(),
        defaults
    );
}

#[test]
fn backend_repair_experience_preferences_reject_foreign_payload_and_invalid_write() {
    let fixture = Fixture::new("experience-fixture-account");
    let mut invalid = ExperiencePreferencesDto::default();
    invalid.pinned_services.push("payment".into());
    assert!(
        fixture
            .runtime
            .save_experience_preferences(invalid)
            .is_err()
    );
    fixture
        .runtime
        .experience_preferences_cache(&fixture.username)
        .write(ExperiencePreferencesPayload {
            account_scope: cache_account_scope("foreign-fixture-account"),
            preferences: ExperiencePreferencesDto::default(),
        })
        .unwrap();
    assert!(fixture.runtime.load_experience_preferences().is_err());
}

#[test]
fn backend_repair_experience_signed_out_cannot_load_or_save_preferences() {
    let root = std::env::temp_dir().join(format!("thyou-experience-signedout-{}", Uuid::new_v4()));
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".into(),
        false,
        root.join("cache.json").to_string_lossy().into(),
        false,
    )
    .unwrap();
    assert!(runtime.load_experience_preferences().is_err());
    assert!(
        runtime
            .save_experience_preferences(ExperiencePreferencesDto::default())
            .is_err()
    );
    assert!(
        runtime
            .peek_overview("2026-09-21".into())
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(root);
}
