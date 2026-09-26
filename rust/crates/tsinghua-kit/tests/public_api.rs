use std::{fs, path::PathBuf};

use tsinghua_kit::{
    Error, ErrorCode, Result, SelfServiceClient, Service,
    auth::{AccountAuthState, AuthDomain},
    calendar::{
        AcademicTerm, LearnTermCalendar, SchoolCalendarImage, SchoolCalendarLanguage,
        SchoolCalendarQuery, SchoolCalendarSemester,
    },
    campus_card::{
        CampusCardAccount, CampusCardInteraction, CampusCardPasswordRequest, CampusCardTransaction,
        CampusCardTransactionRange, CampusCardTransactionType, CampusCardTransactions,
    },
    classrooms::{
        ClassroomAvailability, ClassroomBuildings, ClassroomSlotStatus, ClassroomWeek,
        ClassroomWeekSelection,
    },
    electricity::{ElectricityPaymentHistory, ElectricityPaymentRecord, ElectricityRemainder},
    learn::{CourseCatalog, CourseRef, HomeworkDetail, HomeworkList, HomeworkRef, HomeworkState},
    library::{
        LibraryAvailability, LibraryDay, LibraryDirectory, LibrarySocketAvailability,
        LibraryTimeWindows,
    },
    network::{
        NetworkAccessMethod, NetworkProfileId, NetworkProfileInput, NetworkProfilePassword,
        NetworkProfileStoragePolicy, NetworkProfileSummary, PortalAddressRegistration,
        PortalConnectionResult, PortalConnectionState, PortalObservation, PreparedNetworkInput,
    },
    news::{
        NewsCatalog, NewsChannelRef, NewsFavorites, NewsPage, NewsQuery, NewsSourceRef,
        NewsSubscription, NewsSubscriptionRef, NewsSubscriptions,
    },
    read::ReadPolicy,
    registrar::{AcademicStage, ExamReport, GradeReport, GradeReportKind, SemesterSchedule},
    self_service::DeviceRef,
    service_hall::{
        PendingTasks, ServiceDirectory, ServiceHallReadPolicy, TaskView, WorkflowTaskList,
        WorkflowTaskRef,
    },
};

fn succeeds() -> Result<()> {
    Ok(())
}

fn accepts_public_types<T>(_: Option<T>) {}

async fn compile_disconnect_device_api(
    client: &mut SelfServiceClient<'_>,
    reference: &DeviceRef,
) -> Result<()> {
    client.disconnect_device(reference).await
}

async fn compile_news_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let catalog = client.news().catalog().await?;
    let _coverage = catalog.data().channel_coverage();
    if let Some(channel) = catalog.data().channels().first() {
        let search = NewsQuery::search(1, "keyword")?.with_search_channel(channel.reference())?;
        let _search = client.news().articles(search, ReadPolicy::Refresh).await?;
    }
    let subscriptions = client.news().subscriptions().await?;
    if let Some(rule) = subscriptions.data().rules().first() {
        let _page = client
            .news()
            .subscription_articles(rule.reference(), 1)
            .await?;
    }
    let favorites = client.news().favorites().await?;
    let _favorite_count = favorites.data().items().len();
    let query = match (
        catalog.data().sources().first(),
        catalog.data().channels().first(),
    ) {
        (Some(source), Some(channel)) => NewsQuery::list(1, 20)?
            .with_source(source.reference())?
            .with_channel(channel.reference())?,
        _ => NewsQuery::list(1, 20)?,
    };
    let page = client
        .news()
        .articles(query, ReadPolicy::PreferFreshCache)
        .await?;
    let _: &NewsPage = page.data();
    if let Some(article) = page.data().items().first() {
        let detail = client
            .news()
            .article(article.reference(), ReadPolicy::CacheOnly)
            .await?;
        let _ = detail.metadata().source();
    }
    Ok(())
}

async fn compile_learn_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let catalog = client.learn().courses().await?;
    let _semester = catalog.data().semester();
    if let Some(course) = catalog.data().courses().first() {
        let announcements = client.learn().announcements(course.reference()).await?;
        let _announcement_count = announcements.data().items().len();
        let assignments = client.learn().homework(course.reference()).await?;
        let _complete = assignments.metadata().coverage();
        if let Some(item) = assignments.data().items().first() {
            let _state: HomeworkState = item.state();
            if item.detail_available() {
                let detail = client.learn().homework_detail(item.reference()).await?;
                let _: &HomeworkDetail = detail.data();
            }
        }
    }
    Ok(())
}

async fn compile_registrar_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut registrar = client.registrar();
    let schedule = registrar.semester_schedule().await?;
    let _stage: AcademicStage = schedule.data().stage();
    if let Some(event) = schedule.data().events().first() {
        let _kind = event.kind();
        let _start = event.starts_at();
    }
    let grades = registrar.grades().await?;
    let _grade_kind: Option<GradeReportKind> = grades.data().kind();
    let exams = registrar.exams().await?;
    let _exam_count = exams.data().exams().len();
    let _coverage = exams.metadata().coverage();
    Ok(())
}

async fn compile_calendar_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut calendar = client.calendar();
    let terms = calendar.learn_terms().await?;
    let _: &LearnTermCalendar = terms.data();
    let query = SchoolCalendarQuery::latest(
        SchoolCalendarSemester::Autumn,
        SchoolCalendarLanguage::English,
    );
    let image = calendar.school_calendar(query).await?;
    let _image_bytes = image.data().bytes();
    Ok(())
}

async fn compile_service_hall_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut service_hall = client.service_hall();
    let directory = service_hall
        .services(ServiceHallReadPolicy::PreferFreshCache)
        .await?;
    let _: &ServiceDirectory = directory.data();

    for view in [
        TaskView::Completed,
        TaskView::Drafts,
        TaskView::Copies,
        TaskView::Phases,
    ] {
        let tasks = service_hall
            .tasks(view, ServiceHallReadPolicy::PreferFreshCache)
            .await?;
        let _: &WorkflowTaskList = tasks.data();
        if let Some(reference) = tasks
            .data()
            .items()
            .iter()
            .find_map(|task| task.phase_reference())
        {
            let details = service_hall
                .phase_details(reference, ServiceHallReadPolicy::CacheOnly)
                .await?;
            let _step_count = details.data().steps().len();
        }
    }
    Ok(())
}

async fn compile_library_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut library = client.library();
    let directory = library.directory().await?;
    let _: &LibraryDirectory = directory.data();
    let Some(library_ref) = directory
        .data()
        .libraries()
        .iter()
        .find_map(|entry| entry.reference())
    else {
        return Ok(());
    };
    let floors = library.floors(library_ref).await?;
    let Some(floor_ref) = floors
        .data()
        .floors()
        .iter()
        .find_map(|floor| floor.reference())
    else {
        return Ok(());
    };
    let sections = library.sections(floor_ref, LibraryDay::Today).await?;
    let Some(section_ref) = sections
        .data()
        .sections()
        .iter()
        .find_map(|section| section.reference())
    else {
        return Ok(());
    };
    let windows = library.time_windows(section_ref).await?;
    let _: &LibraryTimeWindows = windows.data();
    if let Some(window) = windows.data().windows().first() {
        let availability = library.seats(window.reference()).await?;
        let _: &LibraryAvailability = availability.data();
        let sockets = library.sockets(availability.data()).await?;
        let _: &LibrarySocketAvailability = sockets.data();
    }
    Ok(())
}

async fn compile_classrooms_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut classrooms = client.classrooms();
    let buildings = classrooms.buildings().await?;
    let _: &ClassroomBuildings = buildings.data();
    if let Some(building) = buildings
        .data()
        .buildings()
        .iter()
        .find_map(|entry| entry.reference())
    {
        let availability = classrooms
            .availability(building, ClassroomWeekSelection::BuildingDefault)
            .await?;
        let _: &ClassroomAvailability = availability.data();
        if let Some(room) = availability.data().rooms().first() {
            let _: Option<&ClassroomSlotStatus> = room.slots().first();
        }
    }
    Ok(())
}

async fn compile_electricity_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut electricity = client.electricity();
    let remainder = electricity.remainder().await?;
    let _: &ElectricityRemainder = remainder.data();
    let history = electricity.payment_history().await?;
    let _: &ElectricityPaymentHistory = history.data();
    if let Some(record) = history.data().records().first() {
        let _: &ElectricityPaymentRecord = record;
        let _amount = record.amount();
        let _status = record.status();
    }
    Ok(())
}

fn compile_network_profiles_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut network = client.network();
    let mut profiles = network.profiles();
    let input = NetworkProfileInput::new(
        "Local portal profile",
        "portal-user",
        NetworkAccessMethod::Portal,
    )?
    .save_password("one-shot-local-profile-secret")?;
    let profile: NetworkProfileSummary = profiles.save(input)?;
    let prepared: PreparedNetworkInput = profiles.prepare_fill(profile.id())?;
    assert!(profiles.is_current(&prepared));
    let password: NetworkProfilePassword = profiles
        .password_for_fill(&prepared)?
        .expect("explicitly saved password");
    assert_eq!(password.expose_for_form(), "one-shot-local-profile-secret");
    assert!(!format!("{password:?}").contains("one-shot-local-profile-secret"));
    assert!(profiles.delete(profile.id())?);
    Ok(())
}

async fn compile_portal_api(
    client: &mut tsinghua_kit::Client,
    prepared: &PreparedNetworkInput,
) -> Result<()> {
    let result = client
        .network()
        .connect_portal_profile(prepared, None)
        .await?;
    let _: PortalConnectionState = result.state();
    let _: PortalConnectionResult = result;
    let _ = client.network().disconnect_portal().await?;
    Ok(())
}

async fn compile_campus_card_api(client: &mut tsinghua_kit::Client) -> Result<()> {
    let mut card = client.campus_card();
    let account = card.account().await?;
    let _: &CampusCardAccount = account.data();
    let range = CampusCardTransactionRange::new(
        "2026-09-01",
        "2026-09-30",
        CampusCardTransactionType::Any,
    )?;
    let transactions = card.transactions(range).await?;
    let _items: &[CampusCardTransaction] = transactions.data().items();
    let _interaction: Option<CampusCardInteraction> = card.pending_interaction();
    let _ = card
        .submit_password(CampusCardPasswordRequest::new("one-shot"))
        .await;
    Ok(())
}

#[test]
fn rust_consumers_can_import_curated_domain_modules_without_ffi() {
    let profile_id = NetworkProfileId::new();
    assert!(!profile_id.as_str().is_empty());
    assert_ne!(AuthDomain::Identity, AuthDomain::SelfService);
    assert_ne!(
        NetworkAccessMethod::Portal,
        NetworkAccessMethod::SystemWifiEap
    );
    assert_eq!(
        PortalAddressRegistration::Unknown,
        PortalAddressRegistration::Unknown
    );
    accepts_public_types::<PortalObservation>(None);
    accepts_public_types::<PortalConnectionResult>(None);
    assert_eq!(
        format!("{:?}", AccountAuthState::Authenticated),
        "Authenticated"
    );
    assert_eq!(ReadPolicy::Refresh, ReadPolicy::Refresh);
    assert_eq!(Service::Network.as_str(), "network");
    assert_eq!(ErrorCode::Unsupported.as_str(), "unsupported");

    accepts_public_types::<Error>(None);
    accepts_public_types::<PendingTasks>(None);
    accepts_public_types::<WorkflowTaskRef>(None);
    accepts_public_types::<DeviceRef>(None);
    accepts_public_types::<NewsQuery>(None);
    accepts_public_types::<NewsCatalog>(None);
    accepts_public_types::<NewsFavorites>(None);
    accepts_public_types::<NewsSubscription>(None);
    accepts_public_types::<NewsSubscriptions>(None);
    accepts_public_types::<NewsSourceRef>(None);
    accepts_public_types::<NewsChannelRef>(None);
    accepts_public_types::<NewsSubscriptionRef>(None);
    accepts_public_types::<CourseCatalog>(None);
    accepts_public_types::<tsinghua_kit::learn::CourseAnnouncements>(None);
    accepts_public_types::<CourseRef>(None);
    accepts_public_types::<HomeworkList>(None);
    accepts_public_types::<HomeworkRef>(None);
    accepts_public_types::<GradeReport>(None);
    accepts_public_types::<SemesterSchedule>(None);
    accepts_public_types::<ExamReport>(None);
    accepts_public_types::<LearnTermCalendar>(None);
    accepts_public_types::<AcademicTerm>(None);
    accepts_public_types::<SchoolCalendarImage>(None);
    accepts_public_types::<SchoolCalendarQuery>(None);
    accepts_public_types::<ClassroomBuildings>(None);
    accepts_public_types::<ClassroomAvailability>(None);
    accepts_public_types::<ClassroomSlotStatus>(None);
    accepts_public_types::<CampusCardAccount>(None);
    accepts_public_types::<CampusCardTransaction>(None);
    accepts_public_types::<CampusCardTransactionRange>(None);
    accepts_public_types::<CampusCardTransactions>(None);
    accepts_public_types::<CampusCardPasswordRequest>(None);
    accepts_public_types::<ElectricityRemainder>(None);
    accepts_public_types::<ElectricityPaymentRecord>(None);
    accepts_public_types::<ElectricityPaymentHistory>(None);
    let _ = compile_news_api;
    let _ = compile_learn_api;
    let _ = compile_registrar_api;
    let _ = compile_calendar_api;
    let _ = compile_service_hall_api;
    let _ = compile_library_api;
    let _ = compile_classrooms_api;
    let _ = compile_campus_card_api;
    let _ = compile_electricity_api;
    let _ = compile_network_profiles_api;
    let _ = compile_portal_api;
    let _ = compile_disconnect_device_api;
    succeeds().unwrap();
}

#[tokio::test]
async fn library_read_requires_identity_without_hiding_the_failure() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let error = client.library().directory().await.unwrap_err();
    assert_eq!(error.service(), Service::Library);
    assert_eq!(error.code(), ErrorCode::SessionRequired);
}

#[tokio::test]
async fn classroom_read_requires_identity_without_hiding_the_failure() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let error = client.classrooms().buildings().await.unwrap_err();
    assert_eq!(error.service(), Service::Classrooms);
    assert_eq!(error.code(), ErrorCode::SessionRequired);
}

#[tokio::test]
async fn campus_card_reads_and_password_submission_fail_explicitly_without_identity() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let mut card = client.campus_card();

    let account_error = card.account().await.unwrap_err();
    let range =
        CampusCardTransactionRange::new("2026-09-01", "2026-09-07", CampusCardTransactionType::Any)
            .unwrap();
    let transactions_error = card.transactions(range).await.unwrap_err();
    let password_error = card
        .submit_password(CampusCardPasswordRequest::new("private-card-password"))
        .await
        .unwrap_err();

    for error in [account_error, transactions_error] {
        assert_eq!(error.service(), Service::CampusCard);
        assert_eq!(error.code(), ErrorCode::SessionRequired);
    }
    assert_eq!(password_error.service(), Service::CampusCard);
    assert_eq!(password_error.code(), ErrorCode::InteractionRequired);
    assert_eq!(card.pending_interaction(), None);
    assert!(!card.cancel_password_challenge());
    drop(card);
    assert_eq!(
        client.auth().status().identity().state(),
        AccountAuthState::SignedOut
    );
}

#[tokio::test]
async fn electricity_reads_require_identity_and_return_service_scoped_errors() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let (remainder_error, history_error) = {
        let mut electricity = client.electricity();
        (
            electricity.remainder().await.unwrap_err(),
            electricity.payment_history().await.unwrap_err(),
        )
    };

    for error in [remainder_error, history_error] {
        assert_eq!(error.service(), Service::Electricity);
        assert_eq!(error.code(), ErrorCode::SessionRequired);
    }
    assert_eq!(
        client.auth().status().identity().state(),
        AccountAuthState::SignedOut
    );
}

#[test]
fn network_profile_storage_and_form_preparation_are_local_and_redacted() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let rendered = {
        let mut network = client.network();
        let mut profiles = network.profiles();
        let summary = profiles
            .save(
                NetworkProfileInput::new(
                    "Private profile label",
                    "private-profile-user",
                    NetworkAccessMethod::SystemWifiEap,
                )
                .unwrap()
                .save_password("private-profile-password")
                .unwrap(),
            )
            .unwrap();
        let prepared = profiles.prepare_fill(summary.id()).unwrap();
        let password = profiles
            .password_for_fill(&prepared)
            .unwrap()
            .expect("explicitly saved password");

        assert_eq!(summary.method(), NetworkAccessMethod::SystemWifiEap);
        assert!(summary.has_saved_password());
        assert_eq!(prepared.summary().username(), "private-profile-user");
        assert_eq!(password.expose_for_form(), "private-profile-password");
        assert!(profiles.is_current(&prepared));
        format!("{summary:?} {prepared:?} {password:?}")
    };

    assert!(!rendered.contains("Private profile label"));
    assert!(!rendered.contains("private-profile-user"));
    assert!(!rendered.contains("private-profile-password"));
    assert_eq!(
        client.auth().status().identity().state(),
        AccountAuthState::SignedOut
    );
    assert_eq!(
        client.auth().status().self_service().state(),
        AccountAuthState::SignedOut
    );
}

#[tokio::test]
async fn portal_connect_is_explicit_and_rejects_system_eap_profiles() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let (portal_id, portal, eap) = {
        let mut network = client.network();
        let mut profiles = network.profiles();
        let portal = profiles
            .save(
                NetworkProfileInput::new("Portal", "portal-user", NetworkAccessMethod::Portal)
                    .unwrap(),
            )
            .unwrap();
        let portal_id = portal.id();
        let eap = profiles
            .save(
                NetworkProfileInput::new(
                    "Tsinghua Secure",
                    "eap-user",
                    NetworkAccessMethod::SystemWifiEap,
                )
                .unwrap()
                .save_password("eap-only-secret")
                .unwrap(),
            )
            .unwrap();
        (
            portal_id,
            profiles.prepare_fill(portal_id).unwrap(),
            profiles.prepare_fill(eap.id()).unwrap(),
        )
    };

    let missing_password = client
        .network()
        .connect_portal_profile(&portal, None)
        .await
        .unwrap_err();
    assert_eq!(missing_password.service(), Service::Network);
    assert_eq!(missing_password.code(), ErrorCode::InteractionRequired);

    let wrong_method = client
        .network()
        .connect_portal_profile(&eap, Some("eap-only-secret".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(wrong_method.service(), Service::Network);
    assert_eq!(wrong_method.code(), ErrorCode::Unsupported);

    let mut other_client = tsinghua_kit::Client::builder().build().unwrap();
    let cross_client_reference = other_client
        .network()
        .connect_portal_profile(&portal, Some("portal-only-secret".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(cross_client_reference.service(), Service::Network);
    assert_eq!(cross_client_reference.code(), ErrorCode::ContextMismatch);

    {
        let mut network = client.network();
        let mut profiles = network.profiles();
        profiles
            .update(
                portal_id,
                NetworkProfileInput::new(
                    "Edited Portal",
                    "portal-user",
                    NetworkAccessMethod::Portal,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let stale_reference = client
        .network()
        .connect_portal_profile(&portal, Some("portal-only-secret".to_owned()))
        .await
        .unwrap_err();
    assert_eq!(stale_reference.service(), Service::Network);
    assert_eq!(stale_reference.code(), ErrorCode::ContextMismatch);

    let status = client.auth().status();
    assert_eq!(status.identity().state(), AccountAuthState::SignedOut);
    assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);
}

#[test]
fn network_profile_opt_in_storage_survives_client_rebuild_without_auth_state() {
    struct ScratchDirectory(PathBuf);
    impl ScratchDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tsinghua-kit-public-profile-{}",
                NetworkProfileId::new().as_str()
            ));
            Self(path)
        }
    }
    impl Drop for ScratchDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    let scratch = ScratchDirectory::new();
    let policy = NetworkProfileStoragePolicy::encrypted_directory(
        &scratch.0,
        "org.example.tsinghua-kit-test",
    )
    .unwrap();

    let profile_id = {
        let mut client = tsinghua_kit::Client::builder()
            .network_profile_storage(policy.clone())
            .build()
            .unwrap();
        let summary = client
            .network()
            .profiles()
            .save(
                NetworkProfileInput::new(
                    "Persistent local profile",
                    "persistent-profile-user",
                    NetworkAccessMethod::Portal,
                )
                .unwrap()
                .save_password("synthetic-persistent-profile-password")
                .unwrap(),
            )
            .unwrap();
        summary.id()
    };

    let mut reopened = tsinghua_kit::Client::builder()
        .network_profile_storage(policy.clone())
        .build()
        .unwrap();
    let summary = reopened.network().profiles().list().remove(0);
    assert_eq!(summary.id(), profile_id);
    assert_eq!(summary.username(), "persistent-profile-user");
    assert_eq!(summary.method(), NetworkAccessMethod::Portal);
    assert!(summary.has_saved_password());
    let password = {
        let mut network = reopened.network();
        let profiles = network.profiles();
        let prepared = profiles.prepare_fill(profile_id).unwrap();
        profiles
            .password_for_fill(&prepared)
            .unwrap()
            .expect("explicitly saved password")
    };
    assert_eq!(
        password.expose_for_form(),
        "synthetic-persistent-profile-password"
    );
    assert_eq!(
        reopened.auth().status().identity().state(),
        AccountAuthState::SignedOut
    );
    assert_eq!(
        reopened.auth().status().self_service().state(),
        AccountAuthState::SignedOut
    );
    drop(password);
    drop(reopened);

    let mut deleter = tsinghua_kit::Client::builder()
        .network_profile_storage(policy.clone())
        .build()
        .unwrap();
    assert!(deleter.network().profiles().delete(profile_id).unwrap());
    drop(deleter);
    let mut after_delete = tsinghua_kit::Client::builder()
        .network_profile_storage(policy)
        .build()
        .unwrap();
    assert!(after_delete.network().profiles().list().is_empty());
}

#[test]
fn memory_only_network_profiles_do_not_survive_client_drop() {
    {
        let mut client = tsinghua_kit::Client::builder().build().unwrap();
        client
            .network()
            .profiles()
            .save(
                NetworkProfileInput::new(
                    "Temporary local profile",
                    "temporary-profile-user",
                    NetworkAccessMethod::Portal,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let mut reopened = tsinghua_kit::Client::builder().build().unwrap();
    assert!(reopened.network().profiles().list().is_empty());
}

#[test]
fn campus_card_password_request_debug_redacts_the_secret() {
    let request = CampusCardPasswordRequest::new("private-card-password");
    let rendered = format!("{request:?}");

    assert!(rendered.contains("password_present: true"));
    assert!(!rendered.contains("private-card-password"));
}

#[tokio::test]
async fn service_hall_cache_reads_fail_explicitly_without_identity_session() {
    let mut client = tsinghua_kit::Client::builder().build().unwrap();
    let mut service_hall = client.service_hall();

    let directory_error = service_hall
        .services(ServiceHallReadPolicy::CacheOnly)
        .await
        .unwrap_err();
    let tasks_error = service_hall
        .tasks(TaskView::Phases, ServiceHallReadPolicy::CacheOnly)
        .await
        .unwrap_err();

    for error in [directory_error, tasks_error] {
        assert_eq!(error.service(), Service::ServiceHall);
        assert_eq!(error.code(), ErrorCode::SessionRequired);
    }
}

#[test]
fn news_query_validation_is_available_without_exposing_protocol_values() {
    assert!(NewsQuery::list(0, 10).is_err());
    assert!(NewsQuery::list(1, 101).is_err());
    let search = NewsQuery::search(1, "TsinghuaKit").unwrap();
    let debug = format!("{search:?}");
    assert!(!debug.contains("TsinghuaKit"));
    let _ = search;
}

#[test]
fn school_calendar_selection_is_typed_and_validated() {
    let error = SchoolCalendarQuery::for_year(
        1999,
        SchoolCalendarSemester::Autumn,
        SchoolCalendarLanguage::Chinese,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput);

    let query = SchoolCalendarQuery::latest(
        SchoolCalendarSemester::Spring,
        SchoolCalendarLanguage::English,
    );
    assert_eq!(query.year(), None);
    assert_eq!(query.semester(), SchoolCalendarSemester::Spring);
    assert_eq!(query.language(), SchoolCalendarLanguage::English);
}

#[test]
fn classroom_week_selection_is_bounded_and_typed() {
    assert!(ClassroomWeek::new(0).is_none());
    assert!(ClassroomWeek::new(101).is_none());
    let week = ClassroomWeek::new(12).unwrap();
    assert_eq!(week.get(), 12);
    let _selection = ClassroomWeekSelection::Week(week);
}
