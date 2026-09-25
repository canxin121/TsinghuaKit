//! INFO's academic roaming profile for the WebVPN deployment. This does not
//! retry the separate Learn/direct ALL_ZHJW protocol or submit credentials.
use super::*;

pub(super) fn calendar_selector(stage: AcademicStage) -> &'static str {
    match stage {
        AcademicStage::Undergraduate => "287C0C6D90ABB364CD5FDF1495199962",
        AcademicStage::Graduate => "BEABB32641DC4EC3510B048BAF42471A",
    }
}

/// Applies the same academic-stage rule as THUInfo's `InfoHelper.graduate()`.
///
/// The reference client first requires the login id to be an all-digit
/// student id, then treats the fifth digit (index 4) as the stage marker:
/// `2` and `3` mean graduate; every other digit means undergraduate.  Keep
/// this as a pure, fallible helper so an unexpected SSO username never turns
/// into a guessed academic stage.
pub(super) fn academic_stage_from_student_id(username: &str) -> Option<AcademicStage> {
    let bytes = username.as_bytes();
    if bytes.len() <= 4 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }

    Some(if matches!(bytes[4], b'2' | b'3') {
        AcademicStage::Graduate
    } else {
        AcademicStage::Undergraduate
    })
}

fn alternate_stage(stage: AcademicStage) -> AcademicStage {
    match stage {
        AcademicStage::Undergraduate => AcademicStage::Graduate,
        AcademicStage::Graduate => AcademicStage::Undergraduate,
    }
}

fn may_retry_for_stage_detection(error: &str) -> bool {
    matches!(
        crate::telemetry::diagnostic_reason(error),
        "info_handoff_result_failure"
            | "info_handoff_result_with_target"
            | "info_handoff_result_with_object"
            | "info_handoff_result_object_no_target"
            | "info_handoff_result_with_message"
            | "info_handoff_success_flag_failure"
            | "info_handoff_error_field_failure"
            | "info_handoff_message_failure"
            | "info_handoff_rejected"
    )
}

fn may_probe_calendar_after_info_failure(error: &InfoSessionError) -> bool {
    let is_result_failure = |error: &crate::info::InfoError| {
        matches!(
            error,
            crate::info::InfoError::ResultFailure
                | crate::info::InfoError::ResultFailureWithTarget
                | crate::info::InfoError::ResultFailureWithObject
                | crate::info::InfoError::ResultFailureWithObjectNoTarget
                | crate::info::InfoError::ResultFailureWithMessage
        )
    };

    match error {
        InfoSessionError::Profile(error) => is_result_failure(error),
        InfoSessionError::Client(crate::info_client::InfoClientError::Profile(error)) => {
            is_result_failure(error)
        }
        _ => false,
    }
}

impl CampusRuntime {
    pub(super) fn apply_reference_academic_stage(&mut self, user: &UserIdentity) {
        if !self.stage_auto_detection || self.stage_selection_explicit || self.stage_detected {
            return;
        }
        if let Some(stage) = academic_stage_from_student_id(&user.username) {
            self.stage = stage;
            self.stage_detected = true;
        }
    }

    pub(super) async fn establish_registrar_with_stage_detection(
        &mut self,
        user: &UserIdentity,
        registrar: &RegistrarClient,
        window: CalendarWindow,
    ) -> Result<(), String> {
        // The authenticated student id is a stronger source of stage
        // selection than the constructor's UI preference.  Apply it before
        // constructing the first INFO handoff, so the correct selector is
        // sent on the first request for normal THU student accounts.
        self.apply_reference_academic_stage(user);
        if !self.stage_auto_detection || self.stage_selection_explicit || self.stage_detected {
            let result = self
                .establish_registrar_from_info(user, registrar, window)
                .await;
            if result.is_ok() {
                // A successful stage-specific calendar response is the
                // service proof for both an explicit UI selection and a
                // fixed-stage runtime. The user selection itself is not proof.
                self.stage_detected = true;
                self.last_error = None;
            }
            return result;
        }

        let preferred = self.stage;
        match self
            .establish_registrar_from_info(user, registrar, window.clone())
            .await
        {
            Ok(()) => {
                // The stage is not considered known until this exact
                // stage-specific calendar JSONP has been decoded.
                self.stage_detected = true;
                self.last_error = None;
                Ok(())
            }
            Err(error) if may_retry_for_stage_detection(&error) => {
                // The first INFO business envelope was explicit, and the
                // first calendar probe did not prove that stage.  The failed
                // handoff has already invalidated Registrar; clear only the
                // one retry gate so the opposite selector can be attempted
                // once in this login.  No password, captcha, or ALL_ZHJW
                // fallback is involved.
                self.registrar_handoff_failure = None;
                self.stage = alternate_stage(preferred);
                match self
                    .establish_registrar_from_info(user, registrar, window)
                    .await
                {
                    Ok(()) => {
                        self.stage_detected = true;
                        self.last_error = None;
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn establish_registrar_from_info(
        &mut self,
        user: &UserIdentity,
        registrar: &RegistrarClient,
        window: CalendarWindow,
    ) -> Result<(), String> {
        if let Some(reason) = self.registrar_handoff_failure {
            return Err(self.record_business_failure("registrar", "registrar_handoff", reason));
        }
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
                "registrar",
                "registrar_handoff",
                "registrar_session_binding",
            ));
        }
        // Claim before I/O; cancellation cannot make a possibly consumed
        // one-time handoff available to another dependent feature.
        self.registrar_handoff_failure = Some("registrar_handoff_interrupted");
        let result = self
            .registrar_info_handoff_inner(user, registrar, window)
            .await;
        match result {
            Ok(()) => {
                self.registrar_handoff_failure = None;
                Ok(())
            }
            Err(reason) => {
                self.registrar_handoff_failure = Some(reason);
                self.invalidate_registrar_session();
                Err(self.record_business_failure("registrar", "registrar_handoff", reason))
            }
        }
    }

    async fn registrar_info_handoff_inner(
        &mut self,
        user: &UserIdentity,
        registrar: &RegistrarClient,
        window: CalendarWindow,
    ) -> Result<(), &'static str> {
        registrar
            .calendar_request_plan(self.stage, window.clone(), "thyouCalendar")
            .map_err(|_| "registrar_config")?;
        let base =
            Url::parse(&registrar.config().registrar_base_url).map_err(|_| "registrar_config")?;
        let known = Url::parse(REGISTRAR_WEBVPN_BASE_URL).expect("static registrar mapping");
        if base.path() != known.path() || base.query().is_some() || base.fragment().is_some() {
            return Err("registrar_origin_rejected");
        }
        if self
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            .is_some_and(|bound| bound != user)
        {
            return Err("registrar_session_binding");
        }
        if let Some(info) = self.info_adapter.as_ref()
            && (!same_origin(&base, &info.config().webvpn_base_url)
                || !Arc::ptr_eq(
                    info.transport().cookie_jar(),
                    registrar.transport().cookie_jar(),
                )
                || !Arc::ptr_eq(
                    info.transport().cookie_jar(),
                    self.identity.transport().cookie_jar(),
                ))
        {
            return Err("registrar_session_binding");
        }
        self.ensure_info_session(user)
            .await
            .map_err(|_| "registrar_info_unavailable")?;
        if !self.service_session_is_proven(ServiceId::Info)
            || self
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Info)
                .user
                .as_ref()
                != Some(user)
        {
            return Err("registrar_session_binding");
        }
        let info = self
            .info_adapter
            .as_ref()
            .ok_or("registrar_info_unavailable")?;
        if !same_origin(&base, &info.config().webvpn_base_url)
            || !Arc::ptr_eq(
                info.transport().cookie_jar(),
                registrar.transport().cookie_jar(),
            )
            || !Arc::ptr_eq(
                info.transport().cookie_jar(),
                self.identity.transport().cookie_jar(),
            )
        {
            return Err("registrar_session_binding");
        }
        self.coordinator
            .begin_authentication(ServiceId::Registrar)
            .map_err(|_| "registrar_session_binding")?;
        tracing::info!(target:"tsinghua_kit::auth",event="registrar_handoff",service="registrar",business_stage="registrar_info_roaming");
        let target = match info.additional_roaming(calendar_selector(self.stage)).await {
            Ok(target) => target,
            Err(error) => {
                // THUInfo's roamingWrapper first gives the real academic
                // operation a chance and only roams when that operation is
                // not already usable. A fresh runtime can still inherit a
                // valid registrar cookie from the shared WebVPN jar even if
                // INFO returns a failed envelope for this selector. Probe
                // that read-only proof once; only a fully decoded calendar
                // response may establish the Registrar session.
                if may_probe_calendar_after_info_failure(&error)
                    && registrar
                        .fetch_calendar_window(self.stage, window.clone(), "thyouCalendar")
                        .await
                        .is_ok()
                {
                    self.coordinator
                        .mark_authenticated(ServiceId::Registrar, user.clone(), None, None, None)
                        .map_err(|_| "registrar_session_binding")?;
                    return Ok(());
                }
                return Err(info_failure_code(&error));
            }
        };
        let target = Url::parse(target.as_str()).map_err(|_| "registrar_origin_rejected")?;
        if !crate::webvpn_url::endpoint_is_allowed(&base, &target) {
            return Err("registrar_origin_rejected");
        }
        tracing::info!(target:"tsinghua_kit::auth",event="registrar_handoff",service="registrar",business_stage="registrar_calendar_proof");
        registrar
            .fetch_calendar_window(self.stage, window, "thyouCalendar")
            .await
            .map_err(|error| {
                crate::registrar_session::RegistrarSessionError::Client(error).diagnostic_code()
            })?;
        // Neither a ticket, a redirect nor HTTP 200 is authentication proof.
        // The exact calendar endpoint/callback/JSONP must have been parsed.
        self.coordinator
            .mark_authenticated(ServiceId::Registrar, user.clone(), None, None, None)
            .map_err(|_| "registrar_session_binding")?;
        Ok(())
    }
}

#[cfg(test)]
mod academic_stage_tests {
    use super::academic_stage_from_student_id;
    use crate::protocol::AcademicStage;

    #[test]
    fn backend_repair_reference_student_id_rule_maps_fifth_digit_to_stage() {
        assert_eq!(
            academic_stage_from_student_id("2026000000"),
            Some(AcademicStage::Graduate)
        );
        assert_eq!(
            academic_stage_from_student_id("2026313469"),
            Some(AcademicStage::Graduate)
        );
        assert_eq!(
            academic_stage_from_student_id("2026113469"),
            Some(AcademicStage::Undergraduate)
        );
    }

    #[test]
    fn backend_repair_reference_student_id_rule_rejects_non_student_ids() {
        assert_eq!(academic_stage_from_student_id("1234"), None);
        assert_eq!(academic_stage_from_student_id("2026x13469"), None);
        assert_eq!(academic_stage_from_student_id("2026 13469"), None);
    }

    #[test]
    fn backend_repair_bridge_inference_returns_only_a_safe_boolean() {
        assert_eq!(
            crate::api::runtime::infer_academic_stage(" 2026000000 ".to_owned()),
            Some(true)
        );
        assert_eq!(
            crate::api::runtime::infer_academic_stage("2026113469".to_owned()),
            Some(false)
        );
        assert_eq!(
            crate::api::runtime::infer_academic_stage("username".to_owned()),
            None
        );
    }
}

/// Fixed diagnostics never include table values, course names or raw HTML.
pub(super) fn grade_error_code(
    error: &crate::registrar_client::RegistrarClientError,
) -> &'static str {
    use crate::registrar_academic::RegistrarGradesParseError as P;
    use crate::registrar_client::RegistrarClientError as E;
    match error {
        E::LoginExpired { .. }
        | E::InvalidGradeResponse {
            source: P::SessionExpired,
        } => "registrar_grade_login_required",
        E::InvalidGradeContentType => "registrar_grade_content_type",
        E::InvalidGradeResponse { source } => match source {
            P::EmptyResponse => "registrar_grade_empty_response",
            P::MissingGradeTable => "registrar_grade_table_missing",
            P::MissingHeaderRow | P::InvalidHeader { .. } => "registrar_grade_header",
            P::WrongColumnCount { row: 0, .. } => "registrar_grade_columns",
            P::WrongColumnCount { .. } => "registrar_grade_row_columns",
            P::EmptyField { .. } => "registrar_grade_empty_field",
            P::InvalidNumber { .. } => "registrar_grade_number",
            P::BusinessFailure { .. } => "registrar_grade_business_failure",
            P::MalformedHtml { context } => match *context {
                "invalid grade cell span"
                | "overlapping grade cell spans"
                | "gaps in grade cell spans"
                | "unterminated grade cell rowspan"
                | "merged grade data fields" => "registrar_grade_spans",
                "ambiguous grade header"
                | "ambiguous grade report containers"
                | "multiple grade reports" => "registrar_grade_ambiguous_header",
                "grade header stage mismatch" => "registrar_grade_stage",
                "unrecognized rows before grade header"
                | "contradictory empty grade report"
                | "grade summary without records" => "registrar_grade_structure",
                "missing grade course identifier" => "registrar_grade_empty_field",
                _ => "registrar_grade_html",
            },
            _ => "registrar_config",
        },
        E::Transport(crate::transport::TransportError::HttpStatus { .. }) => "registrar_http",
        E::Transport(_) => "registrar_network",
        E::UnexpectedOrigin | E::InvalidConfig { .. } => "registrar_origin_rejected",
        _ => "registrar_response_decode",
    }
}

fn report_needs_target_handoff(error: &crate::registrar_client::RegistrarClientError) -> bool {
    use crate::registrar_academic::RegistrarGradesParseError as P;
    use crate::registrar_client::RegistrarClientError as E;
    matches!(
        error,
        E::LoginExpired { .. }
            | E::InvalidGradeResponse {
                source: P::SessionExpired | P::MissingGradeTable
            }
    ) || matches!(error, E::InvalidGradeResponse {source:P::BusinessFailure {message}} if message == "upstream grade access denied")
}

impl CampusRuntime {
    fn grade_read_failure(&mut self, code: &'static str) -> crate::ServiceError {
        let message = self.record_business_failure("registrar", "registrar_grades", code);
        crate::ServiceError::Adapter { message }
    }

    /// Calendar proof is not grade-menu permission. Match the reference's
    /// read-first -> correct grade-menu roam -> original report read ordering,
    /// but never repeat a failed one-shot roam or treat malformed grades as
    /// permission to reauthenticate. No password is used by this flow.
    pub(super) async fn read_registrar_grade_report(
        &mut self,
        registrar: &RegistrarClient,
        profile: RegistrarGradesProfile,
    ) -> Result<crate::registrar_academic::RegistrarGradeReport, crate::ServiceError> {
        if let Some(code) = self.registrar_grade_handoff_failure {
            return Err(self.grade_read_failure(code));
        }
        let error = match registrar.fetch_grades(profile).await {
            Ok(report) => return Ok(report),
            Err(error) => error,
        };
        let original = grade_error_code(&error);
        tracing::warn!(target:"tsinghua_kit::auth",event="business_read_failure",service="registrar",business_stage="registrar_grades",reason=original);
        if !report_needs_target_handoff(&error) {
            return Err(self.grade_read_failure(original));
        }
        // This bridge supports the verified WebVPN grade-menu mapping only.
        let base = Url::parse(&registrar.config().registrar_base_url)
            .map_err(|_| self.grade_read_failure("registrar_config"))?;
        let known = Url::parse(REGISTRAR_WEBVPN_BASE_URL).expect("static mapping");
        if base.path() != known.path() || base.query().is_some() || base.fragment().is_some() {
            return Err(self.grade_read_failure(original));
        }
        if !self.service_session_is_proven(ServiceId::Identity) || profile.stage() != self.stage {
            return Err(self.grade_read_failure("registrar_session_binding"));
        }
        let user = self
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .user
            .ok_or_else(|| self.grade_read_failure("registrar_session_binding"))?;
        if self
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Registrar)
            .user
            .as_ref()
            != Some(&user)
        {
            return Err(self.grade_read_failure("registrar_session_binding"));
        }
        self.registrar_grade_handoff_failure = Some("registrar_grade_handoff_interrupted");
        if self.ensure_info_session(&user).await.is_err() {
            self.registrar_grade_handoff_failure = Some("registrar_info_unavailable");
            return Err(self.grade_read_failure("registrar_info_unavailable"));
        }
        let info = self
            .info_adapter
            .as_ref()
            .ok_or_else(|| crate::ServiceError::Adapter {
                message: "registrar_info_unavailable".into(),
            })?;
        if !same_origin(&base, &info.config().webvpn_base_url)
            || !Arc::ptr_eq(
                info.transport().cookie_jar(),
                registrar.transport().cookie_jar(),
            )
            || !Arc::ptr_eq(
                info.transport().cookie_jar(),
                self.identity.transport().cookie_jar(),
            )
        {
            self.registrar_grade_handoff_failure = Some("registrar_session_binding");
            return Err(self.grade_read_failure("registrar_session_binding"));
        }
        tracing::info!(target:"tsinghua_kit::auth",event="registrar_handoff",service="registrar",business_stage="registrar_grade_roaming");
        let target = match info.additional_roaming(profile.roaming_selector()).await {
            Ok(target) => target,
            Err(error) => {
                let code = info_failure_code(&error);
                self.registrar_grade_handoff_failure = Some(code);
                return Err(self.grade_read_failure(code));
            }
        };
        if !Url::parse(target.as_str())
            .is_ok_and(|target| crate::webvpn_url::endpoint_is_allowed(&base, &target))
        {
            self.registrar_grade_handoff_failure = Some("registrar_origin_rejected");
            return Err(self.grade_read_failure("registrar_origin_rejected"));
        }
        match registrar.fetch_grades(profile).await {
            Ok(report) => {
                self.registrar_grade_handoff_failure = None;
                Ok(report)
            }
            Err(error) => {
                let code = grade_error_code(&error);
                self.registrar_grade_handoff_failure = Some(code);
                // Retain independently valid calendar proof on format or
                // permission errors; explicit login loss invalidates it.
                if matches!(
                    error,
                    crate::registrar_client::RegistrarClientError::LoginExpired { .. }
                        | crate::registrar_client::RegistrarClientError::InvalidGradeResponse {
                            source:
                                crate::registrar_academic::RegistrarGradesParseError::SessionExpired
                        }
                ) {
                    self.invalidate_registrar_session();
                }
                Err(self.grade_read_failure(code))
            }
        }
    }
}
