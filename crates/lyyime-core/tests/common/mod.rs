//! 集成测试公共辅助:fixtures 路径、临时目录、引擎构造、效果流断言工具。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use lyyime_core::{Config, Effect, Engine, LKey};

/// 手写小 fixtures 目录(wubi.tsv / pinyin_char.tsv / ...)。
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// 唯一自增,避免并发测试目录冲突。
static SEQ: AtomicU32 = AtomicU32::new(0);

/// 测试专用临时目录(严格避免写真实 HOME;Drop 时自动清理)。
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("lyyime-core-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

/// 每次调用给一个独立的临时目录(泄漏保活到进程结束,避免写真实 HOME、
/// 也避免各测试间通过同一用户词典相互污染)。
fn leaked_user_dict() -> PathBuf {
    let td: &'static TempDir = Box::leak(Box::new(TempDir::new()));
    td.join("user.tsv")
}

/// 真实尺度 fixtures 目录(单字 ~1e9 / 词组 ~1e3 跨源混排)。
pub fn fixtures_real() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures-real-scale")
}

/// 从 fixtures 构造引擎(用户词典指向独立临时目录,测试绝不写真实 HOME)。
pub fn engine() -> Engine {
    engine_with_fixtures(&fixtures(), Config::default())
}

/// 从真实尺度 fixtures 构造引擎(同样封闭用户词典)。
pub fn engine_real() -> Engine {
    engine_with_fixtures(&fixtures_real(), Config::default())
}

/// 从 fixtures 构造引擎,用户词典指向给定 tempdir(学习类测试专用)。
pub fn engine_with_user_dict(td: &TempDir) -> Engine {
    engine_with_fixtures(
        &fixtures(),
        Config {
            user_dict: Some(td.join("user.tsv")),
            ..Config::default()
        },
    )
}

/// 从 fixtures 构造引擎并套用自定义配置;未指定 user_dict 时同样落到临时目录。
pub fn engine_with(cfg: Config) -> Engine {
    engine_with_fixtures(&fixtures(), cfg)
}

/// 通用:指定词库目录 + 配置构造引擎;用户词典缺省落到独立临时目录。
pub fn engine_with_fixtures(dir: &Path, cfg: Config) -> Engine {
    let mut eng = Engine::new(dir).unwrap();
    let cfg = Config {
        user_dict: cfg.user_dict.or_else(|| Some(leaked_user_dict())),
        ..cfg
    };
    eng.set_config(cfg);
    eng
}

/// 逐键喂入一串小写字母,返回全部效果。
pub fn type_str(eng: &mut Engine, s: &str) -> Vec<Effect> {
    let mut all = Vec::new();
    for c in s.chars() {
        assert!(c.is_ascii_lowercase(), "type_str 仅接受小写字母,收到 {c}");
        all.extend(eng.process_key(LKey::Char(c)));
    }
    all
}

/// 取效果流里的全部 Commit 文本。
pub fn commits(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Commit(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// 是否包含某个 Commit 文本。
pub fn has_commit(effects: &[Effect], text: &str) -> bool {
    commits(effects).iter().any(|c| c == text)
}

/// 是否为单一 `[Effect::Pass]`。
pub fn is_pass(effects: &[Effect]) -> bool {
    effects.len() == 1 && matches!(effects[0], Effect::Pass)
}

/// 是否为单一 `[Effect::Consumed]`。
pub fn is_consumed(effects: &[Effect]) -> bool {
    effects.len() == 1 && matches!(effects[0], Effect::Consumed)
}

/// 当前页候选文本。
pub fn page_texts(eng: &Engine) -> Vec<String> {
    eng.flush_page().iter().map(|c| c.text.clone()).collect()
}

/// 最后一次 Preedit 的内容。
pub fn last_preedit(effects: &[Effect]) -> Option<String> {
    effects.iter().rev().find_map(|e| match e {
        Effect::Preedit(p) => p.clone(),
        _ => None,
    })
}

/// 取效果流里的全部 Notice 文本。
pub fn notices(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Notice(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// 造词模式下当前展示的单条候选(文本, 注释)。
pub fn coin_candidate(eng: &Engine) -> Option<(String, String)> {
    let page = eng.flush_page();
    page.first()
        .filter(|_| eng.page() == 0)
        .map(|c| (c.text.clone(), c.comment.clone()))
}

/// 取效果流里的全部 Hint 文本(词组效率提示)。
pub fn hints(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Hint(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}
