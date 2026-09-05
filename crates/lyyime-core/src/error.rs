//! 统一错误类型:所有对外的 `Result` 都使用本类型。
//!
//! 按项目质量红线,错误消息必须是面向用户的"人话"(中文),
//! 宿主(GUI / ibus 引擎 / CLI)可直接把 `Error::message()` 展示给用户或写入日志。

use std::fmt;
use std::io;

/// lyyime-core 统一错误。
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
}

impl Error {
    /// 由一段中文消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }

    /// 取人话错误消息(可直接展示)。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::new(format!("文件读写失败:{e}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new(format!("JSON 解析失败:{e}"))
    }
}

impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Self::new(format!("TOML 解析失败:{e}"))
    }
}
