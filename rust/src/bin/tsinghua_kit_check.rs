//! Explicitly authorized terminal entry point. Interactive by default; an
//! opt-in environment mode consumes credentials before any runtime threads.
//! Credentials are never accepted in argv, redirected stdin or session files.
#[path = "terminal_check/env_auth.rs"]
mod env_auth;
use env_auth::EnvironmentCredentials;
use std::{
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use tsinghua_kit::{
    api::runtime::{
        cli_validation::{self, CheckSpec, CheckStatus, ReportWriter, UserPrompts},
        create_backend_validation_runtime,
    },
    telemetry::{self, LogConfig, LogSession},
};

struct Options {
    output: PathBuf,
    log: String,
    graduate: bool,
    usereg: bool,
    skip_usereg: bool,
    network_only: bool,
    plan: bool,
    status: bool,
    cases: Vec<String>,
    retry_report: Option<PathBuf>,
    credentials_env: bool,
    yes: bool,
    trust_device: bool,
    allow_target_password: bool,
    network: cli_validation::NetworkEnvironment,
    execution: cli_validation::ExecutionOptions,
}
fn usage() {
    println!(
        "调度与测速：\n  --mode app          默认：与 Flutter 相同的共享 Runtime 并发入队、优先级与依赖调度\n  --mode serial       单任务基线；不自动重复真实请求做 A/B 测试\n  --concurrency N     同时提交的就绪任务数，1–8，默认 4（Runtime 互斥执行上限仍为 1）\n输出 report.json/report.md/timings.csv；记录排队、执行、人工输入、HTTP、正文、解析、缓存和认证阶段。\n"
    );
    println!(
        "环境变量登录（明确授权才运行）：\n  --credentials-env --yes  从 THYOU_USERNAME / THYOU_PASSWORD 读取本轮凭证\n  --trust-device           另行授权登记当前终端设备；已信任设备通常不需要\n  --allow-target-password  另行授权校园卡目标表单使用一次密码\n需要验证码或其他交互时停止，不自动发码；环境变量不会传给构建工具。\n"
    );
    println!(
        "离线凭证生命周期回归：./backend-check.command --lifecycle [--plan|--status|--current]\n该模式不登录、不读取 App 凭证；下面的真实验收不替代生命周期回归。\n"
    );
    println!(
        "TsinghuaKit 终端真实只读验收\n\n用法：cargo run --manifest-path rust/Cargo.toml --features terminal-check --bin tsinghua-kit-check -- [选项]\n  --plan              只显示测试计划，不登录\n  --status            显示最近一轮脱敏报告，不登录\n  --case NAME         仅测试指定项（可重复，自动包含依赖）\n  --graduate          学号无法按 Reference 规则判断时优先研究生；有效数字学号按第 5 位自动判断\n  --network-only      只检查网络自助、校园网及必要认证依赖\n  --include-usereg    复验时补入网络自助未通过/未选择项\n  --skip-usereg       本轮不做网络自助交互，明确记为未验证\n  --log FILTER        info/debug/trace 或 debug,http=trace,auth=info\n  --output DIR        私有日志/报告根目录，默认 .local/backend-check\n\n每次手动确认后建立新的内存会话；本轮每项最多一次，不恢复 Cookie。\n默认交互验收会询问网络自助账号、密码与图片验证码。\n验证码方式由真实挑战提供，发送必须再次由你确认。校园网分别检查请求出口和本机 IPv4；不会自动登录校园网或断开设备。\n"
    );
    println!(
        "  --retry-failed FILE  读取旧 report.json，仅复验未通过项及本轮依赖，可与 --plan 合用。\n\n同一项目的终端设备标识保存在 .local/backend-check-device，与 --output 无关。\n设备信任由你明确同意；学校确认登记后才显示成功，不自动删除旧设备。"
    );
    println!(
        "\n  --off-campus        显式校外模式：校园网出口状态保留为未验证，不发起请求，验收仍不完整；其余服务照常验收。\n不能与 --case tunet_status 合用；需要主动探测校园网出口时省略 --off-campus。"
    );
}
fn parse() -> Result<Option<Options>, &'static str> {
    let mut o = Options {
        output: PathBuf::from(".local/backend-check"),
        log: "debug,http=trace,auth=debug".into(),
        graduate: false,
        usereg: false,
        skip_usereg: false,
        network_only: false,
        plan: false,
        status: false,
        cases: vec![],
        retry_report: None,
        credentials_env: false,
        yes: false,
        trust_device: false,
        allow_target_password: false,
        network: cli_validation::NetworkEnvironment::Unspecified,
        execution: cli_validation::ExecutionOptions::default(),
    };
    let mut concurrency_explicit = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                usage();
                return Ok(None);
            }
            "--plan" => o.plan = true,
            "--status" => o.status = true,
            "--graduate" => o.graduate = true,
            "--include-usereg" => o.usereg = true,
            "--skip-usereg" => o.skip_usereg = true,
            "--network-only" => o.network_only = true,
            "--credentials-env" => o.credentials_env = true,
            "--yes" => o.yes = true,
            "--trust-device" => o.trust_device = true,
            "--allow-target-password" => o.allow_target_password = true,
            "--off-campus" => o.network = cli_validation::NetworkEnvironment::OffCampus,
            "--mode" => {
                o.execution.mode = match args.next().as_deref() {
                    Some("app") => cli_validation::ExecutionMode::App,
                    Some("serial") => cli_validation::ExecutionMode::Serial,
                    _ => return Err("invalid_execution_mode"),
                };
            }
            "--concurrency" => {
                o.execution.concurrency = args
                    .next()
                    .ok_or("missing_concurrency")?
                    .parse()
                    .map_err(|_| "invalid_concurrency")?;
                concurrency_explicit = true;
            }
            "--output" => o.output = PathBuf::from(args.next().ok_or("missing_output")?),
            "--log" => o.log = args.next().ok_or("missing_log_filter")?,
            "--case" => o.cases.push(args.next().ok_or("missing_case")?),
            "--retry-failed" => {
                o.retry_report = Some(PathBuf::from(args.next().ok_or("missing_retry_report")?))
            }
            _ => return Err("unknown_option"),
        }
    }
    if o.execution.mode == cli_validation::ExecutionMode::Serial && !concurrency_explicit {
        o.execution.concurrency = 1;
    }
    o.execution
        .validate()
        .map_err(|_| "invalid_execution_concurrency")?;
    if o.plan && o.status {
        return Err("conflicting_mode");
    }
    if (o.network_only && (!o.cases.is_empty() || o.retry_report.is_some() || o.status))
        || (o.skip_usereg
            && (o.usereg
                || o.network_only
                || o.cases.iter().any(|case| case.starts_with("usereg_"))))
    {
        return Err("conflicting_network_selection");
    }
    if o.network_only {
        o.usereg = true;
    }
    if o.retry_report.is_some() && (!o.cases.is_empty() || o.status) {
        return Err("conflicting_retry_mode");
    }
    if o.network == cli_validation::NetworkEnvironment::OffCampus
        && (o
            .cases
            .iter()
            .any(|case| matches!(case.as_str(), "tunet_status" | "tunet_local_status"))
            || o.status)
    {
        return Err("conflicting_off_campus_mode");
    }
    if !o.credentials_env && (o.yes || o.trust_device || o.allow_target_password) {
        return Err("environment_mode_required");
    }
    if o.credentials_env && !(o.plan || o.status) && !o.yes {
        return Err("environment_confirmation_required");
    }
    if o.credentials_env && o.usereg && !(o.plan || o.status) {
        return Err("environment_auth_interaction_required");
    }
    if !o.output.is_absolute() {
        o.output = std::env::current_dir()
            .map_err(|_| "working_directory_unavailable")?
            .join(o.output);
    }
    LogConfig::parse(&o.log, false).map_err(|_| "invalid_log_filter")?;
    cli_validation::select_cases(&o.cases).map_err(|_| "unknown_case")?;
    Ok(Some(o))
}

struct Console {
    directory: PathBuf,
    captcha: Option<PathBuf>,
    credentials: Option<EnvironmentCredentials>,
}

/// Disable echo before the visible prompt, not only before the first read.
/// This closes the small prompt/paste race while retaining rpassword's TTY
/// reader. The owned terminal and saved attributes are restored on unwind.
#[cfg(unix)]
struct PromptEchoGuard {
    terminal: std::fs::File,
    original: libc::termios,
}
#[cfg(unix)]
impl PromptEchoGuard {
    fn new() -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let terminal = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes the provided termios on a successful
        // return; the open file owns a live terminal fd throughout this guard.
        if unsafe { libc::tcgetattr(terminal.as_raw_fd(), original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ECHONL);
        // SAFETY: both pointers reference initialized termios values and the
        // owned file descriptor is valid until Drop completes.
        if unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, &hidden) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { terminal, original })
    }
}
#[cfg(unix)]
impl Drop for PromptEchoGuard {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: restore the saved attributes to the still-owned terminal.
        let _ =
            unsafe { libc::tcsetattr(self.terminal.as_raw_fd(), libc::TCSANOW, &self.original) };
    }
}

impl UserPrompts for Console {
    fn timing(&mut self, _spec: &CheckSpec, timing: &cli_validation::CaseTiming) {
        use tsinghua_kit::telemetry::timing::Phase;
        println!(
            "  排队 {:.1}ms · 执行 {:.1}ms · 人工 {:.1}ms · 限流 {:.1}ms · 响应头累计 {:.1}ms · 正文累计 {:.1}ms · {} 次 HTTP",
            timing.queue_wait_us as f64 / 1000.0,
            timing.execution_us as f64 / 1000.0,
            timing.user_input_us as f64 / 1000.0,
            timing.detail.phase_us(Phase::RateLimit) as f64 / 1000.0,
            timing.detail.phase_us(Phase::ResponseHeaders) as f64 / 1000.0,
            timing.detail.phase_us(Phase::ResponseBody) as f64 / 1000.0,
            timing.detail.requests
        );
    }
    fn device_trust_status(&mut self, status: &str) {
        println!(
            "{}",
            match status {
                "saved" => "设备信任：学校已确认登记；以后复用同一终端标识，不保存登录会话。",
                "limit_reached" =>
                    "设备信任：学校提示设备数量达到上限，未新增；没有自动删除其他设备。",
                "rejected" => "设备信任：学校拒绝登记，本次登录仍按实际业务证明继续。",
                _ => "设备信任：登记结果未确认；本次不会重试登记，下次仍可能需要验证码。",
            }
        );
    }
    fn secret(&mut self, label: &'static str) -> Result<String, String> {
        if let Some(credentials) = self.credentials.as_mut() {
            return credentials.secret(label);
        }
        #[cfg(unix)]
        let _echo_guard =
            PromptEchoGuard::new().map_err(|_| "terminal_echo_control_unavailable")?;
        let value = zeroize::Zeroizing::new(
            rpassword::prompt_password(label).map_err(|_| "terminal_input_unavailable")?,
        );
        if value.len() > 4096 || value.contains(['\n', '\r', '\0']) {
            return Err("invalid_terminal_input".into());
        }
        Ok(value.to_string())
    }
    fn confirm(&mut self, label: &'static str) -> Result<bool, String> {
        if let Some(credentials) = self.credentials.as_ref() {
            return credentials.confirm(label);
        }
        print!("{label} [y/N] ");
        io::stdout().flush().map_err(|_| "terminal_unavailable")?;
        let mut line = String::new();
        if io::stdin()
            .read_line(&mut line)
            .map_err(|_| "terminal_input_unavailable")?
            == 0
        {
            return Err("input_closed".into());
        }
        Ok(matches!(line.trim(), "y" | "Y" | "yes" | "YES"))
    }
    fn choose_factor(&mut self, methods: &[String]) -> Result<String, String> {
        if self.credentials.is_some() {
            eprintln!("学校要求二次认证；环境变量模式已停止，没有自动发送或提交验证码。");
            return Err("environment_auth_interaction_required".into());
        }
        let allowed = methods
            .iter()
            .filter(|m| matches!(m.as_str(), "wechat" | "mobile" | "sms" | "totp"))
            .collect::<Vec<_>>();
        if allowed.len() != methods.len() || allowed.is_empty() {
            return Err("factor_method_unavailable".into());
        }
        println!("需要二次认证。当前服务提供：");
        for (i, m) in allowed.iter().enumerate() {
            let name = match m.as_str() {
                "wechat" => "企业微信",
                "mobile" | "sms" => "短信",
                "totp" => "动态口令 TOTP",
                _ => unreachable!(),
            };
            println!("  {}. {}", i + 1, name);
        }
        print!("选择方式（1-{}；留空取消）：", allowed.len());
        io::stdout().flush().map_err(|_| "terminal_unavailable")?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|_| "terminal_input_unavailable")?;
        let i = input
            .trim()
            .parse::<usize>()
            .map_err(|_| "user_cancelled_factor")?;
        allowed
            .get(i.checked_sub(1).ok_or("factor_selection_invalid")?)
            .map(|s| (*s).clone())
            .ok_or_else(|| "factor_selection_invalid".into())
    }
    fn show_captcha(&mut self, bytes: &[u8], content_type: &str) -> Result<(), String> {
        if self.credentials.is_some() {
            return Err("environment_auth_interaction_required".into());
        }
        self.clear_captcha();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let suffix = match mime.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => return Err("captcha_type_unsupported".into()),
        };
        if bytes.is_empty() || bytes.len() > 2 * 1024 * 1024 {
            return Err("captcha_size_invalid".into());
        }
        if !self.confirm("网络自助需要图片验证码。是否将仅本次临时图片交给系统图片查看器？图片在完成后删除，不进入日志。")?{return Err("captcha_view_declined".into())}
        let path = self.directory.join(format!(
            ".captcha-{}.{}",
            uuid::Uuid::new_v4().simple(),
            suffix
        ));
        let mut file =
            telemetry::new_private_file(&path).map_err(|_| "captcha_file_unavailable")?;
        self.captcha = Some(path.clone());
        file.write_all(bytes)
            .map_err(|_| "captcha_file_unavailable")?;
        drop(file);
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("open").arg(&path).status();
        #[cfg(target_os = "linux")]
        let result = std::process::Command::new("xdg-open").arg(&path).status();
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let result: io::Result<std::process::ExitStatus> =
            Err(io::Error::other("captcha_viewer_unavailable"));
        if !result.is_ok_and(|status| status.success()) {
            return Err("captcha_viewer_unavailable".into());
        }
        Ok(())
    }
    fn clear_captcha(&mut self) {
        if let Some(path) = self.captcha.take() {
            let _ = fs::remove_file(path);
        }
    }
    fn progress(&mut self, spec: &CheckSpec, status: CheckStatus, reason: &str) {
        let label = match status {
            CheckStatus::Running => "进行中",
            CheckStatus::Passed => "通过",
            CheckStatus::Failed => "失败",
            CheckStatus::Blocked => "依赖阻塞",
            CheckStatus::Skipped => "跳过",
            CheckStatus::Unverified => "未验证",
            CheckStatus::Interrupted => "中断",
            CheckStatus::Pending => "未开始",
        };
        println!("[{label}][{}] {} — {}", spec.service, spec.label, reason);
        if reason == "tunet_address_mismatch" {
            println!(
                "  校园网返回的在线地址与本机活动物理网卡 IPv4 不同；NAT 或其他设备会话可能造成此情况。本项未证明，未自动换 IP 或执行校园网登录。"
            );
        }
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        self.clear_captcha();
    }
}

fn status(root: &Path) -> Result<(), String> {
    if !root.exists() {
        println!("尚无真实验收报告。");
        return Ok(());
    }
    let mut paths = fs::read_dir(root)
        .map_err(|_| "report_directory_unreadable")?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("run-")
                && e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink())
        })
        .map(|e| e.path().join("report.json"))
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    paths.sort();
    let Some(path) = paths.last() else {
        println!("尚无真实验收报告。");
        return Ok(());
    };
    if fs::symlink_metadata(path)
        .map_err(|_| "report_unreadable")?
        .file_type()
        .is_symlink()
    {
        return Err("unsafe_report_path".into());
    }
    let data = fs::read(path).map_err(|_| "report_unreadable")?;
    if data.len() > 1024 * 1024 {
        return Err("report_too_large".into());
    }
    let r: cli_validation::CheckReport =
        serde_json::from_slice(&data).map_err(|_| "report_invalid")?;
    if r.schema != 1
        || r.cases
            .keys()
            .any(|k| !cli_validation::CHECKS.iter().any(|s| s.id == k))
    {
        return Err("report_invalid".into());
    }
    println!("最近报告：{}", path.display());
    println!(
        "日志完整性：{}",
        match r.logging_complete {
            Some(true) => "已刷新且未检测到写入失败",
            Some(false) => "写入不完整",
            None => "尚未确认",
        }
    );
    for spec in cli_validation::CHECKS {
        if let Some(row) = r.cases.get(spec.id) {
            println!(
                "[{}] {} — {} ms / {} requests",
                row.status.key(),
                spec.label,
                row.duration_ms,
                row.requests
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    // Panic payloads from dependencies may contain request data; do not print
    // arguments, response bodies or a raw panic to a terminal/report.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("[ERROR][validation] verifier_panicked; no automatic retry")
    }));
    match entry() {
        Ok(code) => ExitCode::from(code),
        Err(reason) => {
            if reason == "conflicting_off_campus_mode" {
                eprintln!(
                    "--off-campus 不能与 --case tunet_status 或 --status 合用；检查校园网出口时请省略 --off-campus。"
                );
            }
            if matches!(
                reason.as_str(),
                "environment_credentials_invalid"
                    | "environment_credentials_missing"
                    | "environment_mode_required"
                    | "environment_confirmation_required"
                    | "environment_auth_interaction_required"
            ) {
                eprintln!(
                    "环境变量验收未启动：{reason}；需要 --credentials-env --yes 以及有效的 THYOU_USERNAME / THYOU_PASSWORD。"
                );
            }
            eprintln!(
                "验证器安全停止：输入、配置或输出写入失败。没有自动重试；检查已有日志和 report.json。"
            );
            ExitCode::from(2)
        }
    }
}
fn entry() -> Result<u8, String> {
    let Some(mut o) = parse().map_err(str::to_owned)? else {
        return Ok(0);
    };
    let selected = if let Some(path) = &o.retry_report {
        let (selected, graduate) = match cli_validation::retry_plan_including_usereg(path, o.usereg)
        {
            Ok(plan) => plan,
            Err(reason) if reason == "retry_report_has_no_unfinished_cases" => {
                println!(
                    "旧报告所含项目均已通过；目录中未选择的功能不在此结论内。没有开始新的登录或测试。"
                );
                return Ok(0);
            }
            Err(reason) => return Err(reason),
        };
        if o.graduate && !graduate {
            return Err("retry_profile_conflict".into());
        }
        o.graduate = graduate;
        println!("失败复验：只选取未通过项及本轮必要依赖；历史通过记录不当作当前会话证明。");
        selected
    } else if o.network_only {
        cli_validation::network_service_cases()?
    } else {
        cli_validation::select_cases(&o.cases)?
    };
    // Selecting USEREG in an interactive plan means actually asking for its
    // credentials/captcha. Do not silently mark it optional and omit it.
    if !o.skip_usereg && !o.credentials_env && selected.iter().any(|id| id.starts_with("usereg_")) {
        o.usereg = true;
    }
    if o.plan {
        println!("测试计划（不会登录、发送验证码或请求校园接口）：");
        let policy = cli_validation::ExecutionSummary::new(o.execution);
        println!(
            "HTTP 普通读取派发上限 {}；固定请求间隔 {} ms；认证与一次性票据独占，遵守服务器 Retry-After。",
            policy.http_read_parallelism_limit, policy.fixed_request_gap_ms
        );
        println!(
            "调度模式：{:?}；就绪任务并发入队上限 {}；共享 Runtime 同时执行上限 1；内部独立数据源保持 Rust 并发。",
            o.execution.mode, o.execution.concurrency
        );
        for c in cli_validation::CHECKS {
            if selected.contains(c.id) {
                if let Some(reason) = o.network.skip_reason(c.id) {
                    println!(
                        "{} | {} | {} | unverified: {}（校外未验证，不发起请求）",
                        c.id, c.service, c.label, reason
                    );
                } else if c.service == "usereg" && !o.usereg {
                    println!(
                        "{} | {} | {} | unverified: optional_login_not_selected（本轮未启用独立交互）",
                        c.id, c.service, c.label
                    );
                } else {
                    println!("{} | {} | {}", c.id, c.service, c.label);
                }
            }
        }
        println!(
            "每个接口族读取有限真实样本；普通请求不强制休眠；有界派发并遵守服务器退避。研究生考试读取已确认学期的研究生教学日历；未验证项不会作为通过，旧跳过项不会从复验中消失。"
        );
        return Ok(0);
    }
    if o.status {
        status(&o.output)?;
        return Ok(0);
    }
    if selected.iter().all(|id| {
        o.network.skip_reason(id).is_some()
            || (!o.usereg
                && (id.starts_with("usereg_")
                    || (matches!(id.as_str(), "identity_session" | "portal_bootstrap")
                        && selected.iter().any(|case| case.starts_with("usereg_")))))
    }) {
        println!(
            "此计划只有网络环境或独立交互尚未满足的项目；验收仍不完整，没有启动登录或网络请求，也没有接口通过验证。"
        );
        return Ok(3);
    }
    if !o.credentials_env && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        eprintln!("必须由用户在交互终端运行；拒绝管道输入、密码参数和无人值守登录。");
        return Ok(2);
    }
    if o.credentials_env && o.usereg {
        return Err("environment_auth_interaction_required".into());
    }
    let credentials = if o.credentials_env {
        // SAFETY: synchronous entry before the logger and Tokio are created;
        // no runtime threads or spawned environment readers exist yet.
        Some(
            unsafe {
                EnvironmentCredentials::take_from_process(o.trust_device, o.allow_target_password)
            }
            .map_err(str::to_owned)?,
        )
    } else {
        None
    };
    telemetry::private_dir(&o.output).map_err(|_| "output_directory_not_private")?;
    // Lock identity follows this verifier executable, not --output: choosing
    // another report directory must not create a parallel real-test process.
    let executable = std::env::current_exe().map_err(|_| "executable_path_unavailable")?;
    let lock_path = executable
        .parent()
        .ok_or("executable_path_unavailable")?
        .join("tsinghua-kit-check.run.lock");
    let lock = if lock_path.exists() {
        let meta = fs::symlink_metadata(&lock_path).map_err(|_| "lock_unavailable")?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("unsafe_lock_path".into());
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|_| "lock_unavailable")?
    } else {
        telemetry::new_private_file(&lock_path).map_err(|_| "lock_unavailable")?
    };
    if fs2::FileExt::try_lock_exclusive(&lock).is_err() {
        eprintln!("已有真实验收进程在运行；未开始任何请求。");
        return Ok(2);
    }
    let mut console = Console {
        directory: o.output.clone(),
        captcha: None,
        credentials,
    };
    println!(
        "TsinghuaKit 后端真实只读验收\n这是新一轮用户主动验收：不会导入 App 会话，不自动重登，不进行支付/预约/选退课/断网。旧报告不会冒充本次通过。"
    );
    if !console.confirm("确认开始这份计划并在终端交互登录？")? {
        return Ok(0);
    }
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let run_dir = o.output.join(format!(
        "run-{}-{run_id}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    telemetry::private_dir(&run_dir).map_err(|_| "report_directory_unavailable")?;
    console.directory = run_dir.clone();
    let mut logging = LogSession::start(
        &run_dir.join("logs"),
        LogConfig::parse(&o.log, false).map_err(str::to_owned)?,
    )
    .map_err(|_| "log_init_failed")?;
    tracing::dispatcher::set_global_default(logging.dispatch.clone())
        .map_err(|_| "logger_already_configured")?;
    let mut report = ReportWriter::new(
        run_dir.join("report.json"),
        logging.run_id.clone(),
        &selected,
        o.graduate,
    )?;
    report.set_network_environment(o.network)?;
    if let Some(path) = &o.retry_report {
        report.set_previous_report(path)?;
    }
    println!(
        "脱敏日志：{}\n结果报告：{}",
        logging.directory.display(),
        run_dir.join("report.json").display()
    );
    tracing::info!(target:"tsinghua_kit::validation",event="run_started",total=selected.len() as u64);
    let mut runtime = create_backend_validation_runtime(
        "auto".into(),
        o.graduate,
        std::env::current_dir()
            .map_err(|_| "terminal_workdir_unavailable")?
            .join(".local")
            .join("backend-check-device")
            .join("unused-cache.json")
            .to_string_lossy()
            .into_owned(),
    )?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "executor_unavailable")?;
    let result=executor.block_on(async {
        tokio::select! {
            biased;
            _=tokio::signal::ctrl_c()=>Err("user_interrupted".to_owned()),
            result=Box::pin(cli_validation::run_with_options(&mut runtime,&mut console,&mut report,o.usereg,o.execution))=>result,
        }
    });
    console.clear_captcha();
    if result.is_err() {
        report.interrupted()?;
    }
    let mut code = if result
        .as_ref()
        .err()
        .is_some_and(|reason| reason == "user_interrupted")
    {
        130
    } else if result.is_err() {
        2
    } else {
        report.exit_code()
    };
    if result.is_err() {
        tracing::error!(target:"tsinghua_kit::validation",event="run_stopped",reason="interrupted_or_output_failure");
    }
    tracing::info!(target:"tsinghua_kit::validation",event="run_finished",outcome=if code==0{"completed"}else{"incomplete"},request_count=report.report.request_count);
    logging.flush();
    if !logging.healthy() {
        code = 2;
        eprintln!("日志写入不完整，退出码 2；报告不等于完整诊断证据。");
    }
    report.report.logging_complete = Some(logging.healthy());
    report.finish()?;
    cli_validation::write_markdown(&report.report, &run_dir.join("report.md"))?;
    cli_validation::write_timings_csv(&report.report, &run_dir.join("timings.csv"))?;
    if let Some(summary) = &report.report.execution {
        println!(
            "测速：总墙钟 {:.3}s，人工交互 {:.3}s，排除交互 {:.3}s；入队峰值 {}，Runtime 执行峰值 {}。",
            summary.total_wall_us as f64 / 1_000_000.0,
            summary.user_input_us as f64 / 1_000_000.0,
            summary.active_wall_us as f64 / 1_000_000.0,
            summary.max_submitted,
            summary.max_runtime_executing
        );
        println!(
            "报告检查点：{} 次写入，{:.3}ms（已包含在总墙钟）。",
            summary.checkpoint_write_count,
            summary.checkpoint_write_us as f64 / 1000.0
        );
    }
    let passed = report
        .report
        .cases
        .values()
        .filter(|r| r.status == CheckStatus::Passed)
        .count();
    let skipped = report
        .report
        .cases
        .values()
        .filter(|r| matches!(r.status, CheckStatus::Skipped | CheckStatus::Unverified))
        .count();
    println!(
        "\n定向验收结束：{} 本轮通过，{} 未验证/旧跳过，其余及未选择目录项请查看报告。退出码 {}。本结果不代表全部功能已验证。\n{}\n进程退出后本次内存会话不可恢复。",
        passed,
        skipped,
        code,
        run_dir.join("report.md").display()
    );
    drop(runtime);
    drop(lock);
    Ok(code)
}
