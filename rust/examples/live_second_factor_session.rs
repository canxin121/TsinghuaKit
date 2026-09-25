use std::io::{self, BufRead, Write};

use tsinghua_kit::{CampusRuntime, create_runtime};

fn error_category(error: &str) -> &'static str {
    let normalized = error.to_ascii_lowercase();
    if error.contains("验证码") || normalized.contains("verification") {
        "verification_failure"
    } else if error.contains("服务票据") || normalized.contains("anchor ticket") {
        "missing_service_ticket"
    } else if error.contains("二次") || normalized.contains("second") {
        "second_factor_failure"
    } else if error.contains("网络") || normalized.contains("network") {
        "network_failure"
    } else {
        "other_failure"
    }
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut value = String::new();
    reader
        .read_line(&mut value)
        .ok()
        .filter(|count| *count > 0)?;
    Some(value.trim_end_matches(['\r', '\n']).to_owned())
}

fn emit(message: &str) {
    println!("{message}");
    let _ = io::stdout().flush();
}

fn available_service_ids(runtime: &CampusRuntime) -> String {
    let ids = runtime
        .service_catalog()
        .services
        .into_iter()
        .filter(|service| service.availability == "available")
        .map(|service| service.id)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        String::from("none")
    } else {
        ids.join(",")
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let Some(username) = read_line(&mut reader) else {
        emit("probe=missing_input");
        return;
    };
    let Some(password) = read_line(&mut reader) else {
        emit("probe=missing_input");
        return;
    };

    let Ok(mut runtime) = create_runtime(
        "auto".to_owned(),
        false,
        "/tmp/thyou-live-probe-cache.json".to_owned(),
    ) else {
        emit("probe=runtime_init_failure");
        return;
    };

    let Ok(status) = runtime
        .login(username, password, None, false, false, false)
        .await
    else {
        emit("probe=primary_login_failure");
        return;
    };
    if status.second_factor_methods.is_empty() {
        emit("probe=no_second_factor");
        return;
    }

    let method = status
        .second_factor_methods
        .iter()
        .find(|method| method.as_str() == "mobile")
        .or_else(|| {
            status
                .second_factor_methods
                .iter()
                .find(|method| method.as_str() == "sms")
        })
        .cloned()
        .unwrap_or_else(|| status.second_factor_methods[0].clone());

    if let Err(error) = runtime.send_second_factor_code(method.clone()).await {
        emit(&format!(
            "probe=second_factor_request=error category={}",
            error_category(&error)
        ));
        return;
    }
    emit("probe=ready_for_code");

    let Some(code) = read_line(&mut reader) else {
        emit("probe=missing_code");
        return;
    };
    match runtime.complete_second_factor(method, code).await {
        Ok(status) => emit(&format!(
            "probe=completed state={} error_present={} available_services={}",
            status.state,
            status.error.is_some(),
            available_service_ids(&runtime)
        )),
        Err(error) => emit(&format!(
            "probe=completion_error category={}",
            error_category(&error)
        )),
    }
}
