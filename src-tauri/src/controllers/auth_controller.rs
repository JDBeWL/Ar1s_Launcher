//! 认证 command 模块
//!
//! 离线身份与 Microsoft 两种登录流程的 invoke 入口；待用的 device_code 与授权码流程参数（verifier + state）存在进程内 Mutex

use crate::errors::LauncherError;
use crate::models::AuthType;
use crate::services::config::{
    acquire_auth_lock, load_config, reload_config_from_disk, update_config, update_config_async,
};
use crate::services::microsoft_auth;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

static PENDING_DEVICE_CODE: Mutex<Option<String>> = Mutex::new(None);

static AUTH_CANCELLED: AtomicBool = AtomicBool::new(false);

fn reset_auth_cancel() {
    AUTH_CANCELLED.store(false, Ordering::SeqCst);
}

async fn wait_for_auth_cancel() {
    while !AUTH_CANCELLED.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[tauri::command]
pub fn cancel_microsoft_login() {
    AUTH_CANCELLED.store(true, Ordering::SeqCst);
}

/// verifier 和 state 必须同生同灭，拆成两个 static 会失同步
struct PendingAuthCodeFlow {
    code_verifier: String,
    state: String,
}

static PENDING_AUTH_FLOW: Mutex<Option<PendingAuthCodeFlow>> = Mutex::new(None);

#[tauri::command]
pub fn get_saved_username() -> Result<Option<String>, LauncherError> {
    crate::services::config::get_saved_username()
}

#[tauri::command]
pub async fn set_saved_username(username: String) -> Result<(), LauncherError> {
    crate::controllers::run_blocking(move || crate::services::config::set_saved_username(username))
        .await
}

#[tauri::command]
pub fn get_saved_uuid() -> Result<Option<String>, LauncherError> {
    crate::services::config::get_saved_uuid()
}

#[tauri::command]
pub async fn set_saved_uuid(uuid: String) -> Result<(), LauncherError> {
    crate::controllers::run_blocking(move || crate::services::config::set_saved_uuid(uuid)).await
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub auth_type: String,
    pub logged_in: bool,
    pub username: Option<String>,
    pub uuid: Option<String>,
    pub skin_url: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCodeDisplay {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftLoginResult {
    pub username: String,
    pub uuid: String,
    pub skin_url: Option<String>,
    pub expires_at: i64,
}

#[tauri::command]
pub fn get_auth_status() -> Result<AuthStatus, LauncherError> {
    let config = load_config()?;
    let auth_type = match config.auth_type {
        AuthType::Microsoft => "microsoft",
        AuthType::Offline => "offline",
    };
    let logged_in = if matches!(config.auth_type, AuthType::Microsoft) {
        config.ms_refresh_token.is_some()
    } else {
        config.username.is_some()
    };
    Ok(AuthStatus {
        auth_type: auth_type.to_string(),
        logged_in,
        username: config.username.clone(),
        uuid: config.uuid.clone(),
        // 切回离线模式后皮肤不该再显示
        skin_url: matches!(config.auth_type, AuthType::Microsoft)
            .then(|| config.skin_url.clone())
            .flatten(),
        expires_at: config.ms_expires_at,
    })
}

#[tauri::command]
pub async fn start_microsoft_login(
    window: tauri::Window,
) -> Result<DeviceCodeDisplay, LauncherError> {
    reset_auth_cancel();
    let device_code_info = microsoft_auth::start_device_code_flow().await?;

    {
        let mut pending = PENDING_DEVICE_CODE
            .lock()
            .map_err(|e| LauncherError::Custom(format!("内部锁错误: {}", e)))?;
        *pending = Some(device_code_info.device_code.clone());
    }

    let display = DeviceCodeDisplay {
        user_code: device_code_info.user_code,
        verification_uri: device_code_info.verification_uri,
        expires_in: device_code_info.expires_in,
    };

    let _ = window.emit(
        "microsoft-login-device-code",
        serde_json::json!({
            "userCode": display.user_code,
            "verificationUri": display.verification_uri,
        }),
    );

    Ok(display)
}

#[tauri::command]
pub async fn complete_microsoft_login() -> Result<MicrosoftLoginResult, LauncherError> {
    let device_code = {
        let mut pending = PENDING_DEVICE_CODE
            .lock()
            .map_err(|e| LauncherError::Custom(format!("内部锁错误: {}", e)))?;
        pending
            .take()
            .ok_or_else(|| LauncherError::Custom("没有进行中的登录流程，请重新开始".to_string()))?
    };

    let mut interval = 5u64;
    let max_attempts = 180 / interval as u32;

    for _ in 0..max_attempts {
        let result = tokio::select! {
            result = microsoft_auth::poll_device_token(&device_code) => result,
            _ = wait_for_auth_cancel() => {
                return Err(LauncherError::Cancelled);
            }
        };

        if AUTH_CANCELLED.load(Ordering::Relaxed) {
            return Err(LauncherError::Cancelled);
        }

        match result {
            Ok(token_resp) => {
                let refresh = token_resp.refresh_token.clone().unwrap_or_default();
                let expires_in = token_resp.expires_in.unwrap_or(3600);

                let auth_result = microsoft_auth::complete_microsoft_auth(
                    &token_resp.access_token,
                    &refresh,
                    expires_in,
                )
                .await?;

                update_config_async({
                    let auth = auth_result.clone();
                    move |config| {
                        config.auth_type = AuthType::Microsoft;
                        config.username = Some(auth.username.clone());
                        config.uuid = Some(auth.uuid.clone());
                        config.skin_url = auth.skin_url.clone();
                        config.ms_access_token = Some(auth.access_token.clone());
                        config.ms_refresh_token = Some(auth.refresh_token.clone());
                        config.ms_expires_at = Some(auth.expires_at);
                        Ok(())
                    }
                })
                .await?;

                return Ok(MicrosoftLoginResult {
                    username: auth_result.username,
                    uuid: auth_result.uuid,
                    skin_url: auth_result.skin_url,
                    expires_at: auth_result.expires_at,
                });
            }
            Err(LauncherError::Custom(msg))
                if msg == "authorization_pending" || msg == "slow_down" =>
            {
                if msg == "slow_down" {
                    interval = (interval + 5).min(15);
                }
                tokio::select! {
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(interval)) => {}
                    _ = wait_for_auth_cancel() => {
                        return Err(LauncherError::Cancelled);
                    }
                }
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    Err(LauncherError::Custom("登录超时，请重新尝试".to_string()))
}

#[tauri::command]
pub async fn refresh_skin_url() -> Result<Option<String>, LauncherError> {
    let stored = {
        let config = load_config()?;
        if !matches!(config.auth_type, AuthType::Microsoft) {
            return Ok(None);
        }
        (config.ms_access_token.clone(), config.skin_url.clone())
    };

    let Some(token) = stored.0 else {
        return Ok(stored.1);
    };

    match microsoft_auth::fetch_skin_url(&token).await {
        Ok(skin_url) => {
            let saved_skin = skin_url.clone();
            update_config_async(move |config| {
                config.skin_url = saved_skin;
                Ok(())
            })
            .await?;
            Ok(skin_url)
        }
        // 皮肤地址只是装饰，取不到就退回已存的值，不该因此打断启动流程
        Err(e) => {
            log::warn!("刷新皮肤地址失败，沿用已存的值: {}", e);
            Ok(stored.1)
        }
    }
}

#[tauri::command]
pub async fn refresh_microsoft_auth() -> Result<MicrosoftLoginResult, LauncherError> {
    // 同一时刻只允许一个实例刷新；后到的实例拿到锁后会直接复用新 token
    let _auth_lock = acquire_auth_lock().await?;
    let config = reload_config_from_disk()?;

    let now = chrono::Utc::now().timestamp();
    if matches!(config.auth_type, AuthType::Microsoft)
        && config.ms_access_token.is_some()
        && config
            .ms_expires_at
            .is_some_and(|expires| expires > now + 60)
        && let (Some(username), Some(uuid)) = (config.username.clone(), config.uuid.clone())
    {
        return Ok(MicrosoftLoginResult {
            username,
            uuid,
            skin_url: config.skin_url.clone(),
            expires_at: config.ms_expires_at.unwrap_or(0),
        });
    }

    let refresh_token = config
        .ms_refresh_token
        .clone()
        .ok_or_else(|| LauncherError::Custom("未找到 refresh_token，请重新登录".to_string()))?;

    let auth_result = microsoft_auth::refresh_and_authenticate(&refresh_token).await?;

    // 网络请求期间另一个实例可能已完成登录/刷新；refresh token 变了就不要覆盖
    let applied = update_config_async({
        let auth = auth_result.clone();
        move |config| {
            if config.ms_refresh_token.as_deref() != Some(refresh_token.as_str()) {
                return Ok(false);
            }
            config.auth_type = AuthType::Microsoft;
            config.username = Some(auth.username.clone());
            config.uuid = Some(auth.uuid.clone());
            config.skin_url = auth.skin_url.clone();
            config.ms_access_token = Some(auth.access_token.clone());
            config.ms_refresh_token = Some(auth.refresh_token.clone());
            config.ms_expires_at = Some(auth.expires_at);
            Ok(true)
        }
    })
    .await?;

    if applied {
        return Ok(MicrosoftLoginResult {
            username: auth_result.username,
            uuid: auth_result.uuid,
            skin_url: auth_result.skin_url,
            expires_at: auth_result.expires_at,
        });
    }

    // 另一个实例赢了，返回它写下的最新状态
    let current = reload_config_from_disk()?;
    if current.ms_access_token.is_some()
        && let (Some(username), Some(uuid)) = (current.username.clone(), current.uuid.clone())
    {
        return Ok(MicrosoftLoginResult {
            username,
            uuid,
            skin_url: current.skin_url.clone(),
            expires_at: current.ms_expires_at.unwrap_or(0),
        });
    }

    Err(LauncherError::Custom(
        "登录状态已被另一个启动器实例修改，请重新登录".to_string(),
    ))
}

#[tauri::command]
pub async fn logout_microsoft() -> Result<(), LauncherError> {
    let _ = microsoft_auth::revoke_microsoft_token().await;

    // 清 token 也走 update_config：与另一实例的刷新在同一把配置锁下串行，不会被写回
    update_config_async(|config| {
        config.auth_type = AuthType::Offline;
        config.ms_access_token = None;
        config.ms_refresh_token = None;
        config.ms_expires_at = None;
        config.skin_url = None;
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn start_microsoft_auth_code_login() -> Result<String, LauncherError> {
    reset_auth_cancel();
    let flow_info = microsoft_auth::start_auth_code_flow()?;

    {
        let mut pending = PENDING_AUTH_FLOW
            .lock()
            .map_err(|e| LauncherError::Custom(format!("内部锁错误: {}", e)))?;
        *pending = Some(PendingAuthCodeFlow {
            code_verifier: flow_info.code_verifier,
            state: flow_info.state,
        });
    }

    Ok(flow_info.auth_url)
}

#[tauri::command]
pub async fn complete_microsoft_auth_code_login() -> Result<MicrosoftLoginResult, LauncherError> {
    let flow = {
        let mut pending = PENDING_AUTH_FLOW
            .lock()
            .map_err(|e| LauncherError::Custom(format!("内部锁错误: {}", e)))?;
        pending
            .take()
            .ok_or_else(|| LauncherError::Custom("没有进行中的登录流程，请重新开始".to_string()))?
    };

    let auth_result =
        microsoft_auth::complete_auth_code_flow(&flow.code_verifier, &flow.state, &AUTH_CANCELLED)
            .await?;

    update_config_async({
        let auth = auth_result.clone();
        move |config| {
            config.auth_type = AuthType::Microsoft;
            config.username = Some(auth.username.clone());
            config.uuid = Some(auth.uuid.clone());
            config.skin_url = auth.skin_url.clone();
            config.ms_access_token = Some(auth.access_token.clone());
            config.ms_refresh_token = Some(auth.refresh_token.clone());
            config.ms_expires_at = Some(auth.expires_at);
            Ok(())
        }
    })
    .await?;

    Ok(MicrosoftLoginResult {
        username: auth_result.username,
        uuid: auth_result.uuid,
        skin_url: auth_result.skin_url,
        expires_at: auth_result.expires_at,
    })
}

#[tauri::command]
pub async fn set_auth_type(auth_type: String) -> Result<(), LauncherError> {
    crate::controllers::run_blocking(move || {
        update_config(|config| {
            config.auth_type = match auth_type.as_str() {
                "microsoft" => AuthType::Microsoft,
                _ => AuthType::Offline,
            };
            Ok(())
        })
    })
    .await
}
