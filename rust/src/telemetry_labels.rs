//! Closed source-owned logging vocabulary. The sole dynamic field is a
//! structurally validated, ticket-free WebVPN site mapping on a blocked news
//! redirect; no URL, route, query or response body passes this boundary.
pub(super) const OPERATIONS: &[&str] = &[
    "load_thos_pending",
    "thos_pending",
    "load_thos_services",
    "load_thos_task_list",
    "load_thos_phase_steps",
    "thos_services",
    "thos_completed",
    "thos_drafts",
    "thos_unread",
    "thos_phases",
    "resume_restored_session",
    "load_semester_schedule",
    "load_info_news_detail_result",
    "load_campus_card_account_result",
    "load_campus_card_transactions_result",
    "load_classroom_buildings_result",
    "load_electricity_remainder_result",
    "load_electricity_payment_history_result",
    "campus_card_account",
    "campus_card_session",
    "campus_card_transactions",
    "classroom_buildings",
    "classroom_session",
    "classroom_state",
    "complete_second_factor",
    "complete_service_second_factor",
    "complete_usereg_login",
    "disconnect_tunet",
    "disconnect_usereg_device",
    "electricity_history",
    "electricity_remainder",
    "electricity_session",
    "establish_service_session",
    "identity_session",
    "info_detail",
    "info_news",
    "info_search",
    "info_session",
    "learn_announcements",
    "learn_courses",
    "learn_session",
    "learn_todos",
    "learn_homework",
    "learn_homework_detail",
    "library_area_tree",
    "library_day_segments",
    "library_seats",
    "library_session",
    "library_socket_status",
    "load_campus_card_account",
    "load_campus_card_transactions",
    "load_classroom_buildings",
    "load_classroom_state",
    "load_electricity_payment_history",
    "load_electricity_remainder",
    "load_exams",
    "load_grades",
    "load_info_news",
    "load_info_news_detail",
    "load_learn_announcements",
    "load_learn_courses",
    "load_learn_term_calendar",
    "load_learn_homework",
    "load_learn_homework_detail",
    "load_library_area_tree",
    "load_library_area_tree_result",
    "load_library_day_segments_result",
    "load_library_day_segments",
    "load_library_seats",
    "load_library_socket_status",
    "load_overview",
    "load_tunet_status",
    "load_usereg_account",
    "load_usereg_balance",
    "load_usereg_devices",
    "login",
    "login_tunet",
    "overview_live_inputs",
    "portal_bootstrap",
    "refresh_usereg_captcha",
    "registrar_exams",
    "registrar_grades",
    "registrar_schedule",
    "registrar_session",
    "search_info_news",
    "send_second_factor_code",
    "send_service_second_factor_code",
    "start_usereg_login",
    "tunet_status",
    "tunet_local_status",
    "usereg_account",
    "usereg_balance",
    "usereg_devices",
    "usereg_session",
];
pub(super) const EVENTS: &[&str] = &[
    "homework_parse_rejected",
    "operation_timing",
    "phase_finished",
    "phase_started",
    "body_finished",
    "request_timing",
    "request_wait_finished",
    "case_queued",
    "queue_configured",
    "registrar_grade_structure",
    "registrar_handoff",
    "news_navigation_boundary",
    "info_catalog_response",
    "thos_read_redirect",
    "electricity_target_evidence",
    "learn_home_compatibility",
    "card_session_probe",
    "session_reuse_decision",
    "trusted_device_registration",
    "business_read_failure",
    "portal_route_decision",
    "card_read_failure",
    "portal_navigation_failure",
    "identity_failure_evidence",
    "identity_submission_profile",
    "tunet_status_evidence",
    "identity_response",
    "identity_handoff",
    "factor_response",
    "cache_miss",
    "cache_read_completed",
    "cache_read_failed",
    "cache_write_completed",
    "cache_write_failed",
    "case_finished",
    "case_started",
    "factor_send_requested",
    "factor_submission",
    "operation_finished",
    "operation_started",
    "portal_handoff",
    "rate_limit_cooldown",
    "rate_limit_wait",
    "redirect_blocked",
    "redirect_followed",
    "request_cancelled",
    "request_dispatched",
    "request_failed",
    "request_queued",
    "response_headers",
    "run_finished",
    "run_started",
    "run_stopped",
    "session_restore",
    "transition_applied",
    "transition_rejected",
];
pub(super) const REASONS: &[&str] = &[
    "tunet_local_online",
    "tunet_local_offline",
    "tunet_local_address_unavailable",
    "usereg_http_auth_rejected",
    "usereg_rate_limited",
    "usereg_http_unavailable",
    "usereg_http_rejected",
    "usereg_config",
    "usereg_transport",
    "usereg_session_expired",
    "usereg_response_invalid",
    "usereg_validation_response_invalid",
    "usereg_validation_content_type",
    "usereg_validation_success_missing",
    "usereg_captcha_metadata_invalid",
    "usereg_captcha_image_invalid",
    "usereg_captcha_too_large",
    "usereg_header_csrf_invalid",
    "usereg_form_csrf_missing",
    "usereg_public_key_invalid",
    "usereg_password_encryption",
    "usereg_login_page_invalid",
    "usereg_route_changed",
    "usereg_session_unproven",
    "usereg_account_mismatch",
    "usereg_home_layout_invalid",
    "usereg_devices_invalid",
    "usereg_account_invalid",
    "usereg_balance_invalid",
    "usereg_device_limit_invalid",
    "usereg_action_unconfirmed",
    "info_news_reauth_needs_user",
    "info_news_login_required",
    "info_news_origin_invalid",
    "info_news_query_invalid",
    "info_news_fragment_invalid",
    "info_news_mapping_unknown",
    "info_news_path_invalid",
    "info_news_publication_scope",
    "info_news_location_invalid",
    "info_news_document_received",
    "environment_auth_interaction_required",
    "environment_credentials_invalid",
    "environment_credentials_missing",
    "environment_credential_already_consumed",
    "validation_workspace_invalid",
    "validation_workspace_unavailable",
    "validation_live_result_required",
    "validation_news_catalog_partial",
    "validation_news_catalog_empty",
    "course_evidence_context_mismatch",
    "registrar_grade_row_columns",
    "registrar_grade_header_unrecognized",
    "registrar_grade_spans",
    "registrar_grade_ambiguous_header",
    "registrar_grade_structure",
    "registrar_grade_stage",
    "registrar_grade_login_required",
    "registrar_grade_content_type",
    "registrar_grade_empty_response",
    "registrar_grade_table_missing",
    "registrar_grade_header",
    "registrar_grade_columns",
    "registrar_grade_empty_field",
    "registrar_grade_number",
    "registrar_grade_business_failure",
    "registrar_grade_html",
    "registrar_grade_handoff_interrupted",
    "campus_card_password_required",
    "campus_card_password_declined",
    "campus_card_password_boundary_missing",
    "campus_card_password_boundary_invalid",
    "campus_card_factor_pending",
    "library_sample_budget_exhausted",
    "registrar_unavailable",
    "registrar_session_expired",
    "registrar_network",
    "registrar_response_decode",
    "registrar_info_unavailable",
    "registrar_handoff_interrupted",
    "info_session_expired",
    "electricity_callback_query_rejected",
    "electricity_callback_path_rejected",
    "electricity_absolute_path_mapped",
    "electricity_broker_encoding",
    "electricity_callback_origin_rejected",
    "electricity_callback_uri_rejected",
    "electricity_redirect_target_rejected",
    "registrar_origin_rejected",
    "registrar_ticket_invalid",
    "registrar_http",
    "registrar_calendar_jsonp",
    "registrar_calendar_content_type",
    "registrar_calendar_payload",
    "registrar_config",
    "registrar_session_binding",
    "tunet_request_origin_online",
    "tunet_request_origin_offline",
    "learn_reference_directory_home",
    "campus_card_auth_attempt_exhausted",
    "classroom_mapping_rejected",
    "portal_navigation_target_rejected",
    "portal_navigation_response_rejected",
    "portal_navigation_cycle",
    "portal_navigation_location_missing",
    "portal_broker_uri_rejected",
    "identity_login_rejected",
    "identity_session_invalid",
    "image_captcha_required",
    "http_status",
    "unexpected_origin",
    "login_failed",
    "second_factor_required",
    "authenticated_handoff",
    "redirect_callback",
    "cookie_backed_handoff",
    "login_page",
    "other_page",
    "verified",
    "redirect_id_login_page",
    "invalidation_marked",
    "missing_anchor_ticket",
    "unproven_handoff",
    "verification_code_rejected",
    "credentials_rejected",
    "available",
    "blocked",
    "body_read",
    "campus_card",
    "campus_card_account",
    "campus_card_session",
    "campus_card_transactions",
    "card_origin",
    "case_already_attempted",
    "case_finished",
    "case_started",
    "classroom",
    "classroom_buildings",
    "classroom_session",
    "classroom_state",
    "connect",
    "course_evidence_unavailable",
    "csrf_proof_missing",
    "dependency_not_passed",
    "electricity",
    "electricity_history",
    "electricity_remainder",
    "electricity_session",
    "factor_method_not_advertised",
    "factor_send_requested",
    "factor_still_pending",
    "factor_submission",
    "failed",
    "foreign_origin",
    "fresh_terminal_runtime_required",
    "graduate_exam_not_implemented",
    "identity",
    "identity_callback_route",
    "identity_check_single_route",
    "identity_login_form_route",
    "identity_not_proven",
    "identity_other_route",
    "identity_session",
    "identity_submit_route",
    "immutable_case_result",
    "in_progress",
    "info",
    "info_detail",
    "info_news",
    "info_route",
    "info_search",
    "info_session",
    "interrupted",
    "interrupted_or_output_failure",
    "invalid_terminal_credentials",
    "invalid_transition_or_binding",
    "ip",
    "learn",
    "learn_announcements",
    "learn_courses",
    "learn_session",
    "learn_source_unavailable",
    "learn_todos",
    "learn_todos_unconfirmed",
    "library",
    "library_area_tree",
    "library_day_segments",
    "library_seats",
    "library_session",
    "library_socket_status",
    "markdown_report_exists_or_unavailable",
    "markdown_report_write_failed",
    "missing",
    "no_area_selector",
    "no_article_selector",
    "no_building_selector",
    "no_course_selector",
    "no_open_segment",
    "no_pending_challenge",
    "not_started",
    "oauth_route",
    "one_real_course_one_article_one_area_one_building_seven_card_days",
    "optional_login_not_selected",
    "origin_or_mapping",
    "other_campus_origin",
    "overview",
    "overview_live_inputs",
    "passed",
    "pending",
    "portal_bootstrap",
    "portal_not_proven",
    "registrar",
    "registrar_exams",
    "registrar_grades",
    "registrar_not_proven",
    "registrar_schedule",
    "registrar_session",
    "report_already_exists",
    "report_write_failed",
    "requires_second_factor",
    "running",
    "service_proof_missing_after_factor",
    "skipped",
    "snake_case",
    "storage_io",
    "terminal_read_only",
    "timeout",
    "todo_config_failed",
    "totp",
    "transport",
    "tunet",
    "tunet_config_failed",
    "tunet_state_unproven",
    "tunet_status",
    "tunet_status_unconfirmed",
    "off_campus_not_applicable",
    "off_campus_network_unverified",
    "unverified",
    "unknown_case",
    "unknown_case_selection",
    "unresolved_factor",
    "user_cancelled_factor",
    "user_or_process_stopped",
    "usereg",
    "usereg_account",
    "usereg_balance",
    "usereg_devices",
    "usereg_not_proven",
    "usereg_session",
    "verified",
    "webvpn",
    "webvpn_route",
    "zh",
    "campus_card_account_mismatch",
    "campus_card_cookie_rejected",
    "campus_card_encrypted_payload_format",
    "campus_card_http",
    "campus_card_identity_network",
    "campus_card_identity_page_format",
    "campus_card_identity_page_http",
    "campus_card_interactive_auth_required",
    "campus_card_network",
    "campus_card_probe_account_missing",
    "campus_card_probe_config",
    "campus_card_probe_format",
    "campus_card_probe_http",
    "campus_card_probe_invalid_envelope",
    "campus_card_probe_invalid_json",
    "campus_card_probe_network",
    "campus_card_probe_outer_failure",
    "campus_card_probe_result_missing",
    "campus_card_probe_route",
    "campus_card_probe_unavailable",
    "campus_card_rate_limited",
    "campus_card_response",
    "campus_card_route",
    "campus_card_second_factor_required",
    "campus_card_service_failure",
    "campus_card_service_rejected",
    "campus_card_session_expired",
    "campus_card_session_unconfirmed",
    "campus_card_target_missing",
    "campus_card_trusted_continuation_network",
    "campus_card_trusted_continuation_unconfirmed",
    "campus_card_unavailable",
    "card_read_account_mismatch",
    "card_read_cents",
    "card_read_config",
    "card_read_date",
    "card_read_date_range",
    "card_read_encrypted_format",
    "card_read_encrypted_rejection",
    "card_read_envelope",
    "card_read_failure",
    "card_read_field_missing",
    "card_read_http",
    "card_read_json",
    "card_read_no_card",
    "card_read_non_json",
    "card_read_number",
    "card_read_origin",
    "card_read_path",
    "card_read_probe_format",
    "card_read_redirect",
    "card_read_rejected",
    "card_read_result_missing",
    "card_read_session_expired",
    "card_read_shape",
    "card_read_text",
    "card_read_transport",
    "portal_after_handoff_csrf_missing",
    "portal_after_handoff_login_required",
    "portal_config",
    "portal_crypto",
    "portal_fresh_entry_unconfirmed",
    "portal_fresh_submit_unconfirmed",
    "portal_fresh_target_unconfirmed",
    "portal_identity_entry_form_without_key",
    "portal_identity_entry_no_form",
    "portal_identity_entry_password_form",
    "portal_identity_entry_trusted_form",
    "portal_identity_entry_trusted_script",
    "portal_identity_required",
    "portal_identity_submitted_generic_notice",
    "portal_identity_target_form_without_key",
    "portal_identity_target_no_form",
    "portal_identity_target_password_form",
    "portal_identity_target_trusted_form",
    "portal_identity_target_trusted_script",
    "portal_interactive_auth_required",
    "portal_login_http",
    "portal_login_page_format",
    "portal_login_page_http",
    "portal_network",
    "portal_oauth_generic_notice",
    "portal_oauth_rejected",
    "portal_oauth_unconfirmed",
    "portal_resource_document_login",
    "portal_resource_http",
    "portal_resource_http_rejected",
    "portal_resource_identity_login",
    "portal_resource_login_required",
    "portal_resource_unconfirmed",
    "portal_resource_webvpn_login",
    "portal_resume_account_format",
    "portal_resume_account_http",
    "portal_resume_account_mismatch",
    "portal_resume_account_route",
    "portal_resume_captcha_required",
    "portal_resume_config",
    "portal_resume_cookie_http",
    "portal_resume_cookie_route",
    "portal_resume_credential_page_rejected",
    "portal_resume_csrf_missing",
    "portal_resume_identity_failure",
    "portal_resume_identity_rejected",
    "portal_resume_login_required",
    "portal_resume_navigation_unconfirmed",
    "portal_resume_network",
    "portal_resume_password_required",
    "portal_resume_rate_limited",
    "portal_resume_second_factor_required",
    "portal_resume_sso_body_limit",
    "portal_resume_sso_encoding",
    "portal_resume_sso_hop_limit",
    "portal_resume_sso_http",
    "portal_resume_sso_network",
    "portal_resume_sso_rejected",
    "portal_resume_sso_route",
    "portal_resume_trusted_form_without_key",
    "portal_resume_trusted_network",
    "portal_resume_trusted_no_form",
    "portal_resume_trusted_password_form",
    "portal_resume_trusted_script_only",
    "portal_resume_trusted_unconfirmed",
    "portal_resume_unconfirmed",
    "portal_second_factor_required",
    "portal_session_unconfirmed",
    "portal_target_missing",
    "portal_target_rejected",
    "portal_trusted_continuation_network",
    "portal_trusted_continuation_unconfirmed",
    "portal_webvpn_entry_generic_notice",
    "portal_webvpn_home_generic_notice",
    "portal_webvpn_target_generic_notice",
    "tunet_address_mismatch",
    "tunet_stack_mismatch",
    "info_origin_rejected",
    "info_mapping_rejected",
    "info_csrf_missing",
    "info_handoff_rejected",
    "info_handoff_result_failure",
    "info_handoff_result_with_target",
    "info_handoff_result_with_object",
    "info_handoff_result_object_no_target",
    "info_handoff_result_with_message",
    "info_handoff_success_flag_failure",
    "info_handoff_error_field_failure",
    "info_handoff_message_failure",
    "info_handoff_envelope",
    "info_handoff_client",
    "info_transport",
    "info_handoff_proof_missing",
    "info_handoff_http",
    "info_detail_envelope",
    "info_detail_title",
    "info_detail_content",
    "info_detail_id",
    "info_detail_field",
    "info_detail_encoding",
    "info_detail_legacy_policy",
    "info_detail_legacy_html",
    "info_detail_legacy_title",
    "info_detail_legacy_tag_unterminated",
    "info_detail_legacy_fragment",
    "info_detail_legacy_control",
    "info_detail_legacy_summary_markup",
    "info_detail_legacy_entity",
    "info_detail_legacy_summary",
    "info_detail_legacy_selector_missing",
    "info_detail_legacy_empty",
    "info_detail_legacy_content",
    "info_detail_parse",
    "info_catalog_format",
    "info_service_unconfirmed",
    "library_floor_unconfirmed",
    "library_section_unconfirmed",
    "electricity_amount",
    "electricity_body_empty",
    "electricity_auth_required",
    "electricity_config",
    "electricity_content_type",
    "electricity_field_missing",
    "electricity_field_duplicate",
    "electricity_field_empty",
    "electricity_handoff",
    "electricity_handoff_body_limit",
    "electricity_handoff_config",
    "electricity_handoff_cycle",
    "electricity_handoff_hop_limit",
    "electricity_handoff_http",
    "electricity_handoff_location",
    "electricity_handoff_network",
    "electricity_handoff_target",
    "electricity_handoff_unconfirmed",
    "electricity_http",
    "electricity_identity_binding",
    "electricity_identity_http",
    "electricity_identity_network",
    "electricity_identity_rejected",
    "electricity_network",
    "electricity_origin",
    "electricity_parse",
    "electricity_number_range",
    "electricity_history_table_missing",
    "electricity_history_table_duplicate",
    "electricity_history_table_shape",
    "electricity_history_row_shape",
    "electricity_history_sequence",
    "electricity_history_timestamp",
    "electricity_history_amount",
    "electricity_history_status",
    "electricity_path",
    "electricity_public_key_invalid",
    "electricity_public_key_missing",
    "electricity_read",
    "electricity_second_factor_required",
    "electricity_template",
    "electricity_timestamp",
    "electricity_trusted_unconfirmed",
    "info_news_link_conflict",
    "library_auth_required",
    "library_business_failure",
    "library_collection",
    "library_config",
    "library_conflicting_ids",
    "library_data_envelope",
    "library_foreign_context",
    "library_html",
    "library_http",
    "library_network",
    "library_origin",
    "library_parse",
    "library_path",
    "library_record",
    "library_request",
    "library_segment_record",
    "classroom_handoff",
    "classroom_read",
    "electricity_target_evidence",
    "electricity_target_missing",
    "electricity_target_selected",
    "info_news_http",
    "info_news_redirect_cycle",
    "info_news_redirect_followed",
    "info_news_redirect_limit",
    "info_news_redirect_rejected",
    "classroom_auth_required",
    "classroom_body_empty",
    "classroom_buildings_empty",
    "classroom_business_rejected",
    "classroom_deployment",
    "classroom_html",
    "classroom_http",
    "classroom_link_encoding",
    "classroom_link_path",
    "classroom_link_query",
    "classroom_link_week",
    "classroom_network",
    "classroom_origin",
    "classroom_parse",
    "classroom_protocol",
    "classroom_response_path",
    "electricity_callback_http",
    "electricity_callback_origin",
    "electricity_callback_rejected",
    "electricity_callback_target_missing",
    "electricity_success_marker_missing",
    "learn_auth_required",
    "learn_course_home",
    "learn_csrf_invalid",
    "learn_home_csrf_missing",
    "learn_home_http",
    "learn_home_proof_missing",
    "learn_response_format",
    "learn_response_origin",
    "learn_response_path",
    "learn_response_query",
    "learn_target_handoff",
    "learn_transport",
    "library_segment_day",
    "library_segment_end",
    "library_segment_id",
    "library_segment_start",
];

pub(super) fn allowed(field: &str, value: &str) -> bool {
    match field {
        "bucket" => matches!(value, "pending" | "submitted" | "graded"),
        "parse_reason" => matches!(
            value,
            "invalid_json"
                | "invalid_root"
                | "result_failure"
                | "invalid_collection"
                | "invalid_record"
                | "invalid_field"
                | "missing_field"
                | "invalid_date"
                | "course_mismatch"
        ),
        "parse_field" => matches!(
            value,
            "none"
                | "course_id"
                | "student_id"
                | "base_id"
                | "title"
                | "deadline"
                | "late_deadline"
                | "submission_time"
                | "grade_time"
                | "creation_time"
                | "other"
        ),
        "body_shape" => matches!(
            value,
            "json_object" | "json_array" | "html" | "other" | "empty"
        ),
        "date_shape" => matches!(
            value,
            "none"
                | "empty"
                | "dotnet"
                | "digits"
                | "unicode"
                | "iso_t_zone"
                | "iso_t_local"
                | "slash"
                | "dash_space_alpha"
                | "dash_space_zone"
                | "dash_space"
                | "dash_date"
                | "dot"
                | "other"
        ),
        "date_width" => matches!(
            value,
            "none"
                | "w10"
                | "w13"
                | "w16"
                | "w19"
                | "w21"
                | "w23"
                | "w24"
                | "w25"
                | "w29"
                | "other"
        ),
        "news_mapping_id" => super::safe_webvpn_mapping_id(value),
        "reuse_result" => matches!(
            value,
            "probe_denied_initial_handoff"
                | "initial_handoff_exhausted"
                | "probe_existing"
                | "cookies_verified"
                | "target_auth_required"
                | "no_reauthentication"
                | "memory_proof"
                | "trusted_continuation"
                | "mfa_required"
        ),
        "trust_result" => matches!(
            value,
            "saved"
                | "limit_reached"
                | "rejected"
                | "invalid_response"
                | "request_unconfirmed"
                | "consent_granted"
                | "consent_declined"
        ),
        "header_term_kind" => matches!(
            value,
            "standard_term"
                | "academic_year_term"
                | "selection_term"
                | "course_term"
                | "study_year_term"
                | "unrecognized"
        ),
        "header_term_shape" => matches!(
            value,
            "none"
                | "year_only"
                | "term_with_context"
                | "year_with_context"
                | "other"
                | "standard_term"
                | "academic_year_term"
                | "selection_term"
                | "course_term"
                | "study_year_term"
        ),
        "reference_fallback_state" => matches!(
            value,
            "none"
                | "stage_or_width"
                | "role_positions"
                | "context_role"
                | "semester_header_shape"
                | "eligible"
        ),
        "header_failure_stage" => matches!(value, "schema" | "header_structure" | "preamble"),
        "grade_field" => matches!(
            value,
            "course_name" | "credit" | "grade" | "grade_point" | "semester"
        ),
        "grade_value_shape" => matches!(
            value,
            "empty"
                | "ascii_letters"
                | "ascii_letters_digits"
                | "ascii_letters_punctuation"
                | "ascii_letters_digits_punctuation"
                | "ascii_letters_space"
                | "ascii_numeric_other"
                | "ascii_punctuation"
                | "ascii_other"
                | "unicode"
                | "mixed_unicode"
        ),
        "business_stage" => matches!(
            value,
            "registrar_info_roaming"
                | "registrar_grade_roaming"
                | "registrar_grades"
                | "registrar_grade_header"
                | "registrar_grade_data"
                | "registrar_calendar_proof"
                | "registrar_handoff"
                | "learn_course_home"
                | "classroom_read"
                | "card_probe_before_handoff"
                | "card_probe_after_handoff"
                | "learn_target_handoff"
                | "classroom_handoff"
                | "info_detail"
                | "info_catalog_sources"
                | "info_catalog_channels"
                | "electricity_read"
                | "electricity_handoff"
                | "library_read"
                | "library_floor"
                | "library_section"
                | "start_usereg_login"
                | "refresh_usereg_captcha"
                | "usereg_validate_user"
                | "usereg_final_login"
                | "usereg_account_proof"
                | "usereg_account"
                | "usereg_balance"
                | "usereg_devices"
        ),
        "route_decision" => matches!(
            value,
            "dispatch"
                | "http_location"
                | "reject_route"
                | "reject_cycle"
                | "reject_response"
                | "reject_location"
                | "navigation_complete"
        ),
        "route_class" => matches!(
            value,
            "unsafe_url_components"
                | "webvpn_home"
                | "webvpn_login"
                | "webvpn_invalid_encoding"
                | "webvpn_info_root"
                | "webvpn_info_resource"
                | "webvpn_identity_route"
                | "webvpn_control_route"
                | "webvpn_other_mapping"
                | "webvpn_other_protocol"
                | "webvpn_other_route"
                | "oauth_broker"
                | "oauth_callback"
                | "oauth_other_route"
                | "identity_form"
                | "identity_callback"
                | "identity_trusted_form"
                | "identity_other_route"
                | "info_direct"
                | "foreign_origin"
        ),
        "data_field" => matches!(
            value,
            "effective_date"
                | "valid_until"
                | "last_transaction_date"
                | "account_id"
                | "display_name"
                | "department_name"
                | "department_id"
                | "balance_cents"
                | "amount_cents"
                | "post_balance_cents"
                | "daily_limit_cents"
                | "single_limit_cents"
                | "card_status"
                | "card_id"
                | "gender"
                | "transaction_summary"
                | "transaction_date"
                | "merchant_address"
                | "merchant_name"
                | "transaction_name"
                | "other_field"
                | "not_applicable"
        ),
        "evidence_source" => matches!(value, "field" | "visible_text" | "invalidation_marker"),
        "evidence_marker" => matches!(
            value,
            "login_error"
                | "error_msg"
                | "error_message"
                | "captcha_error"
                | "login_invalid"
                | "configured_field"
                | "invalidation_marker"
                | "credentials_text"
                | "captcha_text"
                | "session_text"
                | "generic_text"
        ),
        "submission_encoding" => matches!(value, "urlencoded" | "multipart"),
        "submission_route" => matches!(value, "credential_check" | "trusted_check_single"),
        "response_encoding" => matches!(value, "json" | "jsonp" | "legacy_comma"),
        "status_signal" => matches!(value, "positive" | "negative" | "unknown"),
        "observed_state" | "bound_state" => matches!(value, "online" | "offline" | "unknown"),
        "news_target" => matches!(
            value,
            "info"
                | "system_publication"
                | "legacy_reference"
                | "login"
                | "userinfo"
                | "foreign_origin"
                | "portal_control"
                | "unmapped"
                | "unknown_mapping"
        ),
        "thos_target" => matches!(
            value,
            "unsafe_url"
                | "identity_login"
                | "webvpn_home"
                | "webvpn_login"
                | "webvpn_identity_login"
                | "webvpn_identity_mapping_other"
                | "mapped_thos"
                | "webvpn_thos_root"
                | "webvpn_thos_other"
                | "webvpn_other_mapping_root"
                | "webvpn_other_mapping_fp"
                | "webvpn_other_mapping_identity"
                | "webvpn_other_mapping_info"
                | "webvpn_other_mapping_route"
                | "webvpn_other_protocol"
                | "webvpn_other_route"
                | "direct_thos"
                | "direct_thos_other"
                | "foreign_origin"
        ),
        "news_fragment" => matches!(
            value,
            "none" | "publish_prefix" | "publish_suffix" | "unsupported"
        ),
        "news_query" => matches!(
            value,
            "none" | "ordinary" | "csrf" | "malformed" | "control"
        ),
        "event" => EVENTS.contains(&value),
        "operation" | "case" => OPERATIONS.contains(&value),
        "service" | "endpoint" => matches!(
            value,
            "oauth"
                | "identity"
                | "webvpn"
                | "learn"
                | "registrar"
                | "info"
                | "library"
                | "classroom"
                | "electricity"
                | "campus_card"
                | "tunet"
                | "usereg"
                | "overview"
                | "service"
                | "loopback"
                | "other_campus"
                | "external"
        ),
        "method" => matches!(
            value,
            "get"
                | "post"
                | "put"
                | "delete"
                | "head"
                | "options"
                | "patch"
                | "other"
                | "wechat"
                | "sms"
                | "mobile"
                | "totp"
        ),
        "from" | "to" => matches!(
            value,
            "anonymous" | "authenticating" | "requires_second_factor" | "authenticated" | "expired"
        ),
        "outcome" => matches!(
            value,
            "success"
                | "failed"
                | "cancelled"
                | "completed"
                | "incomplete"
                | "pending"
                | "running"
                | "passed"
                | "blocked"
                | "skipped"
                | "unverified"
                | "interrupted"
                | "restored"
                | "none"
                | "rejected"
                | "error"
        ),
        "phase" => matches!(
            value,
            "runtime_queue"
                | "gate_queue"
                | "rate_limit"
                | "response_headers"
                | "response_body"
                | "decode"
                | "cache_read"
                | "cache_write"
                | "credential_store"
                | "auth_bootstrap"
                | "session_recovery"
                | "service_handoff"
                | "user_input"
                | "selected_target"
                | "primary_submit"
                | "primary_handoff"
                | "handoff_fetch"
                | "second_factor_redirect"
                | "second_factor_handoff"
                | "other"
        ),
        "category" => matches!(
            value,
            "network" | "session" | "response" | "unsupported" | "other" | "dependency"
        ),
        "reason" => {
            REASONS.contains(&value) || crate::live_validation::error_reason(value) == value
        }
        _ => false,
    }
}

#[cfg(test)]
mod coverage_tests {
    #[test]
    fn backend_repair_coverage_unverified_telemetry_retains_fixed_label() {
        assert!(super::allowed("outcome", "unverified"));
        assert!(super::allowed("reason", "off_campus_network_unverified"));
        assert!(super::allowed("reason", "validation_live_result_required"));
        assert!(!super::allowed("outcome", "unverified-user-supplied-value"));
    }

    #[test]
    fn backend_repair_news_catalog_status_log_keeps_only_fixed_stage_labels() {
        assert!(super::EVENTS.contains(&"info_catalog_response"));
        assert!(super::allowed("business_stage", "info_catalog_sources"));
        assert!(super::allowed("business_stage", "info_catalog_channels"));
        assert!(!super::allowed("business_stage", "https://foreign.invalid"));
    }
}
