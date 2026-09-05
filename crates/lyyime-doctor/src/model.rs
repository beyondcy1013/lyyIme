//! 公共数据模型(serde 可序列化,与 CLI --json 输出一一对应)。

use serde::Serialize;

/// 检查结果状态。`fixed` 保留给 GUI 修复后复检场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Fixed,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Fixed => "fixed",
        }
    }
    pub fn is_problem(&self) -> bool {
        matches!(self, Status::Warn | Status::Fail)
    }
}

/// 单项检查结果
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub detail: String,
    pub fix_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub ok: u32,
    pub warn: u32,
    pub fail: u32,
}

/// `check --json` 顶层结构
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub checks: Vec<CheckResult>,
    pub summary: Summary,
}

impl CheckReport {
    pub fn new(checks: Vec<CheckResult>) -> Self {
        let (mut ok, mut warn, mut fail) = (0, 0, 0);
        for c in &checks {
            match c.status {
                Status::Ok | Status::Fixed => ok += 1,
                Status::Warn => warn += 1,
                Status::Fail => fail += 1,
            }
        }
        CheckReport { checks, summary: Summary { ok, warn, fail } }
    }
}

/// 一个修复动作(id 为 fix id,issue 为来源检查 id)
#[derive(Debug, Clone, Serialize)]
pub struct FixAction {
    pub id: String,
    pub issue: String,
    pub description: String,
}

/// apply 的结果
#[derive(Debug, Clone)]
pub enum ApplyOutcome {
    Applied(String),
    DryRun(String),
    Skipped(String),
    Failed(String),
}

impl ApplyOutcome {
    pub fn is_failure(&self) -> bool {
        matches!(self, ApplyOutcome::Failed(_))
    }
    pub fn tag(&self) -> &'static str {
        match self {
            ApplyOutcome::Applied(_) => "APPLIED",
            ApplyOutcome::DryRun(_) => "DRY-RUN",
            ApplyOutcome::Skipped(_) => "SKIPPED",
            ApplyOutcome::Failed(_) => "FAILED",
        }
    }
    pub fn message(&self) -> &str {
        match self {
            ApplyOutcome::Applied(s)
            | ApplyOutcome::DryRun(s)
            | ApplyOutcome::Skipped(s)
            | ApplyOutcome::Failed(s) => s,
        }
    }
}

/// 输入法类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ImeKind {
    #[serde(rename = "ibus-engine")]
    IbusEngine,
    #[serde(rename = "fcitx5")]
    Fcitx5,
    #[serde(rename = "lyyime")]
    Lyyime,
    #[serde(rename = "other")]
    Other,
}

impl ImeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ImeKind::IbusEngine => "ibus-engine",
            ImeKind::Fcitx5 => "fcitx5",
            ImeKind::Lyyime => "lyyime",
            ImeKind::Other => "other",
        }
    }
    /// 排序权重:lyyime 在前,其后 ibus 引擎、fcitx5、其它
    pub fn rank(&self) -> u8 {
        match self {
            ImeKind::Lyyime => 0,
            ImeKind::IbusEngine => 1,
            ImeKind::Fcitx5 => 2,
            ImeKind::Other => 3,
        }
    }
}

/// ime-list 条目(ARCHITECTURE.md §9.1 合同)
#[derive(Debug, Clone, Serialize)]
pub struct ImeEntry {
    pub id: String,
    pub name: String,
    pub kind: ImeKind,
    pub installed: bool,
    pub active: bool,
    pub is_default: bool,
    pub package: String,
    pub language: String,
}

/// ime-add / ime-remove / ime-default 的操作结果
#[derive(Debug, Clone, Serialize)]
pub struct ImeOpResult {
    pub action: String,
    pub target: String,
    /// false = dry-run 预览(未实际执行)
    pub executed: bool,
    /// 将执行/已执行的命令(人读形式)
    pub commands: Vec<String>,
    /// 操作前的输入法 id 列表(差量证据)
    pub before: Vec<String>,
    /// 操作后的 id 列表(仅实际执行时)
    pub after: Option<Vec<String>>,
    pub warnings: Vec<String>,
    /// gsettings 等不可用时的手动指引
    pub manual_hint: Option<String>,
    pub log_path: Option<String>,
}
