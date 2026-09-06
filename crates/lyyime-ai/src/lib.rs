//! lyyIme AI 助手共享客户端(Mode A / Mode B 共用)。
//!
//! Rust 移植自原 lyyime_ai.py(已删除;行为与 CLI 文案保持兼容):
//! * 读取 `~/.config/lyyime/config.toml` 的 `[ai]` 段并做健壮解析
//!   (toml 标准解析失败时回退逐行解析,兼容历史 C 端写入的无引号注释行);
//! * 调用 **OpenAI Chat Completions 兼容**接口完成"提示词 → 回复文本"
//!   (接口形态依据 OpenAI API Reference;DeepSeek / Moonshot / 通义千问
//!   dashscope 兼容模式 / 智谱 / SiliconFlow / Ollama / LM Studio / vLLM
//!   等均兼容该协议,故"自定义大模型"= 填 base_url + key + model);
//! * HTTP 走 ureq(默认 rustls,同时支持本地 http 服务,如 Ollama)。
//!
//! CLI 入口(`lyyime-ai` 二进制):lyyime-xim(Mode B,C 无 HTTP 依赖)以
//! 子进程方式调用,`--prompt` 回复写 stdout、失败写 stderr 退出码 1;
//! `--check` 配置体检 + 最小连通测试(设置窗"测试连接")。

use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const VERSION: &str = "0.2.0";

/// 回复文本上限(字节):防御异常服务返回超大响应拖死输入法
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT: u64 = 60;

pub const DEFAULT_SYSTEM_PROMPT: &str = "你是输入法内置的 AI 助手。直接输出用户要求的结果本体:不要解释、不要前后缀寒暄、不要 markdown 代码块标记,保持可直接上屏的纯文本。";

#[derive(Clone, Debug, PartialEq)]
pub struct AiConfig {
    pub enabled: bool,
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub system_prompt: String,
    pub timeout: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: false, // 未配置前功能完全不介入按键
            api_base: String::new(),
            api_key: String::new(),
            model: String::new(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// AI 助手调用失败(消息面向最终用户,可直接展示)。
#[derive(Debug, Clone)]
pub struct AiError(pub String);

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AiError {}

/// 共用配置文件路径(~/.config/lyyime/config.toml,XDG 可覆盖)。
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            format!("{}/.config", std::env::var("HOME").unwrap_or_else(|_| "/root".into()))
        });
    PathBuf::from(base).join("lyyime").join("config.toml")
}

/// 功能是否已启用并配好(api_base + model 必填;api_key 本地服务可空)。
pub fn is_configured(cfg: &AiConfig) -> bool {
    cfg.enabled && !cfg.api_base.trim().is_empty() && !cfg.model.trim().is_empty()
}

/// base_url → chat/completions 端点(容忍带/不带 /v1、写全端点三种写法)。
pub fn completions_url(api_base: &str) -> String {
    let base = api_base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

/// 类型与边界归一:非法值回退默认,不让手改配置炸掉输入法。
fn sanitize(mut cfg: AiConfig) -> AiConfig {
    cfg.api_base = cfg.api_base.trim().to_string();
    cfg.api_key = cfg.api_key.trim().to_string();
    cfg.model = cfg.model.trim().to_string();
    if cfg.system_prompt.trim().is_empty() {
        cfg.system_prompt = DEFAULT_SYSTEM_PROMPT.into();
    }
    cfg.timeout = cfg.timeout.clamp(5, 300);
    cfg
}

/// 单行 TOML 值解析(兜底路径用):识别带引号字符串(含转义)、
/// true/false/1/0、整数;裸值剥掉 `#` 注释 —— 历史 C 端 save 会写
/// `key = 5 注释` 这种无 # 的行,按"取到 # 或末尾"处理。
fn parse_toml_value(raw: &str) -> Option<serde_json::Value> {
    let raw = raw.trim();
    if raw.starts_with('"') {
        // 取第一对完整引号,处理 \" \\ \/ \b \f \n \r \t 转义
        let mut out = String::new();
        let mut chars = raw[1..].chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(serde_json::Value::String(out)),
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some(e) => out.push(e),
                    None => break,
                },
                c => out.push(c),
            }
        }
        return None;
    }
    let body = if raw.contains('#') {
        raw.split('#').next().unwrap_or("").trim()
    } else {
        raw
    };
    match body {
        "true" | "1" => return Some(serde_json::Value::Bool(true)),
        "false" | "0" => return Some(serde_json::Value::Bool(false)),
        _ => {}
    }
    if let Ok(n) = body.parse::<i64>() {
        return Some(serde_json::Value::Number(n.into()));
    }
    // 与原 python 版一致:裸值取到 # 或末尾,原样作为字符串(兼容历史写法)
    Some(serde_json::Value::String(body.to_string()))
}

/// 逐行兜底解析:tomllib/toml 标准解析失败时使用(历史无引号注释行等)。
/// 只认 `[ai]` 段内的已知键,顶层同名键不认,避免误伤。
fn load_config_fallback(path: &Path) -> AiConfig {
    let mut cfg = AiConfig::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return cfg;
    };
    let mut in_ai = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_ai = t.trim_matches(['[', ']']) == "ai";
            continue;
        }
        if !in_ai || t.starts_with('#') || t.is_empty() {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let k = k.trim();
        let v = v.trim();
        match k {
            "enabled" => {
                cfg.enabled = matches!(parse_toml_value(v), Some(serde_json::Value::Bool(true)))
            }
            "api_base" | "api_key" | "model" | "system_prompt" => {
                if let Some(serde_json::Value::String(s)) = parse_toml_value(v) {
                    match k {
                        "api_base" => cfg.api_base = s,
                        "api_key" => cfg.api_key = s,
                        "model" => cfg.model = s,
                        _ => cfg.system_prompt = s,
                    }
                }
            }
            "timeout" => {
                if let Some(serde_json::Value::Number(n)) = parse_toml_value(v) {
                    if let Some(n) = n.as_i64() {
                        cfg.timeout = n as u64;
                    }
                }
            }
            _ => {}
        }
    }
    sanitize(cfg)
}

/// 读取 `[ai]` 段;文件缺失/损坏/段缺失都回退默认(功能关闭),绝不 panic。
/// 先用 toml 标准解析;失败(历史无引号注释行等)再逐行兜底。
pub fn load_config_at(path: &Path) -> AiConfig {
    let mut cfg = AiConfig::default();
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(data) = toml::from_str::<toml::Value>(&text) {
            if let Some(sec) = data.get("ai").and_then(|v| v.as_table()) {
                if let Some(v) = sec.get("enabled").and_then(|v| v.as_bool()) {
                    cfg.enabled = v;
                }
                for key in ["api_base", "api_key", "model", "system_prompt"] {
                    if let Some(v) = sec.get(key).and_then(|v| v.as_str()) {
                        match key {
                            "api_base" => cfg.api_base = v.into(),
                            "api_key" => cfg.api_key = v.into(),
                            "model" => cfg.model = v.into(),
                            _ => cfg.system_prompt = v.into(),
                        }
                    }
                }
                if let Some(v) = sec.get("timeout").and_then(|v| v.as_integer()) {
                    cfg.timeout = v as u64;
                }
                return sanitize(cfg);
            }
            // 无 [ai] 段:也走兜底再确认一次(标准解析成功但段缺失时,
            // 兜底结果与默认相同,直接返回默认即可)
            return sanitize(cfg);
        }
    }
    load_config_fallback(path)
}

pub fn load_config() -> AiConfig {
    load_config_at(&config_path())
}

/// 同步调用一次对话补全,返回回复文本;失败抛 AiError(人话消息)。
pub fn chat(prompt: &str, cfg: &AiConfig, timeout: Option<u64>) -> Result<String, AiError> {
    if !is_configured(cfg) {
        return Err(AiError(format!(
            "AI 助手未配置:请在 lyyIme 设置中填写 API 地址、模型并勾选启用(配置文件:{} 的 [ai] 段)",
            config_path().display()
        )));
    }
    let url = completions_url(&cfg.api_base);
    let mut messages = Vec::new();
    if !cfg.system_prompt.is_empty() {
        messages.push(json!({"role": "system", "content": cfg.system_prompt}));
    }
    messages.push(json!({"role": "user", "content": prompt}));
    let payload = json!({"model": cfg.model, "messages": messages, "stream": false});
    let t = timeout.unwrap_or(cfg.timeout);

    let body = payload.to_string();
    let resp = ureq::post(&url)
        .timeout(Duration::from_secs(t))
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", cfg.api_key))
        .send_bytes(body.as_bytes());
    match resp {
        Ok(r) => {
            let mut body = Vec::new();
            let mut reader = r.into_reader().take((MAX_RESPONSE_BYTES + 1) as u64);
            if reader.read_to_end(&mut body).is_err() {
                return Err(AiError(format!("AI 响应读取失败({url})")));
            }
            if body.len() > MAX_RESPONSE_BYTES {
                return Err(AiError(format!(
                    "AI 回复超过 {} KB 上限,已放弃上屏",
                    MAX_RESPONSE_BYTES / 1024
                )));
            }
            let data: serde_json::Value = serde_json::from_slice(&body)
                .map_err(|_| AiError(format!("AI 服务返回的不是有效 JSON({url})")))?;
            let text = data
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .ok_or_else(|| {
                    AiError(format!(
                        "AI 服务响应缺少 choices[0].message.content 字段:{}",
                        &body.iter().take(200).map(|b| *b as char).collect::<String>()
                    ))
                })?;
            Ok(text.trim().to_string())
        }
        Err(ureq::Error::Status(code, r)) => {
            // HTTP 错误:尽量取响应体里的 error.message 给人话提示
            let mut detail = String::new();
            let mut body = Vec::new();
            if r.into_reader().take(4096).read_to_end(&mut body).is_ok() {
                if let Ok(err) = serde_json::from_slice::<serde_json::Value>(&body) {
                    detail = err
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(200)
                        .collect();
                }
            }
            Err(AiError(format!(
                "AI 服务返回 HTTP {}{}({url})",
                code,
                if detail.is_empty() { String::new() } else { format!(":{detail}") }
            )))
        }
        Err(e) => Err(AiError(format!(
            "无法连接 AI 服务 {url}({e});请检查网络或 API 地址"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_url_variants() {
        assert_eq!(
            completions_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            completions_url("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            completions_url("http://127.0.0.1:9/v1/chat/completions"),
            "http://127.0.0.1:9/v1/chat/completions"
        );
    }

    #[test]
    fn not_configured_by_default() {
        let cfg = AiConfig::default();
        assert!(!is_configured(&cfg));
        assert!(chat("hi", &cfg, Some(1)).is_err());
    }

    #[test]
    fn sanitize_clamps_timeout_and_defaults_prompt() {
        let cfg = sanitize(AiConfig { timeout: 9999, system_prompt: "  ".into(), ..Default::default() });
        assert_eq!(cfg.timeout, 300);
        assert_eq!(cfg.system_prompt, DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn load_missing_file_is_default() {
        let cfg = load_config_at(Path::new("/nonexistent/lyyime/config.toml"));
        assert_eq!(cfg, AiConfig::default());
    }

    #[test]
    fn load_standard_toml_section() {
        let dir = tempfile_dir();
        let p = dir.join("config.toml");
        std::fs::write(
            &p,
            "page_size = 5\n[ai]\nenabled = true\napi_base = \"http://127.0.0.1:9/v1\"\napi_key = \"sk-t\"\nmodel = \"m1\"\ntimeout = 7\n",
        )
        .unwrap();
        let cfg = load_config_at(&p);
        assert!(cfg.enabled);
        assert_eq!(cfg.api_base, "http://127.0.0.1:9/v1");
        assert_eq!(cfg.model, "m1");
        assert_eq!(cfg.timeout, 7);
        assert_eq!(cfg.system_prompt, DEFAULT_SYSTEM_PROMPT); // 未写用默认
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_legacy_broken_toml_falls_back_line_parser() {
        // 历史 C 端写出的 `key = 5 注释` 无 # 行:标准 toml 解析必失败
        let dir = tempfile_dir();
        let p = dir.join("config.toml");
        std::fs::write(
            &p,
            "page_size = 5 注释\n[ai]\nenabled = 1\napi_base = \"http://127.0.0.1:9/v1\"\nmodel = m1 注释\ntimeout = 9\n",
        )
        .unwrap();
        let cfg = load_config_at(&p);
        assert!(cfg.enabled);
        assert_eq!(cfg.api_base, "http://127.0.0.1:9/v1");
        // 与 python 版一致:裸值原样保留(到 # 或末尾)
        assert_eq!(cfg.model, "m1 注释");
        assert_eq!(cfg.timeout, 9);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempfile_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "lyyime-ai-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
