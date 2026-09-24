//! Explicit, one-run environment authentication. No Debug implementation,
//! persistence, shell subprocess, automatic MFA send or primary-login retry.
use zeroize::Zeroizing;

pub(super) const USERNAME_VARIABLE: &str = "THYOU_USERNAME";
pub(super) const PASSWORD_VARIABLE: &str = "THYOU_PASSWORD";

pub(super) struct EnvironmentCredentials {
    username: Option<Zeroizing<String>>,
    password: Option<Zeroizing<String>>,
    target_password: Option<Zeroizing<String>>,
    trust_device: bool,
}

impl EnvironmentCredentials {
    pub(super) fn new(
        username: String,
        password: String,
        trust_device: bool,
        allow_target_password: bool,
    ) -> Result<Self, &'static str> {
        let username = Zeroizing::new(username);
        let password = Zeroizing::new(password);
        if username.trim().is_empty()
            || username.len() > 128
            || username.chars().any(char::is_control)
            || password.is_empty()
            || password.len() > 4096
            || password.contains(['\n', '\r', '\0'])
        {
            return Err("environment_credentials_invalid");
        }
        let target_password = allow_target_password.then(|| Zeroizing::new(password.to_string()));
        Ok(Self {
            username: Some(username),
            password: Some(password),
            target_password,
            trust_device,
        })
    }

    /// The caller must be the synchronous entry point, before runtime/logger
    /// threads or other environment readers exist. Never call from a test thread.
    pub(super) unsafe fn take_from_process(
        trust_device: bool,
        allow_target_password: bool,
    ) -> Result<Self, &'static str> {
        let username = std::env::var(USERNAME_VARIABLE);
        let password = std::env::var(PASSWORD_VARIABLE);
        // SAFETY: the caller enforces single-threaded early process startup.
        unsafe {
            std::env::remove_var(USERNAME_VARIABLE);
            std::env::remove_var(PASSWORD_VARIABLE);
        }
        match (username, password) {
            (Ok(username), Ok(password)) => {
                Self::new(username, password, trust_device, allow_target_password)
            }
            (username, password) => {
                // Zeroize any partially supplied UTF-8 secret before rejecting.
                drop(username.ok().map(Zeroizing::new));
                drop(password.ok().map(Zeroizing::new));
                Err("environment_credentials_missing")
            }
        }
    }

    pub(super) fn secret(&mut self, label: &'static str) -> Result<String, String> {
        let slot = match label {
            "校园账号（不回显）：" => &mut self.username,
            "校园密码（不回显）：" => &mut self.password,
            "当前校园账号密码（仅用于这次校园卡认证，不回显；留空取消）：" => {
                &mut self.target_password
            }
            _ => return Err("environment_auth_interaction_required".into()),
        };
        slot.take()
            .map(|value| value.to_string())
            .ok_or_else(|| "environment_credential_already_consumed".into())
    }

    pub(super) fn confirm(&self, label: &'static str) -> Result<bool, String> {
        match label {
            "确认开始这份计划并在终端交互登录？" => Ok(true),
            "信任此终端设备以减少重复验证？仅保存随机设备标识，不保存密码、验证码或 Cookie；学校仍可要求验证。" => {
                Ok(self.trust_device)
            }
            "校园卡目标页要求当前账号密码。是否仅为本次校园卡认证输入一次？不会保存或重新登录主账号。" => {
                Ok(self.target_password.is_some())
            }
            // In particular, --yes NEVER means consent to send an MFA code.
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_run123904_environment_credentials_are_one_shot() {
        let mut value = EnvironmentCredentials::new(
            "fixture-user".into(),
            "synthetic password with spaces".into(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            value.secret("校园账号（不回显）：").unwrap(),
            "fixture-user"
        );
        assert_eq!(
            value.secret("校园密码（不回显）：").unwrap(),
            "synthetic password with spaces"
        );
        assert!(value.secret("校园密码（不回显）：").is_err());
        assert!(
            value
                .secret("请输入当前验证码（不回显，留空取消）：")
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run123904_environment_confirmation_does_not_authorize_factor_send() {
        let value =
            EnvironmentCredentials::new("fixture-user".into(), "synthetic".into(), false, false)
                .unwrap();
        assert!(value.confirm("确认开始这份计划并在终端交互登录？").unwrap());
        assert!(
            !value
                .confirm("发送一次验证码？选择否可输入你已收到的验证码；不会自动重发。")
                .unwrap()
        );
        assert!(!value.confirm("信任此终端设备以减少重复验证？仅保存随机设备标识，不保存密码、验证码或 Cookie；学校仍可要求验证。").unwrap());
        assert!(!value.confirm("校园卡目标页要求当前账号密码。是否仅为本次校园卡认证输入一次？不会保存或重新登录主账号。").unwrap());
    }
    #[test]
    fn backend_repair_run123904_target_password_requires_separate_opt_in() {
        let mut value =
            EnvironmentCredentials::new("fixture-user".into(), "synthetic".into(), true, true)
                .unwrap();
        assert!(value.confirm("信任此终端设备以减少重复验证？仅保存随机设备标识，不保存密码、验证码或 Cookie；学校仍可要求验证。").unwrap());
        assert_eq!(value.secret("校园密码（不回显）：").unwrap(), "synthetic");
        assert_eq!(
            value
                .secret("当前校园账号密码（仅用于这次校园卡认证，不回显；留空取消）：")
                .unwrap(),
            "synthetic"
        );
        assert!(
            value
                .secret("当前校园账号密码（仅用于这次校园卡认证，不回显；留空取消）：")
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run123904_invalid_environment_input_is_rejected_without_echo() {
        for (username, password) in [
            ("", "secret"),
            ("fixture-user", ""),
            ("user\nname", "synthetic"),
            ("fixture-user", "secret\nvalue"),
        ] {
            let err = EnvironmentCredentials::new(username.into(), password.into(), false, false)
                .err()
                .unwrap();
            assert_eq!(err, "environment_credentials_invalid");
            assert!(!err.contains("secret"));
        }
    }
}
