use std::path::Path;
use tsinghua_kit::{
    Client, ClientCachePolicy, ErrorCode, SelfServiceLoginRequest, Service,
    auth::{
        AccountAuthState, AuthDomain, IdentityLoginRequest, IdentitySessionStoragePolicy,
        SecondFactorMethod,
    },
    learn::{CourseDiscussion, CourseFile, CourseFileCategory, CourseFileRef, CourseRef},
    service_hall::PendingReadPolicy,
};

#[allow(dead_code)]
async fn public_learn_file_surface(
    client: &mut Client,
    course: &CourseRef,
    file: &CourseFileRef,
    destination: &Path,
) -> tsinghua_kit::Result<()> {
    let files = client.learn().files(course).await?;
    let _: &[CourseFile] = files.data().items();
    let _: &[CourseFileCategory] = client.learn().file_categories(course).await?.data().items();
    let _: &[CourseDiscussion] = client.learn().discussions(course).await?.data().items();
    let _receipt = client.learn().save_file(file, destination).await?;
    Ok(())
}

#[test]
fn learn_file_and_discussion_methods_are_available_from_the_public_crate() {
    let _ = public_learn_file_surface;
}

#[test]
fn client_owns_two_signed_out_account_domains_without_logging_in() {
    let mut client = Client::builder().build().unwrap();
    let status = client.auth().status();

    assert_eq!(status.identity().state(), AccountAuthState::SignedOut);
    assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);
    assert_ne!(status.identity().domain(), status.self_service().domain());
    assert_eq!(SecondFactorMethod::Sms.as_str(), "sms");
    assert!(client.auth().identity().interaction().unwrap().is_none());
}

#[test]
fn opening_network_facade_does_not_create_a_third_auth_account_or_dispatch() {
    let mut client = Client::builder().build().unwrap();
    {
        let _network = client.network();
    }

    let status = client.auth().status();
    assert_eq!(status.identity().state(), AccountAuthState::SignedOut);
    assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);
}

#[tokio::test]
async fn cache_only_service_hall_read_fails_explicitly_without_a_session() {
    let mut client = Client::builder().build().unwrap();

    let error = client
        .service_hall()
        .pending(PendingReadPolicy::CacheOnly)
        .await
        .unwrap_err();

    assert_eq!(error.service(), Service::ServiceHall);
    assert_eq!(error.code(), ErrorCode::SessionRequired);
}

#[tokio::test]
async fn self_service_login_requires_identity_access_and_does_not_create_a_third_slot() {
    let mut client = Client::builder().build().unwrap();

    let error = client
        .auth()
        .self_service()
        .start_login(SelfServiceLoginRequest::new(
            "different-account",
            "private-password",
        ))
        .await
        .unwrap_err();

    assert_eq!(
        error.service(),
        Service::Auth(tsinghua_kit::auth::AuthDomain::Identity)
    );
    assert_eq!(error.code(), ErrorCode::SessionRequired);
    assert_eq!(
        client.auth().status().self_service().state(),
        AccountAuthState::SignedOut
    );
}

#[tokio::test]
async fn identity_session_persistence_is_explicit_and_fresh_revalidation_is_local() {
    let root = std::env::temp_dir().join(format!(
        "tsinghua-kit-public-auth-store-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut client = Client::builder()
        .cache_policy(ClientCachePolicy::Directory(root.clone()))
        .identity_session_storage(IdentitySessionStoragePolicy::EncryptedDirectory(
            root.clone(),
        ))
        .build()
        .unwrap();

    let status = client.auth().status();
    assert_eq!(status.identity().state(), AccountAuthState::SignedOut);
    assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);
    let status = client
        .auth()
        .identity()
        .revalidate_restored_session()
        .await
        .unwrap();
    assert_eq!(status.identity().state(), AccountAuthState::SignedOut);
    assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);

    drop(client);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn self_service_readers_require_the_second_account_before_dispatch() {
    let mut client = Client::builder().build().unwrap();
    let mut self_service = client.self_service();

    let account_error = self_service.account().await.unwrap_err();
    let devices_error = self_service.online_devices().await.unwrap_err();
    let usage_error = self_service.usage().await.unwrap_err();

    for error in [account_error, devices_error, usage_error] {
        assert_eq!(error.service(), Service::Auth(AuthDomain::SelfService));
        assert_eq!(error.code(), ErrorCode::SessionRequired);
    }
    assert_eq!(
        client.auth().status().self_service().state(),
        AccountAuthState::SignedOut
    );
}

#[tokio::test]
async fn invalid_identity_input_is_rejected_before_dispatch() {
    let mut client = Client::builder().build().unwrap();

    let error = client
        .auth()
        .identity()
        .login(IdentityLoginRequest::new("  ", ""))
        .await
        .unwrap_err();

    assert_eq!(
        error.service(),
        Service::Auth(tsinghua_kit::auth::AuthDomain::Identity)
    );
    assert_eq!(error.code(), ErrorCode::InvalidInput);
}

#[test]
fn self_service_login_request_debug_redacts_both_account_and_password() {
    let request = SelfServiceLoginRequest::new("private-account", "private-password");
    let debug = format!("{request:?}");

    assert!(!debug.contains("private-account"));
    assert!(!debug.contains("private-password"));
    assert!(debug.contains("password_present: true"));
}

#[test]
fn identity_login_request_debug_redacts_the_password() {
    let request = IdentityLoginRequest::new("account-name", "sensitive-password");
    let debug = format!("{request:?}");

    assert!(!debug.contains("account-name"));
    assert!(!debug.contains("sensitive-password"));
    assert!(debug.contains("password_present: true"));
}
