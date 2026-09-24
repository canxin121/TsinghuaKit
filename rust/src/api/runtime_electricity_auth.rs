// The reference dorm read is wrapped in target-specific Identity roam.
// The first business GET is reused on success. Only explicit authentication
// failure enters that roam; network and parse failures never submit a password.
use super::*;
use crate::dorm_electricity_read::ELECTRICITY_WEBVPN_TARGET;
use crate::identity_execution::IdentityHttpResponse;
use std::collections::HashSet;

#[derive(Clone)]
pub(super) struct ElectricityFlow {
    pub vpn: WebVpnIdentityConfig,
    pub mapped: Url,
}
impl ElectricityFlow {
    pub fn current() -> Self {
        Self {
            vpn: WebVpnIdentityConfig::current(),
            mapped: Url::parse(ELECTRICITY_WEBVPN_BASE_URL).expect("static electricity mapping"),
        }
    }
    fn direct(&self) -> Url {
        Url::parse("http://myhome.tsinghua.edu.cn/").expect("static electricity origin")
    }
    fn mapped_allowed(&self, url: &Url) -> bool {
        if !same_origin(url, self.vpn.webvpn_origin()) || !clean(url) {
            return false;
        }
        let mut configured = self.mapped.path().trim_matches('/').split('/');
        let (Some(_), Some(mapping), None) =
            (configured.next(), configured.next(), configured.next())
        else {
            return false;
        };
        let Some((protocol, _)) = url.path().trim_start_matches('/').split_once('/') else {
            return false;
        };
        // The encrypted host is still exactly myhome. Accept only standard
        // HTTP/HTTPS protocols, never another host or non-default port.
        matches!(protocol, "http" | "https" | "http-80" | "https-443")
            && crate::webvpn_url::redirect_path_stays_in_mapping(url, protocol, mapping)
    }

    fn direct_allowed(&self, url: &Url) -> bool {
        // Reference getWebVPNUrl retains the server-selected scheme and its
        // default port. HTTPS on the exact same electricity host is not a
        // different account system; arbitrary ports/hosts remain forbidden.
        let secure =
            Url::parse("https://myhome.tsinghua.edu.cn/").expect("static electricity origin");
        (same_origin(url, &self.direct()) || same_origin(url, &secure)) && clean(url)
    }
    fn broker_allowed(&self, url: &Url) -> bool {
        if !same_origin(url, self.vpn.oauth_origin()) || !clean(url) {
            return false;
        }
        if url.path().starts_with("/thu-oauth/") {
            return true;
        }
        if url.path() != "/lb-auth/lbredirect" {
            return false;
        }
        let query = url.query().unwrap_or_default();
        for prefix in [
            "scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=",
            "scheme=https&host=myhome.tsinghua.edu.cn&port=443&uri=",
        ] {
            if let Some(uri) = query
                .strip_prefix(prefix)
                .filter(|uri| uri.starts_with('/'))
            {
                // A raw second `uri` is ambiguous at the outer broker level.
                // Only the encoded four-field form may carry an inner URI.
                // Count at the OUTER level even when the nested path has no
                // '?': `/path&host=...` is a path to Url::join but a second
                // routing field to the broker. Query decoding also catches
                // percent-encoded/case-varied duplicate parameter names.
                let routing_fields = url
                    .query_pairs()
                    .filter(|(key, _)| {
                        matches!(
                            key.to_ascii_lowercase().as_str(),
                            "scheme" | "host" | "port" | "uri"
                        )
                    })
                    .count();
                return routing_fields == 4
                    && self.uri_allowed(uri)
                    && self.direct().join(uri).is_ok_and(|url| {
                        !url.query_pairs()
                            .any(|(key, _)| key.eq_ignore_ascii_case("uri"))
                    });
            }
        }
        let mut scheme = None;
        let mut host = None;
        let mut port = None;
        let mut uri = None;
        for (key, value) in url.query_pairs() {
            let slot = match key.as_ref() {
                "scheme" => &mut scheme,
                "host" => &mut host,
                "port" => &mut port,
                "uri" => &mut uri,
                _ => return false,
            };
            if slot.replace(value.into_owned()).is_some() {
                return false;
            }
        }
        matches!((scheme.as_deref(),host.as_deref(),port.as_deref(),uri.as_deref()),
            (Some("http"),Some("myhome.tsinghua.edu.cn"),Some("80"),Some(uri))
            |(Some("https"),Some("myhome.tsinghua.edu.cn"),Some("443"),Some(uri)) if self.uri_allowed(uri))
    }
    fn uri_allowed(&self, uri: &str) -> bool {
        if !uri.starts_with('/') || uri.starts_with("//") {
            return false;
        }
        let Ok(url) = self.direct().join(uri) else {
            return false;
        };
        self.direct_allowed(&url) && self.inner_routing_fields_allowed(&url)
    }

    fn inner_routing_fields_allowed(&self, url: &Url) -> bool {
        // These fields belong to the target query, not the outer broker.
        // Permit only same-service routing hints and serialize the entire
        // query inside one encoded URI. Contradictory/duplicate hints remain
        // rejected; an inner `host` never chooses the network destination.
        let mut scheme = None;
        let mut host = None;
        let mut port = None;
        for (key, value) in url.query_pairs() {
            let slot = match key.to_ascii_lowercase().as_str() {
                "scheme" => &mut scheme,
                "host" => &mut host,
                "port" => &mut port,
                _ => continue,
            };
            if slot.replace(value.into_owned()).is_some() {
                return false;
            }
        }
        host.as_deref()
            .is_none_or(|host| host == "myhome.tsinghua.edu.cn")
            && scheme
                .as_deref()
                .is_none_or(|scheme| matches!(scheme, "http" | "https"))
            && port
                .as_deref()
                .is_none_or(|port| matches!(port, "80" | "443"))
            && !matches!(
                (scheme.as_deref(), port.as_deref()),
                (Some("http"), Some("443")) | (Some("https"), Some("80"))
            )
    }

    fn map_absolute_path(&self, target: &Url) -> Result<Url, &'static str> {
        // Reference parseUrl preserves the path after the validated host.
        // An absolute URL may legitimately have // in its PATH. It must not
        // be passed to Url::join as a network-path reference or raw broker
        // URI. Prefix it with the fixed cipher-host mapping instead.
        let mut parts = self.mapped.path().trim_matches('/').split('/');
        let (Some(_), Some(mapping), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err("electricity_handoff_config");
        };
        let mut mapped = self.vpn.webvpn_origin().clone();
        mapped.set_path(&format!("/{}/{mapping}{}", target.scheme(), target.path()));
        mapped.set_query(target.query());
        if !self.mapped_allowed(&mapped) {
            return Err("electricity_callback_path_rejected");
        }
        Ok(mapped)
    }
    pub(super) fn to_broker(&self, target: &Url) -> Result<Url, &'static str> {
        if self.mapped_allowed(target) || self.broker_allowed(target) {
            return Ok(target.clone());
        }
        if !self.direct_allowed(target) {
            return Err("electricity_callback_origin_rejected");
        }
        if !self.inner_routing_fields_allowed(target) {
            return Err("electricity_callback_query_rejected");
        }
        if target.path().starts_with("//") {
            let mapped = self.map_absolute_path(target)?;
            tracing::debug!(target:"tsinghua_kit::auth",event="electricity_target_evidence",service="electricity",reason="electricity_absolute_path_mapped");
            return Ok(mapped);
        }
        let mut uri = target.path().to_owned();
        if let Some(query) = target.query() {
            uri.push('?');
            uri.push_str(query);
        }
        if !self.uri_allowed(&uri) {
            return Err("electricity_callback_uri_rejected");
        }
        let mut broker = self
            .vpn
            .oauth_origin()
            .join("/lb-auth/lbredirect")
            .map_err(|_| "electricity_handoff_config")?;
        // Keep the reference's wire format for a simple opaque ticket. A
        // compound URI (notably an inner `uri` parameter) must be encoded as
        // ONE outer field, otherwise it collides with the broker's routing
        // fields. Decoding that field once preserves inner %2B/%3D bytes.
        let port = target
            .port_or_known_default()
            .expect("known electricity scheme")
            .to_string();
        if target.query().is_some_and(|query| {
            query.contains('&')
                || target.query_pairs().any(|(key, _)| {
                    matches!(
                        key.to_ascii_lowercase().as_str(),
                        "uri" | "scheme" | "host" | "port"
                    )
                })
        }) {
            broker
                .query_pairs_mut()
                .append_pair("scheme", target.scheme())
                .append_pair("host", "myhome.tsinghua.edu.cn")
                .append_pair("port", &port)
                .append_pair("uri", &uri);
        } else {
            broker.set_query(Some(&format!(
                "scheme={}&host=myhome.tsinghua.edu.cn&port={port}&uri={uri}",
                target.scheme()
            )));
        }
        if !self.broker_allowed(&broker) {
            return Err("electricity_broker_encoding");
        }
        Ok(broker)
    }
}
fn clean(url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    let lower = url.path().to_ascii_lowercase();
    if lower.contains("%2e") || lower.contains("%25") || lower.contains("%5c") {
        return false;
    }
    let bytes = url.as_str().as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return false;
            }
            let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            ) else {
                return false;
            };
            index += 3;
            ((high << 4) | low) as u8
        } else {
            let b = bytes[index];
            index += 1;
            b
        };
        if byte.is_ascii_control() || byte == b'\\' {
            return false;
        }
    }
    true
}

pub(super) fn selected_target(
    client: &IdentityClient,
    response: &IdentityHttpResponse,
    flow: &ElectricityFlow,
) -> Option<Url> {
    selected_target_checked(client, response, flow).ok()
}

pub(super) fn selected_target_checked(
    client: &IdentityClient,
    response: &IdentityHttpResponse,
    flow: &ElectricityFlow,
) -> Result<Url, &'static str> {
    if !same_origin(&response.final_url, flow.vpn.identity_origin()) {
        tracing::warn!(target:"tsinghua_kit::auth",event="electricity_target_evidence",service="electricity",reason="electricity_callback_origin",http_status=response.status.as_u16());
        return Err("electricity_callback_origin");
    }
    // Selection and dispatch use the SAME predicate. Previously a direct
    // callback could be declared selected and then rejected locally without
    // ever dispatching the broker, concealing which boundary failed.
    let rejected = std::cell::Cell::new(None);
    let accepted = |url: &Url| match flow.to_broker(url) {
        Ok(_) => true,
        Err(reason) => {
            rejected.set(Some(reason));
            tracing::warn!(target:"tsinghua_kit::auth",event="electricity_target_evidence",service="electricity",route_decision="reject_route",reason);
            false
        }
    };
    if matches!(response.status.as_u16(), 301 | 302 | 303 | 307 | 308)
        && let Some(location) = response
            .redirect_location
            .as_ref()
            .filter(|url| accepted(url))
    {
        return Ok(location.clone());
    }
    let evidence = client.parse_login_page(response.body());
    if !response.status.is_success() || !client.has_identity_success_marker(response.body()) {
        tracing::warn!(target:"tsinghua_kit::auth",event="electricity_target_evidence",service="electricity",
            reason=if response.status.is_success(){"electricity_success_marker_missing"}else{"electricity_callback_http"},
            login_form_present=evidence.has_login_form,factor_present=evidence.second_factor_marker.is_some(),http_status=response.status.as_u16());
        return Err(if response.status.is_success() {
            "electricity_success_marker_missing"
        } else {
            "electricity_callback_http"
        });
    }
    if evidence.second_factor_marker.is_some()
        || evidence.failure.as_ref().is_some_and(|failure| {
            failure.reason != crate::identity_client::LoginFailureReason::Generic
        })
    {
        tracing::warn!(target:"tsinghua_kit::auth",event="electricity_target_evidence",service="electricity",reason="electricity_callback_rejected",login_form_present=evidence.has_login_form,factor_present=evidence.second_factor_marker.is_some());
        return Err("electricity_callback_rejected");
    }
    let target = [
        flow.direct(),
        Url::parse("https://myhome.tsinghua.edu.cn/").expect("static electricity origin"),
        flow.vpn.oauth_origin().clone(),
        flow.vpn.webvpn_origin().clone(),
    ]
    .iter()
    .find_map(|origin| {
        client
            .first_static_redirect_url_for_origin(&response.final_url, response.body(), origin)
            .filter(|url| accepted(url))
    });
    tracing::debug!(target:"tsinghua_kit::auth",event="electricity_target_evidence",service="electricity",
        redirect_present=target.is_some(),login_form_present=evidence.has_login_form,
        reason=if target.is_some(){"electricity_target_selected"}else{"electricity_target_missing"});
    target.ok_or(rejected.get().unwrap_or("electricity_target_missing"))
}

impl CampusRuntime {
    pub(super) fn configured_electricity_flow(&self) -> Result<ElectricityFlow, String> {
        let path = Url::parse(ELECTRICITY_WEBVPN_BASE_URL)
            .map_err(|_| "electricity_config".to_owned())?.path().to_owned();
        let vpn = self.webvpn_identity_config.clone();
        let mapped = vpn.webvpn_origin().join(&path)
            .map_err(|_| "electricity_config".to_owned())?;
        Ok(ElectricityFlow { vpn, mapped })
    }

    pub(super) async fn ensure_electricity_with_flow(
        &mut self,
        user: &UserIdentity,
        flow: &ElectricityFlow,
    ) -> Result<(), String> {
        if self.electricity_service_is_proven() {
            return Ok(());
        }
        self.invalidate_electricity_session();
        self.ensure_info_session(user).await?;
        let adapter = DormElectricityAdapter::try_with_transport(
            flow.mapped.clone(),
            self.identity.transport().clone(),
        )
        .map_err(|_| {
            self.record_business_failure("electricity", "electricity_read", "electricity_config")
        })?;
        match adapter.read_remainder_with_proof().await {
            Ok(read) => {
                self.electricity_proof = Some(read.proof);
                self.electricity_remainder_prefetch = Some(read.value);
                self.electricity_adapter = Some(adapter);
                return Ok(());
            }
            Err(error) if error.is_session_expired() => {}
            Err(error) => {
                return Err(self.record_business_failure(
                    "electricity",
                    "electricity_read",
                    error.diagnostic_code(),
                ));
            }
        }
        self.begin_electricity_identity(user, flow).await
    }

    pub(super) async fn begin_electricity_identity(
        &mut self,
        user: &UserIdentity,
        flow: &ElectricityFlow,
    ) -> Result<(), String> {
        self.allow_new_identity_handoff()?;
        if !self.recovery.target_handoffs.insert(ServiceSecondFactorTarget::Electricity) {
            return Err(self.record_error("服务会话自动续接未完成，请重新建立服务会话"));
        }
        let path = format!("/do/off/ui/auth/login/form/{ELECTRICITY_WEBVPN_TARGET}");
        let mut page = self
            .identity
            .identity()
            .fetch_login_page_at_path(&path)
            .await
            .map_err(|_| {
                self.record_business_failure(
                    "electricity",
                    "electricity_handoff",
                    "electricity_identity_network",
                )
            })?;
        let client = self.identity.identity().client().clone();
        let mut continued = false;
        if selected_target(&client, &page, flow).is_none() {
            if let Some(next) = self
                .identity
                .identity()
                .continue_service_check_single(&page, &self.fingerprint)
                .await
                .map_err(|_| {
                    self.record_business_failure(
                        "electricity",
                        "electricity_handoff",
                        "electricity_identity_network",
                    )
                })?
            {
                page = next;
                continued = true;
            }
        }
        if let Some(target) = selected_target(&client, &page, flow) {
            return self.finish_electricity_with_flow(user, target, flow).await;
        }
        if !page.status.is_success() {
            return Err(self.record_business_failure(
                "electricity",
                "electricity_handoff",
                "electricity_identity_http",
            ));
        }
        let evidence = client.parse_login_page(page.body());
        if evidence.second_factor_marker.is_some() {
            return self
                .pause_for_service_second_factor(ServiceSecondFactorTarget::Electricity)
                .await;
        }
        if continued {
            return Err(self.record_business_failure(
                "electricity",
                "electricity_handoff",
                "electricity_trusted_unconfirmed",
            ));
        }
        if !evidence.has_login_form
            || evidence.failure.as_ref().is_some_and(|failure| failure.reason != crate::identity_client::LoginFailureReason::Generic)
        {
            return Err(self.record_error("服务会话自动续接未完成，请重新建立服务会话"));
        }
        let key = evidence.sm2_public_key.ok_or_else(|| {
            self.record_business_failure(
                "electricity",
                "electricity_handoff",
                "electricity_public_key_missing",
            )
        })?;
        let password = match self.primary_password.clone() {
            Some(password) => password,
            None => self.load_target_credential(user, ServiceSecondFactorTarget::Electricity)?,
        };
        let wire = encrypt_password(&password, &key.value).map_err(|_| {
            self.record_business_failure(
                "electricity",
                "electricity_handoff",
                "electricity_public_key_invalid",
            )
        })?;
        let input = portal_roam_login_input(&user.username, &wire, &self.fingerprint);
        let response = self
            .identity
            .identity()
            .submit_login_for_app_id(
                ELECTRICITY_WEBVPN_TARGET
                    .split('/')
                    .next()
                    .unwrap_or_default(),
                input,
            )
            .await
            .map_err(|_| {
                self.record_business_failure(
                    "electricity",
                    "electricity_handoff",
                    "electricity_identity_network",
                )
            })?;
        let evidence = client.parse_login_page(response.body());
        if evidence.second_factor_marker.is_some() {
            return self
                .pause_for_service_second_factor(ServiceSecondFactorTarget::Electricity)
                .await;
        }
        if let Some(error) = evidence.failure.as_ref() {
            error.trace_safe_diagnostic();
            if error.reason == crate::identity_client::LoginFailureReason::InvalidCredentials {
                self.record_identity_session_error(crate::identity_session::IdentitySessionError::LoginFailed {
                    reason: crate::identity_client::LoginFailureReason::InvalidCredentials,
                });
                return Err(self.require_identity_login("统一认证凭证已失效，请重新登录"));
            }
        }
        let target = selected_target_checked(&client, &response, flow).map_err(|reason| {
            self.record_business_failure("electricity", "electricity_handoff", reason)
        })?;
        self.finish_electricity_with_flow(user, target, flow).await
    }

    pub(super) async fn finish_electricity_with_flow(
        &mut self,
        user: &UserIdentity,
        target: Url,
        flow: &ElectricityFlow,
    ) -> Result<(), String> {
        if !self.service_session_is_proven(ServiceId::Identity)
            || self
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .user
                .as_ref()
                != Some(user)
        {
            return Err(self.record_business_failure(
                "electricity",
                "electricity_handoff",
                "electricity_identity_binding",
            ));
        }
        navigate(self.identity.transport(), flow, target)
            .await
            .map_err(|reason| {
                self.record_business_failure("electricity", "electricity_handoff", reason)
            })?;
        // Navigation/HTTP 200 is not the electricity proof. The established
        // INFO/Identity account and an actual parsed remainder are both needed.
        self.establish_electricity_read_at(flow.mapped.clone()).await?;
        self.recovery.target_handoffs.remove(&ServiceSecondFactorTarget::Electricity);
        self.recovery.target_credentials.remove(&ServiceSecondFactorTarget::Electricity);
        Ok(())
    }
}

async fn navigate(
    transport: &crate::transport::CampusHttpTransport,
    flow: &ElectricityFlow,
    target: Url,
) -> Result<(), &'static str> {
    let mut next = flow.to_broker(&target)?;
    let mut visited = HashSet::new();
    for _ in 0..10 {
        let at_gateway =
            same_origin(&next, flow.vpn.webvpn_origin()) && matches!(next.path(), "/" | "/login");
        if !clean(&next)
            || !(flow.mapped_allowed(&next)
                || flow.direct_allowed(&next)
                || flow.broker_allowed(&next)
                || at_gateway)
        {
            return Err("electricity_redirect_target_rejected");
        }
        if !visited.insert(next.as_str().to_owned()) {
            return Err("electricity_handoff_cycle");
        }
        let request = transport
            .client()
            .get(next.clone())
            .build()
            .map_err(|_| "electricity_handoff_config")?;
        let mut response = transport
            .execute_once(transport.client(), request)
            .await
            .map_err(|_| "electricity_handoff_network")?;
        let status = response.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or("electricity_handoff_location")?;
            next = response
                .url()
                .join(location)
                .map_err(|_| "electricity_handoff_location")?;
            continue;
        }
        if !matches!(status.as_u16(), 200 | 201) {
            return Err("electricity_handoff_http");
        }
        let mut size = 0usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "electricity_handoff_network")?
        {
            size = size.saturating_add(chunk.len());
            if size > 4 * 1024 * 1024 {
                return Err("electricity_handoff_body_limit");
            }
        }
        if flow.mapped_allowed(&next) || flow.direct_allowed(&next) || at_gateway {
            return Ok(());
        }
        return Err("electricity_handoff_unconfirmed");
    }
    Err("electricity_handoff_hop_limit")
}
