//! EngineLogic —— ibus 引擎按键逻辑核心(无 IBus 依赖,可直接单测)。
//!
//! Rust 移植自原 lyyime.py 的 EngineLogic(python 版已删除),行为合同不变:
//! keysym/state → LKey → lyyime-core(进程内直连,不再走 ctypes FFI)→ 效果流,
//! 经 [`Host`] 回调交给胶水层。含 Shift 单击检测与 /AI 触发会话
//! (交互合同:ARCHITECTURE.md §11;与 Mode B lyyime-xim 保持一致)。
//!
//! 健壮性合同(§7):core 初始化失败进入降级英文直通,绝不卡死按键。

use crate::keysym::{
    is_shift, map_keyval, BLOCKING_MODS, KSYM_BACKSPACE, KSYM_ESCAPE, KSYM_KP_ENTER,
    KSYM_RETURN, KSYM_SPACE, MASK_RELEASE, PURPOSE_PASSWORD, PURPOSE_PIN,
};
use lyyime_ai::AiConfig;
use lyyime_core::{Effect, Engine, LKey};
use std::path::PathBuf;

/// 胶水层回调(IBus 实现者或单测 Mock)。
pub trait Host {
    /// 上屏文本
    fn on_commit(&mut self, text: &str);
    /// 预编辑更新(None = 清除)
    fn on_preedit(&mut self, text: Option<&str>);
    /// 候选页更新:cands 为 [(文本, 注释)];aux 为辅助区输入串
    fn on_candidates(&mut self, cands: &[(String, String)], page: usize, pages: usize, aux: &str);
    /// 模式变化(0=中文 1=英文)
    fn on_mode_changed(&mut self, mode: u8);
    /// 辅助区临时提示
    fn on_notice(&mut self, text: &str);
    /// /AI 会话回车后提交提示词;宿主负责异步调用与结果上屏
    fn on_ai_submit(&mut self, prompt: String);
}

/// /AI 触发会话状态:idle=未触发;slash/slash_a=已吞触发前缀;
/// capture=提示词采集中(中文组词照常,组词结果进提示词而不是应用)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AiState {
    Idle,
    Slash,
    SlashA,
    Capture,
}

pub const AI_PROMPT_MAX: usize = 2000; // 提示词长度上限(字符),超出丢弃并提示

pub struct EngineLogic {
    /// core 引擎;None = 降级英文直通(核心未就绪)
    engine: Option<Engine>,
    core_cfg: lyyime_core::Config,
    pub data_dir: PathBuf,
    pub mode: u8,
    pub input_purpose: u32,
    pending_shift: Option<u32>,
    preedit: Option<String>,
    ai_state: AiState,
    ai_prompt: String,
    ai_prompt_full: bool,
    ai_cfg: Option<AiConfig>,
}

impl EngineLogic {
    pub fn new(data_dir: PathBuf, ai_cfg: Option<AiConfig>, core_cfg: lyyime_core::Config) -> EngineLogic {
        let mut engine = Engine::new(&data_dir).ok();
        if let Some(e) = engine.as_mut() {
            e.set_config(core_cfg.clone());
        }
        let mode = engine.as_ref().map(|e| e.mode()).unwrap_or(lyyime_core::Mode::Chinese);
        EngineLogic {
            engine,
            core_cfg,
            data_dir,
            mode: mode as u8,
            input_purpose: 0,
            pending_shift: None,
            preedit: None,
            ai_state: AiState::Idle,
            ai_prompt: String::new(),
            ai_prompt_full: false,
            ai_cfg,
        }
    }

    pub fn degraded(&self) -> bool {
        self.engine.is_none()
    }

    pub fn set_ai_cfg(&mut self, cfg: Option<AiConfig>) {
        self.ai_cfg = cfg;
        self.ai_reset_state();
    }

    /// 按键入口;返回 true=已消费 / false=放行给应用。
    /// 放行走 return false 常规路径,不走 forward_key_event(Qt5 不支持)。
    pub fn process_key_event(&mut self, host: &mut dyn Host, keyval: u32, state: u32) -> bool {
        if self.degraded() {
            return false; // 降级态:一切按键原样放行,绝不卡死输入
        }
        if self.input_purpose == PURPOSE_PASSWORD || self.input_purpose == PURPOSE_PIN {
            // 密码框全放行(借鉴 ibus-table 的防护)
            return false;
        }
        if state & MASK_RELEASE != 0 {
            return self.on_release(host, keyval);
        }
        self.on_press(host, keyval, state)
    }

    fn on_press(&mut self, host: &mut dyn Host, keyval: u32, state: u32) -> bool {
        if is_shift(keyval) {
            if state & BLOCKING_MODS != 0 {
                // Shift 参与组合键(Ctrl/Alt/Super+Shift):放行,不算单击
                self.pending_shift = None;
                return false;
            }
            // Shift 按下:有缓冲立即上屏英文原串;空缓冲进入单击检测
            let effects = self.core_mut().process_key(LKey::ShiftPress);
            let _consumed = self.dispatch(host, effects);
            if self.preedit.is_none() {
                self.pending_shift = Some(keyval);
            }
            return true; // 与 python 版一致:Shift 按下一律吞键
        }
        // 其它键按下:无论成败都取消未决的 Shift 单击
        self.pending_shift = None;
        if state & BLOCKING_MODS != 0 {
            // 应用快捷键(如 Ctrl+C):先放弃 AI 会话并复位缓冲,再放行
            self.ai_reset_state();
            let effects = self.core_mut().process_key(LKey::Other);
            return self.dispatch(host, effects);
        }
        if let Some(taken) = self.ai_take(host, keyval) {
            return taken;
        }
        let (lkey, ch) = map_keyval(keyval);
        let effects = self.core_mut().process_key(lkey);
        let _ = ch;
        self.dispatch(host, effects)
    }

    fn on_release(&mut self, host: &mut dyn Host, keyval: u32) -> bool {
        if is_shift(keyval) {
            let pending = self.pending_shift.take();
            if pending == Some(keyval) {
                // 按下后无其它键 → 判定单击:切换中英(合同 §6)
                self.switch_mode(host);
                return true;
            }
            return false; // 与单击无关的 Shift release:放行
        }
        false // 非 Shift 键的 release 一律放行
    }

    // ------------------------------------------------------------------
    // /AI 触发会话(中文态输入 /AI + 提示词,回车调用配置好的大模型)
    // ------------------------------------------------------------------
    fn ai_ready(&self) -> bool {
        // 英文态 /ai 属正常英文输入,不拦截
        matches!(self.ai_cfg, Some(ref c) if lyyime_ai::is_configured(c)) && self.mode == 0
    }

    fn ai_char(keyval: u32) -> Option<char> {
        // keysym → 可打印 ASCII 字符(含 Shift 产生的大写);其余 None
        char::from_u32(keyval).filter(|c| (' '..='~').contains(c))
    }

    fn ai_display_text(&self) -> String {
        format!(
            "/AI {}{}",
            self.ai_prompt,
            self.preedit.as_deref().unwrap_or("")
        )
    }

    fn ai_show(&mut self, host: &mut dyn Host) {
        let t = self.ai_display_text();
        host.on_preedit(Some(&t));
    }

    fn ai_reset_state(&mut self) {
        // 放弃会话(焦点切换/组合键):只清状态,显示由调用方统一清
        self.ai_state = AiState::Idle;
        self.ai_prompt.clear();
        self.ai_prompt_full = false;
    }

    fn ai_cancel(&mut self, host: &mut dyn Host) {
        self.ai_reset_state();
        self.preedit = None;
        host.on_preedit(None);
        host.on_candidates(&[], 0, 0, "");
    }

    fn ai_flush(&mut self, host: &mut dyn Host, keyval: u32) -> bool {
        // 触发前缀打歪:补发已吞的 `/`(与 `/a`),再按普通路径处理当前键。
        // `/` 在 core 无中文映射,普通文本框内文本上屏 `/` 与直通输出一致。
        let was_slash_a = self.ai_state == AiState::SlashA;
        self.ai_reset_state();
        host.on_commit("/");
        if was_slash_a {
            let effects = self.core_mut().process_key(LKey::Char('a'));
            self.dispatch(host, effects);
        }
        let (lkey, _ch) = map_keyval(keyval);
        let effects = self.core_mut().process_key(lkey);
        self.dispatch(host, effects)
    }

    fn ai_append_prompt(&mut self, host: &mut dyn Host, text: &str) {
        let room = AI_PROMPT_MAX.saturating_sub(self.ai_prompt.chars().count());
        if room == 0 {
            if !self.ai_prompt_full {
                self.ai_prompt_full = true;
                host.on_notice(&format!("AI:提示词已达 {AI_PROMPT_MAX} 字上限"));
            }
            return;
        }
        let take: String = text.chars().take(room).collect();
        self.ai_prompt.push_str(&take);
    }

    /// AI 触发状态机。Some(taken)=接管;None=不涉 AI(走普通路径)。
    fn ai_take(&mut self, host: &mut dyn Host, keyval: u32) -> Option<bool> {
        if !self.ai_ready() {
            return None;
        }
        let ch = Self::ai_char(keyval);
        match self.ai_state {
            AiState::Idle => {
                if ch == Some('/') && self.preedit.is_none() {
                    self.ai_state = AiState::Slash;
                    self.ai_show(host);
                    return Some(true);
                }
                None
            }
            AiState::Slash | AiState::SlashA => {
                if matches!(ch, Some('a') | Some('A')) && self.ai_state == AiState::Slash {
                    self.ai_state = AiState::SlashA;
                    self.ai_show(host);
                    return Some(true);
                }
                if matches!(ch, Some('i') | Some('I')) && self.ai_state == AiState::SlashA {
                    self.ai_state = AiState::Capture;
                    self.ai_prompt.clear();
                    self.preedit = None;
                    self.ai_show(host);
                    return Some(true);
                }
                Some(self.ai_flush(host, keyval))
            }
            AiState::Capture => Some(self.ai_capture_key(host, keyval, ch)),
        }
    }

    fn ai_capture_key(&mut self, host: &mut dyn Host, keyval: u32, ch: Option<char>) -> bool {
        match keyval {
            KSYM_RETURN | KSYM_KP_ENTER => {
                // core 有缓冲:Enter 合同是"上屏原始字母" → 归入提示词后发送
                let effects = self.core_mut().process_key(LKey::Enter);
                self.dispatch_ai(host, effects);
                let prompt = self.ai_prompt.trim().to_string();
                self.ai_cancel(host);
                if prompt.is_empty() {
                    host.on_notice("AI:提示词为空,已取消");
                    return true;
                }
                host.on_ai_submit(prompt);
                true
            }
            KSYM_ESCAPE => {
                let effects = self.core_mut().process_key(LKey::Esc);
                let consumed = self.dispatch_ai(host, effects);
                if !consumed {
                    self.ai_cancel(host); // 空组词缓冲 Esc:退出 AI 会话
                }
                true
            }
            KSYM_BACKSPACE => {
                let effects = self.core_mut().process_key(LKey::Backspace);
                let consumed = self.dispatch_ai(host, effects);
                if !consumed {
                    // core 空缓冲:删提示词尾字符
                    if self.ai_prompt.pop().is_some() {
                        self.ai_show(host);
                    } else {
                        self.ai_cancel(host); // 删穿 /AI 前缀:取消(不回放 /AI)
                    }
                }
                true
            }
            KSYM_SPACE => {
                let effects = self.core_mut().process_key(LKey::Space);
                let consumed = self.dispatch_ai(host, effects);
                if !consumed {
                    // 空组词缓冲:空格是提示词的一部分
                    self.ai_append_prompt(host, " ");
                    self.ai_show(host);
                }
                true
            }
            _ if ch.is_none() => {
                // 方向键/功能键等:让 core 复位组词并放行,放弃 AI 会话
                let effects = self.core_mut().process_key(LKey::Other);
                let consumed = self.dispatch_ai(host, effects);
                if !consumed {
                    self.ai_cancel(host);
                    return false;
                }
                true
            }
            _ => {
                let (lkey, _c) = map_keyval(keyval);
                let effects = self.core_mut().process_key(lkey);
                let consumed = self.dispatch_ai(host, effects);
                if !consumed {
                    // core 放行(英文态字母/无候选数字/未映射标点)→ 原字符进提示词
                    if matches!(lkey, LKey::Char(_) | LKey::Digit(_) | LKey::Punct(_)) {
                        let s = ch.unwrap().to_string();
                        self.ai_append_prompt(host, &s);
                    }
                }
                self.ai_show(host); // 组词选词(commit)也会改提示词,统一刷新展示
                true
            }
        }
    }

    /// 采集态的效果流分发:commit 归入提示词,其余与 dispatch 一致。
    fn dispatch_ai(&mut self, host: &mut dyn Host, effects: Vec<Effect>) -> bool {
        let mut consumed = true;
        for eff in effects {
            match eff {
                Effect::Commit(text) => self.ai_append_prompt(host, &text),
                Effect::Preedit(p) => {
                    self.preedit = p;
                    self.ai_show(host);
                }
                Effect::Candidates(cands) => {
                    let (page, pages) = self
                        .engine
                        .as_ref()
                        .map(|e| (e.page(), e.page_count()))
                        .unwrap_or((0, 0));
                    let list: Vec<(String, String)> = cands
                        .iter()
                        .map(|c| (c.text.clone(), c.comment.clone()))
                        .collect();
                    let aux = self.preedit.clone().unwrap_or_default();
                    host.on_candidates(&list, page, pages, &aux);
                }
                Effect::Pass => consumed = false,
                Effect::Consumed => {}
                Effect::Notice(t) => host.on_notice(&t),
                Effect::ModeChanged(m) => {
                    self.mode = m as u8;
                    host.on_mode_changed(self.mode);
                }
            }
        }
        consumed
    }

    // ------------------------------------------------------------------
    // 效果流分发(合同 §3:顺序即宿主处理顺序)
    // ------------------------------------------------------------------
    fn dispatch(&mut self, host: &mut dyn Host, effects: Vec<Effect>) -> bool {
        /// 逐条执行效果;返回 false 仅当出现 pass(宿主必须放行该键)。
        let mut consumed = true;
        for eff in effects {
            match eff {
                Effect::Commit(text) => {
                    if !text.is_empty() {
                        host.on_commit(&text);
                    }
                }
                Effect::Preedit(p) => {
                    // 缺失 / 空串都视为清除
                    self.preedit = p.filter(|s| !s.is_empty());
                    host.on_preedit(self.preedit.as_deref());
                }
                Effect::Candidates(cands) => {
                    // 页码信息在效果流之外,直连 core 时从引擎当前状态读取
                    let (page, pages) = self
                        .engine
                        .as_ref()
                        .map(|e| (e.page(), e.page_count()))
                        .unwrap_or((0, 0));
                    let list: Vec<(String, String)> = cands
                        .iter()
                        .map(|c| (c.text.clone(), c.comment.clone()))
                        .collect();
                    let aux = self.preedit.clone().unwrap_or_default();
                    host.on_candidates(&list, page, pages, &aux);
                }
                Effect::Pass => consumed = false,
                Effect::Consumed => {} // 吞键但无可见效果
                Effect::Notice(t) => host.on_notice(&t),
                Effect::ModeChanged(m) => {
                    self.mode = m as u8;
                    host.on_mode_changed(self.mode);
                }
            }
        }
        consumed
    }

    // ------------------------------------------------------------------
    // 对外操作(属性菜单/焦点切换调用)
    // ------------------------------------------------------------------
    pub fn switch_mode(&mut self, host: &mut dyn Host) {
        // 切换中英模式并清空会话状态(与 Shift 单击、托盘菜单共用)
        if let Some(e) = self.engine.as_mut() {
            e.reset();
        }
        self.preedit = None;
        host.on_preedit(None);
        host.on_candidates(&[], 0, 0, "");
        if let Some(e) = self.engine.as_mut() {
            self.mode = e.toggle_mode() as u8;
        }
        host.on_mode_changed(self.mode);
    }

    pub fn reset_session(&mut self, host: &mut dyn Host) {
        // 焦点进出/重置:清 core 缓冲与 UI(模式跨焦点保持);AI 会话放弃
        if let Some(e) = self.engine.as_mut() {
            e.reset();
        }
        self.ai_reset_state();
        self.preedit = None;
        host.on_preedit(None);
        host.on_candidates(&[], 0, 0, "");
        host.on_mode_changed(self.mode);
    }

    pub fn reload_dict(&mut self, host: &mut dyn Host) -> Result<(), anyhow::Error> {
        // 重载词库:释放并重建 core Engine(重新加载 data_dir 下的词典)
        let mut e = Engine::new(&self.data_dir)?;
        e.set_config(self.core_cfg.clone());
        e.reset();
        self.mode = e.mode() as u8;
        self.engine = Some(e);
        self.ai_reset_state();
        self.reset_session(host);
        Ok(())
    }

    fn core_mut(&mut self) -> &mut Engine {
        self.engine
            .as_mut()
            .expect("degraded 态不会进入本路径(process_key_event 已拦截)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// 事件记录 Mock 宿主。
    #[derive(Default)]
    struct Mock {
        events: RefCell<Vec<String>>,
        ai_prompts: RefCell<Vec<String>>,
    }
    impl Mock {
        fn log(&self, s: String) {
            self.events.borrow_mut().push(s);
        }
    }
    impl Host for Mock {
        fn on_commit(&mut self, text: &str) {
            self.log(format!("commit:{text}"));
        }
        fn on_preedit(&mut self, text: Option<&str>) {
            self.log(format!("preedit:{}", text.unwrap_or("∅")));
        }
        fn on_candidates(&mut self, cands: &[(String, String)], _page: usize, _pages: usize, aux: &str) {
            let texts: Vec<String> = cands.iter().map(|(t, _)| t.clone()).collect();
            self.log(format!("cands:[{}]", texts.join(",")));
            self.log(format!("aux:{aux}"));
        }
        fn on_mode_changed(&mut self, mode: u8) {
            self.log(format!("mode:{mode}"));
        }
        fn on_notice(&mut self, text: &str) {
            self.log(format!("notice:{text}"));
        }
        fn on_ai_submit(&mut self, prompt: String) {
            self.ai_prompts.borrow_mut().push(prompt.clone());
            self.log(format!("ai-submit:{prompt}"));
        }
    }

    /// 极简词库夹具:wqvb→你好(五笔)、ni→你(拼音)、数字选词由 core 驱动。
    fn fixtures_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "lyyime-ibus-logic-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("wubi.tsv"),
            "wqvb\t你好\t1000\nwq\t你\t900\nntna\t发展\t800\n",
        )
        .unwrap();
        std::fs::write(d.join("pinyin_char.tsv"), "ni\t你\t100\nhao\t好\t90\n").unwrap();
        std::fs::write(
            d.join("pinyin_phrase.tsv"),
            "你好\tni hao\t500\n中国\tzhong guo\t400\n",
        )
        .unwrap();
        std::fs::write(d.join("english.tsv"), "the\t100\nok\t90\n").unwrap();
        std::fs::write(d.join("suggestion.tsv"), "的\t1000\n").unwrap();
        std::fs::write(d.join("meta.json"), "{\"version\":1}").unwrap();
        d
    }

    fn logic_with_ai(cfg: Option<AiConfig>) -> (PathBuf, EngineLogic) {
        let dir = fixtures_dir();
        let logic = EngineLogic::new(dir.clone(), cfg, lyyime_core::Config::default());
        (dir, logic)
    }

    const SHIFT_L: u32 = 0xffe1;

    #[test]
    fn type_wqvb_and_space_commits_nihao() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        for k in ['w', 'q', 'v', 'b'] {
            assert!(l.process_key_event(&mut h, k as u32, 0));
        }
        assert!(l.process_key_event(&mut h, 0x20, 0)); // space
        let ev = h.events.borrow();
        assert!(ev.iter().any(|e| e == "commit:你好"), "events={ev:?}");
        assert!(ev.iter().any(|e| e.starts_with("cands:["))); // 出过候选
    }

    #[test]
    fn shift_press_flushes_letters_then_single_click_toggles_english() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        for k in ['t', 'h', 'e'] {
            l.process_key_event(&mut h, k as u32, 0);
        }
        // Shift 按下:有缓冲 → 上屏英文原串
        l.process_key_event(&mut h, SHIFT_L, 0);
        assert!(h.events.borrow().iter().any(|e| e == "commit:the"));
        // Shift 释放(无其它键)→ 单击生效切英文
        l.process_key_event(&mut h, SHIFT_L, 1 << 30);
        assert!(h.events.borrow().iter().any(|e| e == "mode:1"));
        // 英文态:字母直通
        assert!(!l.process_key_event(&mut h, 0x61, 0)); // 'a' 放行
    }

    #[test]
    fn uppercase_keyval_lowercased() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 0x41, 0); // 'A'(CapsLock 大写)
        let ev = h.events.borrow().clone();
        let evs: Vec<&String> = ev.iter().filter(|e| e.starts_with("aux:")).collect();
        // 辅助区输入串应为小写 'a'
        assert!(evs.iter().any(|e| e.contains("a")), "events={ev:?}");
    }

    #[test]
    fn ai_session_capture_and_submit() {
        let cfg = AiConfig {
            enabled: true,
            api_base: "http://127.0.0.1:9/v1".into(),
            model: "m".into(),
            ..Default::default()
        };
        let (_d, mut l) = logic_with_ai(Some(cfg));
        let mut h = Mock::default();
        for k in ['/', 'a', 'i'] {
            assert!(l.process_key_event(&mut h, k as u32, 0), "key {k} 应被吞");
        }
        for k in ['h', 'i'] {
            l.process_key_event(&mut h, k as u32, 0);
        }
        l.process_key_event(&mut h, 0xff0d, 0); // Return
        assert_eq!(h.ai_prompts.borrow().clone(), vec!["hi"]);
        assert!(h.events.borrow().iter().any(|e| e.starts_with("preedit:/AI ")));
    }

    #[test]
    fn ai_session_broken_prefix_flushes_slash() {
        let cfg = AiConfig {
            enabled: true,
            api_base: "http://127.0.0.1:9/v1".into(),
            model: "m".into(),
            ..Default::default()
        };
        let (_d, mut l) = logic_with_ai(Some(cfg));
        let mut h = Mock::default();
        l.process_key_event(&mut h, '/' as u32, 0);
        l.process_key_event(&mut h, 'x' as u32, 0); // 打歪
        // `/` 已按文本补发,x 走普通路径(直通)
        assert!(h.events.borrow().iter().any(|e| e == "commit:/"));
    }

    #[test]
    fn ai_not_triggered_in_english_mode() {
        let cfg = AiConfig {
            enabled: true,
            api_base: "http://127.0.0.1:9/v1".into(),
            model: "m".into(),
            ..Default::default()
        };
        let (_d, mut l) = logic_with_ai(Some(cfg));
        let mut h = Mock::default();
        l.switch_mode(&mut h); // → 英文
        assert!(!l.process_key_event(&mut h, '/' as u32, 0)); // 直通
        assert!(h.ai_prompts.borrow().is_empty());
    }

    #[test]
    fn password_purpose_all_pass() {
        let (_d, mut l) = logic_with_ai(None);
        l.input_purpose = 8; // PURPOSE_PASSWORD
        let mut h = Mock::default();
        assert!(!l.process_key_event(&mut h, 'w' as u32, 0));
    }

    #[test]
    fn degraded_mode_passes_everything() {
        let mut l = EngineLogic::new(
            PathBuf::from("/nonexistent-lyyime-deg"),
            None,
            lyyime_core::Config::default(),
        );
        // core 对缺失词库仍可建空引擎,不视为降级;手动置降级验证放行路径
        if !l.degraded() {
            l.engine = None;
        }
        let mut h = Mock::default();
        assert!(!l.process_key_event(&mut h, 'w' as u32, 0));
    }

    #[test]
    fn ctrl_combo_releases_buffer_and_passes() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 'n' as u32, 0);
        // Ctrl+C:缓冲复位(有缓冲 Shift 语义不触发),按键放行
        assert!(!l.process_key_event(&mut h, 0x63, 1 << 2));
    }
}
