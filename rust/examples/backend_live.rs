//! Backend-only, read-only verification. Credentials arrive on stdin once.
//! No App session is restored, written, exported, or printed.
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};
use tsinghua_kit::api::runtime::{
    BackendWechatSecondFactorAction, backend_validation_wechat_action,
    create_backend_validation_runtime, run_backend_validation_batch,
};

fn input(reader: &mut impl BufRead) -> Result<String, &'static str> {
    let mut line = String::new();
    if reader
        .read_line(&mut line)
        .map_err(|_| "input_unavailable")?
        == 0
    {
        return Err("input_missing");
    }
    if line.len() > 4096 {
        return Err("input_too_long");
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn login_reason(error: &str) -> &'static str {
    if error.contains("密码错误") {
        "credentials_rejected"
    } else if error.contains("二次") || error.contains("验证码") {
        "second_factor_required"
    } else if error.contains("票据") {
        "service_ticket_missing"
    } else if error.contains("网络") || error.contains("连接") {
        "network"
    } else if error.contains("来源") {
        "origin_rejected"
    } else if error.contains("响应") || error.contains("格式") {
        "response_unconfirmed"
    } else {
        "login_unconfirmed"
    }
}

fn second_factor_failure_reason(error: &str) -> &'static str {
    if error.contains("verification code was rejected") {
        "verification_code_rejected"
    } else if error.contains("second-factor response was not valid JSON") {
        "response_invalid_json"
    } else if error.contains("second-factor response came from an unexpected route") {
        "response_unexpected_route"
    } else if error.contains("second-factor verification request failed") {
        "verification_request_failed"
    } else if error.contains("second-factor redirect request failed") {
        "redirect_request_failed"
    } else if error.contains("did not contain a verified flow") {
        "verified_flow_missing"
    } else if error.contains("did not contain a redirect URL") {
        "redirect_url_missing"
    } else if error.contains("second-factor request failed") {
        "second_factor_request_failed"
    } else if error.contains("second-factor action is not configured") {
        "second_factor_action_missing"
    } else {
        "second_factor_unconfirmed"
    }
}

fn service_second_factor_failure_reason(error: &str) -> &'static str {
    if error.contains("目标页跳转被拒绝:info_route") {
        "portal_target_redirect_info_route"
    } else if error.contains("目标页跳转被拒绝:webvpn_route") {
        "portal_target_redirect_webvpn_route"
    } else if error.contains("目标页跳转被拒绝:oauth_route") {
        "portal_target_redirect_oauth_route"
    } else if error.contains("目标页跳转被拒绝:info_origin_variant") {
        "portal_target_redirect_info_origin_variant"
    } else if error.contains("目标页跳转被拒绝:webvpn_origin_variant") {
        "portal_target_redirect_webvpn_origin_variant"
    } else if error.contains("目标页跳转被拒绝:oauth_origin_variant") {
        "portal_target_redirect_oauth_origin_variant"
    } else if error.contains("目标页跳转被拒绝:identity_callback_route") {
        "portal_target_redirect_identity_callback_route"
    } else if error.contains("目标页跳转被拒绝:identity_submit_route") {
        "portal_target_redirect_identity_submit_route"
    } else if error.contains("目标页跳转被拒绝:identity_check_single_route") {
        "portal_target_redirect_identity_check_single_route"
    } else if error.contains("目标页跳转被拒绝:identity_login_form_route") {
        "portal_target_redirect_identity_login_form_route"
    } else if error.contains("目标页跳转被拒绝:identity_other_route") {
        "portal_target_redirect_identity_other_route"
    } else if error.contains("目标页跳转被拒绝:card_origin") {
        "portal_target_redirect_card_origin"
    } else if error.contains("目标页跳转被拒绝:other_campus_origin") {
        "portal_target_redirect_other_campus_origin"
    } else if error.contains("目标页跳转被拒绝:foreign_origin") {
        "portal_target_redirect_foreign_origin"
    } else if error.contains("目标页跳转未确认") {
        "portal_target_redirect_missing"
    } else if error.contains("目标页跳转被拒绝（INFO 路径）") {
        "portal_target_redirect_info_route"
    } else if error.contains("目标页跳转被拒绝（WebVPN 路径）") {
        "portal_target_redirect_webvpn_route"
    } else if error.contains("目标页跳转被拒绝（OAuth 路径）") {
        "portal_target_redirect_oauth_route"
    } else if error.contains("目标页跳转被拒绝（来源）") {
        "portal_target_redirect_foreign_origin"
    } else if error.contains("目标页跳转被拒绝（INFO 来源变体）") {
        "portal_target_redirect_info_origin_variant"
    } else if error.contains("目标页跳转被拒绝（WebVPN 来源变体）") {
        "portal_target_redirect_webvpn_origin_variant"
    } else if error.contains("目标页跳转被拒绝（OAuth 来源变体）") {
        "portal_target_redirect_oauth_origin_variant"
    } else if error.contains("目标页跳转被拒绝:identity_callback_route") {
        "portal_target_redirect_identity_callback_route"
    } else if error.contains("目标页跳转被拒绝:identity_submit_route") {
        "portal_target_redirect_identity_submit_route"
    } else if error.contains("目标页跳转被拒绝:identity_check_single_route") {
        "portal_target_redirect_identity_check_single_route"
    } else if error.contains("目标页跳转被拒绝:identity_login_form_route") {
        "portal_target_redirect_identity_login_form_route"
    } else if error.contains("目标页跳转被拒绝:identity_other_route") {
        "portal_target_redirect_identity_other_route"
    } else if error.contains("目标页跳转被拒绝（统一认证回调路径）") {
        "portal_target_redirect_identity_callback_route"
    } else if error.contains("目标页跳转被拒绝（统一认证提交路径）") {
        "portal_target_redirect_identity_submit_route"
    } else if error.contains("目标页跳转被拒绝（统一认证 checkSingle 路径）") {
        "portal_target_redirect_identity_check_single_route"
    } else if error.contains("目标页跳转被拒绝（统一认证登录表单路径）") {
        "portal_target_redirect_identity_login_form_route"
    } else if error.contains("目标页跳转被拒绝（统一认证其他路径）") {
        "portal_target_redirect_identity_other_route"
    } else if error.contains("目标页跳转被拒绝（统一认证来源）") {
        "portal_target_redirect_identity_origin"
    } else if error.contains("目标页跳转被拒绝（校园卡来源）") {
        "portal_target_redirect_card_origin"
    } else if error.contains("目标页跳转被拒绝（其他校园来源）") {
        "portal_target_redirect_other_campus_origin"
    } else if error.contains("目标页跳转链过长") {
        "portal_target_redirect_loop"
    } else if error.contains("目标页跳转被拒绝") {
        "portal_target_redirect_rejected"
    } else if error.contains("目标页状态未确认") {
        "portal_target_page_unconfirmed"
    } else if error.contains("portal_resume_network") {
        "portal_resume_network"
    } else if error.contains("portal_resume_cookie") {
        "portal_resume_cookie_unconfirmed"
    } else if error.contains("portal_resume_account") {
        "portal_resume_account_unconfirmed"
    } else if error.contains("portal_resource_") {
        "portal_resource_unconfirmed"
    } else {
        "service_handoff_unconfirmed"
    }
}

fn emit(value: serde_json::Value) {
    println!("{value}");
    let _ = io::stdout().flush();
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 && args.len() != 5 {
        eprintln!(
            "Usage: backend_live <absolute-results.json> <cases.csv> <device-cache-path> [send_wechat_once]"
        );
        std::process::exit(2);
    }
    if args.len() == 5 && args[4] != "send_wechat_once" {
        emit(serde_json::json!({"state":"invalid_send_mode"}));
        std::process::exit(2);
    }
    let explicit_send_enabled = args.len() == 5;
    let ledger = PathBuf::from(&args[1]);
    if !ledger.is_absolute() || ledger.file_name().is_none() {
        emit(serde_json::json!({"state":"invalid_report_path"}));
        std::process::exit(2);
    }
    // The caller holds an OS-level exclusive lock throughout this process.
    // No network action occurs before runtime construction and input validation.
    let mut runtime = match create_backend_validation_runtime("auto".into(), false, args[3].clone())
    {
        Ok(runtime) => runtime,
        Err(_) => {
            emit(serde_json::json!({"state":"runtime_init_failed"}));
            std::process::exit(2);
        }
    };
    if runtime.status().state != "signed_out" {
        emit(serde_json::json!({"state":"fresh_runtime_not_anonymous"}));
        std::process::exit(2);
    }
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let first = input(&mut reader);
    let second = input(&mut reader);
    let third = input(&mut reader);
    let (username, password, verification_code) = match (first, second, third) {
        (Ok(username), Ok(password), verification_code)
            if !username.is_empty() && !password.is_empty() =>
        {
            (username, password, verification_code.ok())
        }
        _ => {
            emit(serde_json::json!({"state":"input_missing"}));
            std::process::exit(2);
        }
    };
    emit(serde_json::json!({"state":"fresh_login_started","session_origin":"empty_cookie_jar"}));
    // Exactly one primary login. Never automatically send a verification code,
    // register a new trusted device, retry login, or change network connectivity.
    let mut status = match runtime
        .login(username, password, None, false, false, false)
        .await
    {
        Ok(status) => status,
        Err(error) => {
            emit(serde_json::json!({"state":"fresh_login_failed","reason":login_reason(&error)}));
            std::process::exit(1);
        }
    };
    if !status.second_factor_methods.is_empty() {
        match backend_validation_wechat_action(
            &status.second_factor_methods,
            verification_code,
            explicit_send_enabled,
        ) {
            BackendWechatSecondFactorAction::Submit(code) => {
                emit(serde_json::json!({
                    "state":"second_factor_submission_started",
                    "method":"wechat",
                    "code_sent":false
                }));
                status = match runtime
                    .complete_second_factor(String::from("wechat"), code)
                    .await
                {
                    Ok(status) => status,
                    Err(error) => {
                        emit(serde_json::json!({
                            "state":"second_factor_failed",
                            "code_sent":false,
                            "reason": second_factor_failure_reason(&error)
                        }));
                        std::process::exit(1);
                    }
                };
                if status.state != "authenticated" {
                    emit(serde_json::json!({
                        "state":"second_factor_not_authenticated",
                        "code_sent":false
                    }));
                    std::process::exit(1);
                }
            }
            BackendWechatSecondFactorAction::SendOnceThenPrompt => {
                if runtime
                    .send_second_factor_code(String::from("wechat"))
                    .await
                    .is_err()
                {
                    emit(serde_json::json!({"state":"second_factor_send_failed","code_sent":true}));
                    std::process::exit(1);
                }
                emit(serde_json::json!({
                    "state":"second_factor_code_requested",
                    "method":"wechat",
                    "send_attempts":1,
                    "code_sent":true
                }));
                let code = match input(&mut reader) {
                    Ok(code) if !code.is_empty() && code.len() <= 64 => code,
                    _ => {
                        emit(serde_json::json!({"state":"second_factor_code_missing"}));
                        std::process::exit(2);
                    }
                };
                status = match runtime
                    .complete_second_factor(String::from("wechat"), code)
                    .await
                {
                    Ok(status) => status,
                    Err(error) => {
                        emit(serde_json::json!({
                            "state":"second_factor_failed",
                            "code_sent":true,
                            "reason": second_factor_failure_reason(&error)
                        }));
                        std::process::exit(1);
                    }
                };
                if status.state != "authenticated" {
                    emit(serde_json::json!({
                        "state":"second_factor_not_authenticated",
                        "code_sent":true
                    }));
                    std::process::exit(1);
                }
            }
            BackendWechatSecondFactorAction::Unused => {
                emit(serde_json::json!({"state":"fresh_login_not_authenticated"}));
                std::process::exit(1);
            }
            BackendWechatSecondFactorAction::MissingCode => {
                emit(serde_json::json!({
                    "state":"blocked_second_factor",
                    "code_sent":false
                }));
                std::process::exit(3);
            }
            BackendWechatSecondFactorAction::Unsupported => {
                emit(serde_json::json!({
                    "state":"blocked_second_factor_method",
                    "code_sent":false
                }));
                std::process::exit(3);
            }
        }
    }
    if status.state != "authenticated" {
        emit(serde_json::json!({"state":"fresh_login_not_authenticated"}));
        std::process::exit(1);
    }
    emit(serde_json::json!({"state":"fresh_login_passed","session_origin":"fresh_backend_login"}));
    let first_batch =
        run_backend_validation_batch(&mut runtime, args[2].clone(), args[1].clone()).await;
    let report = match first_batch {
        Ok(report) => {
            // This contains only case keys, status, attempts and fixed reasons.
            println!("{report}");
            report
        }
        Err(_) => {
            emit(serde_json::json!({"state":"validation_batch_failed"}));
            std::process::exit(1);
        }
    };
    let mut report = report;
    for _ in 0..2 {
        let service_methods = runtime.service_second_factor_methods();
        if !service_methods.iter().any(|method| method == "wechat") {
            break;
        }
        emit(serde_json::json!({
            "state":"service_second_factor_pending",
            "method":"wechat"
        }));
        if explicit_send_enabled {
            if runtime
                .send_service_second_factor_code(String::from("wechat"))
                .await
                .is_err()
            {
                emit(serde_json::json!({"state":"service_second_factor_send_failed"}));
                std::process::exit(1);
            }
            emit(serde_json::json!({
                "state":"second_factor_code_requested",
                "method":"wechat",
                "send_attempts":1
            }));
        }
        let code = match input(&mut reader) {
            Ok(code) if !code.is_empty() && code.len() <= 64 => code,
            _ => {
                emit(serde_json::json!({"state":"second_factor_code_missing"}));
                std::process::exit(2);
            }
        };
        if let Err(reason) = runtime.complete_service_second_factor(code).await {
            emit(serde_json::json!({
                "state":"service_second_factor_failed",
                "reason":service_second_factor_failure_reason(&reason)
            }));
            std::process::exit(1);
        }
        report = match run_backend_validation_batch(&mut runtime, args[2].clone(), args[1].clone())
            .await
        {
            Ok(report) => report,
            Err(_) => {
                emit(serde_json::json!({"state":"validation_batch_failed"}));
                std::process::exit(1);
            }
        };
    }
    let value: serde_json::Value = serde_json::from_str(&report).expect("backend report JSON");
    let incomplete = args[2]
        .split(',')
        .any(|name| value["cases"][name]["status"] != "passed");
    emit(serde_json::json!({"state":"finished","session_reusable_after_exit":false}));
    if incomplete {
        std::process::exit(1);
    }
}
