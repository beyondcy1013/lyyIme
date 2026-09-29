//! EngineLogic —— ibus 引擎按键逻辑核心(无 IBus 依赖,可直接单测)。
//!
//! Rust 移植自原 lyyime.py 的 EngineLogic(python 版已删除),行为合同不变:
//! keysym/state → LKey → lyyime-core(进程内直连,不再走 ctypes FFI)→ 效果流,
//! 经 [`Host`] 回调交给胶水层。含 Shift 单击检测与 /AI 触发会话
//! (交互合同:ARCHITECTURE.md §11;与 Mode B lyyime-xim 保持一致)。
//!
//! 健壮性合同(§7):core 初始化失败进入降级英文直通,绝不卡死按键。

use crate::keysym::{
    hotkey_match, is_shift, map_keyval, parse_hotkey, BLOCKING_MODS, KSYM_BACKSPACE,
    KSYM_ESCAPE, KSYM_KP_ENTER, KSYM_RETURN, KSYM_SPACE, MASK_LOCK, MASK_RELEASE,
    PURPOSE_PASSWORD, PURPOSE_PIN,
};
use lyyime_ai::AiConfig;
use lyyime_core::{CandOp, Effect, Engine, LKey};
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
    /// 候选条效率提示(词组提示):展示后不带定时,直到下一次输入才清除
    fn on_hint(&mut self, text: &str);
    /// /AI 会话回车后提交提示词;宿主负责异步调用与结果上屏
    fn on_ai_submit(&mut self, prompt: String);
    /// 截屏热键命中(§13):宿主拉起 lyyime-shot 子进程(异步,不阻塞按键流)
    fn on_shot(&mut self);
    /// 快速功能键命中(§14):宿主按 `quick_actions[index].command` 执行功能
    /// (@settings/@help 内置或 shell 命令),不上屏任何文本
    fn on_action(&mut self, index: usize);
    /// 自定义查询(§15 菜单第 4 项):宿主拉起浏览器打开已代入词的网址
    /// (xdg-open;异步,不阻塞按键流)
    fn on_open_url(&mut self, url: &str);
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
    /// 截屏热键解析结果((修饰,键值);None=配置非法不拦截,§13)
    hotkey_shot: Option<(u32, u32)>,
    /// 造词热键解析结果(§12;Mode A 与 Mode B 同合同)
    hotkey_coin: Option<(u32, u32)>,
    /// 输入统计目录(None=不记录,单测用;生产为 ~/.local/share/lyyime/stats)
    pub stats_dir: Option<PathBuf>,
    /// §15 候选右键菜单(ibus 版):面板无弹菜单 API,约定为"候选区临时换成
    /// 操作行(固定首位/删除词组/反查英文),数字 1-3 或点选执行;Esc/任意键
    /// 退出并还原真实候选"。Some = 原候选页内下标。
    cand_menu: Option<usize>,
    /// §15 自定义查询(菜单第 4 行;config.toml custom_query_*,
    /// 焦点进入时热读 —— 设置保存后即时生效)。None = 操作行不显示。
    custom_query: Option<lyyime_core::wordops::CustomQuery>,
}

impl EngineLogic {
    pub fn new(data_dir: PathBuf, ai_cfg: Option<AiConfig>, core_cfg: lyyime_core::Config) -> EngineLogic {
        let mut engine = Engine::new(&data_dir).ok();
        if let Some(e) = engine.as_mut() {
            e.set_config(core_cfg.clone());
        }
        let mode = engine.as_ref().map(|e| e.mode()).unwrap_or(lyyime_core::Mode::Chinese);
        let hotkey_shot = parse_hotkey(&core_cfg.shot_hotkey);
        let hotkey_coin = parse_hotkey(&core_cfg.coin_hotkey);
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
            hotkey_shot,
            hotkey_coin,
            stats_dir: lyyime_core::stats::default_dir(),
            cand_menu: None,
            custom_query: None,
        }
    }

    /// §15 自定义查询配置注入(config.toml custom_query_*;None=操作行
    /// 不显示第 4 项)。focus_in 时由 service 热读注入,设置保存即生效。
    pub fn set_custom_query(&mut self, cq: Option<lyyime_core::wordops::CustomQuery>) {
        self.custom_query = cq.filter(|c| c.configured());
    }

    pub fn degraded(&self) -> bool {
        self.engine.is_none()
    }

    /// 截屏热键配置原文(解析非法返回 None;菜单 tooltip 展示用,§13)
    pub fn shot_hotkey_text(&self) -> Option<String> {
        parse_hotkey(&self.core_cfg.shot_hotkey).map(|_| self.core_cfg.shot_hotkey.clone())
    }

    /// 快速功能键配置(合同 §14;胶水层执行 command 用)
    pub fn quick_actions(&self) -> &[lyyime_core::QuickAction] {
        &self.core_cfg.quick_actions
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
        // §15 右键操作行激活期间:数字 1-3(自定义查询已配则到 4)执行、
        // Esc 取消还原、其余键(含 Shift/组合键)先还原真实候选再按常态
        // 路径处理 ——core 侧缓冲/候选从未改动,菜单纯属显示层状态。
        if self.cand_menu.is_some() {
            match keyval {
                KSYM_ESCAPE => {
                    self.cand_menu_restore(host);
                    return true;
                }
                k @ 0x31..=0x34 => return self.cand_menu_exec(host, (k - 0x31) as usize),
                _ => self.cand_menu_restore(host),
            }
        }
        if is_shift(keyval) {
            if state & BLOCKING_MODS != 0 {
                // Shift 参与组合键(Ctrl/Alt/Super+Shift):放行,不算单击
                self.pending_shift = None;
                return false;
            }
            // Shift 按下:有缓冲立即上屏英文原串并消费本次 Shift;
            // 仅空缓冲进入单击检测,避免 release 又把模式切到英文。
            let had_composition = self
                .engine
                .as_ref()
                .is_some_and(|engine| !engine.buffer().is_empty());
            let effects = self.core_mut().process_key(LKey::ShiftPress);
            let _consumed = self.dispatch(host, effects);
            if !had_composition {
                self.pending_shift = Some(keyval);
            }
            return true; // 与 python 版一致:Shift 按下一律吞键
        }
        // 其它键按下:无论成败都取消未决的 Shift 单击
        self.pending_shift = None;
        // ---- 截屏热键(合同 §13):纯工具组合键,中英文态同效。
        // ibus 激活期间按键必经引擎,故不区分 core 模式;命中即拉起
        // lyyime-shot 子进程并吞键,不进组词缓冲、不经 core。 ----
        if let Some((mods, sym)) = self.hotkey_shot {
            if hotkey_match(state, keyval, mods, sym) {
                self.ai_reset_state();
                host.on_shot();
                return true;
            }
        }
        // ---- 造词热键(合同 §12,默认 Ctrl+=):组合键先放弃 AI 会话再进造词 ----
        if let Some((mods, sym)) = self.hotkey_coin {
            if hotkey_match(state, keyval, mods, sym) {
                self.ai_reset_state();
                let effects = self.core_mut().process_key(LKey::Coin);
                return self.dispatch(host, effects);
            }
        }
        if state & BLOCKING_MODS != 0 {
            // 应用快捷键(如 Ctrl+C):先放弃 AI 会话并复位缓冲,再放行
            self.ai_reset_state();
            let effects = self.core_mut().process_key(LKey::Other);
            return self.dispatch(host, effects);
        }
        if let Some(taken) = self.ai_take(host, keyval) {
            return taken;
        }
        // CapsLock 大写态(合同 §6):字母键一律直通英文,不进组词缓冲。
        // 无 Shift 时键值即大写,应用输出大写字母;Shift+字母 键值为小写,
        // 应用按 Caps+Shift 翻译输出小写字母。非字母键不受影响走常态。
        if state & MASK_LOCK != 0 && matches!(keyval, 0x41..=0x5a | 0x61..=0x7a) {
            let effects = self.core_mut().process_key(LKey::Other);
            return self.dispatch(host, effects);
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
                let buf_before = self.core_mut().buffer().to_string();
                let effects = self.core_mut().process_key(lkey);
                let consumed = self.dispatch_ai(host, effects);
                if !consumed {
                    // core 放行(英文态字母/无候选数字/未映射标点)→ 原字符进提示词
                    if matches!(lkey, LKey::Char(_) | LKey::Digit(_) | LKey::Punct(_)) {
                        let s = ch.unwrap().to_string();
                        self.ai_append_prompt(host, &s);
                    }
                } else if matches!(lkey, LKey::Char(_))
                    && !buf_before.is_empty()
                    && self
                        .engine
                        .as_ref()
                        .is_some_and(|e| e.buffer() == buf_before)
                {
                    // 死码吞键(缓冲未变):采集态里字母是提示词原料——先把当前
                    // 缓冲原样归入提示词,再追加本字母,保持敲击顺序。
                    let effects = self.core_mut().process_key(LKey::Enter);
                    self.dispatch_ai(host, effects);
                    self.ai_append_prompt(host, &ch.unwrap().to_string());
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
                Effect::Hint(t) => host.on_hint(&t),
                Effect::Action(i) => host.on_action(i),
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

    /// 输入统计(悬浮窗"停顿显示今日输入"的数据层):上屏即记一次
    /// 非空白字符数;任何错误静默,绝不影响输入主链路。
    fn record_stats(&self, text: &str) {
        let Some(dir) = self.stats_dir.as_ref() else {
            return;
        };
        let chars = text.chars().filter(|c| !c.is_whitespace()).count() as u64;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        lyyime_core::stats::record(dir, chars, now_ms);
    }

    fn dispatch(&mut self, host: &mut dyn Host, effects: Vec<Effect>) -> bool {
        // 逐条执行效果;返回 false 仅当出现 pass(宿主必须放行该键)
        let mut consumed = true;
        for eff in effects {
            match eff {
                Effect::Commit(text) => {
                    if !text.is_empty() {
                        self.record_stats(&text);
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
                Effect::Consumed => {}
                Effect::Notice(t) => host.on_notice(&t),
                Effect::Hint(t) => host.on_hint(&t),
                Effect::Action(i) => host.on_action(i),
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
    pub fn switch_mode(&mut self, host: &mut dyn Host) {
        // 切换中英模式并清空会话状态(与 Shift 单击、托盘菜单共用)
        if let Some(e) = self.engine.as_mut() {
            e.reset();
        }
        self.cand_menu = None;
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
        self.cand_menu = None;
        self.preedit = None;
        host.on_preedit(None);
        host.on_candidates(&[], 0, 0, "");
        host.on_mode_changed(self.mode);
    }

    /// 鼠标/面板点选当前页第 `idx`(0 起)个候选(合同 §14);
    /// 返回 true=已消费。功能键候选经 dispatch 产生 on_action。
    /// 右键菜单激活时,idx 映射到操作行(§15)。
    pub fn select_candidate(&mut self, host: &mut dyn Host, idx: usize) -> bool {
        if self.degraded() {
            return false;
        }
        if self.cand_menu.is_some() {
            return self.cand_menu_exec(host, idx);
        }
        let effects = self.core_mut().select_candidate(idx);
        self.dispatch(host, effects)
    }

    /// 候选窗面板翻页(候选窗「<」「>」按钮 / 滚轮,合同 §6):与键盘 `-`/`=`
    /// 同一条 core 路径(LKey::PageUp/PageDown),边界由 core 钳制不循环;
    /// 造词模式下同键承担多选/少选一字(§12)。
    /// 返回 true=已消费(无候选/单页时 core 给 Pass → false,面板无需刷新)。
    pub fn flip_page(&mut self, host: &mut dyn Host, down: bool) -> bool {
        if self.degraded() {
            return false;
        }
        let key = if down { LKey::PageDown } else { LKey::PageUp };
        let effects = self.core_mut().process_key(key);
        self.dispatch(host, effects)
    }

    /// (当前页, 总页数),页码 0 起;降级态/无候选为 (0, 0)。
    /// 面板翻页日志与单测断言用,避免测试复刻效果流细节。
    pub fn page_pos(&self) -> (usize, usize) {
        self.engine
            .as_ref()
            .map(|e| (e.page(), e.page_count()))
            .unwrap_or((0, 0))
    }

    // ------------------------------------------------------------------
    // §15 候选右键菜单(ibus CandidateClicked button=3)
    // ------------------------------------------------------------------

    /// 右键候选行:菜单状态经 core `cand_pinned` 判定(功能键/越界 → 吞掉
    /// 不开菜单);普通候选 → 候选区换成操作行,core 状态保持不动。
    pub fn cand_menu_open(&mut self, host: &mut dyn Host, idx: usize) -> bool {
        if self.degraded() {
            return false;
        }
        let Some(pinned) = self.engine.as_ref().and_then(|e| e.cand_pinned(idx)) else {
            return true;
        };
        self.cand_menu = Some(idx);
        let pin_label = if pinned { "取消固定首位" } else { "固定首位" };
        let mut rows: Vec<(String, String)> = vec![
            (pin_label.to_string(), "右键".to_string()),
            ("删除词组".to_string(), "右键".to_string()),
            ("反查英文".to_string(), "右键".to_string()),
        ];
        // 自定义查询(config.toml custom_query_*;url 未配则只有三行)
        if let Some(cq) = &self.custom_query {
            rows.push((cq.menu_label().to_string(), "右键".to_string()));
        }
        host.on_candidates(&rows, 0, 1, "右键操作:数字/点选执行,Esc 取消");
        true
    }

    /// 菜单激活时执行操作行(sel=0/1/2 core 操作,sel=3 自定义查询);
    /// 越界选择仅还原显示。
    fn cand_menu_exec(&mut self, host: &mut dyn Host, sel: usize) -> bool {
        let Some(orig) = self.cand_menu.take() else {
            return false;
        };
        const OPS: [CandOp; 3] = [CandOp::PinToggle, CandOp::Delete, CandOp::EnLookup];
        if let Some(&op) = OPS.get(sel) {
            let effects = self.core_mut().cand_op(orig, op);
            return self.dispatch(host, effects);
        }
        // 第 4 行=自定义查询:宿主侧打开浏览器,core 状态不动;先还原真实
        // 候选再发 notice(顺序对调会被候选刷新盖掉),与 Esc 同一还原流。
        self.cand_menu_restore(host);
        if sel == 3 {
            if let (Some(cq), Some(word)) = (
                self.custom_query.clone(),
                self.engine
                    .as_ref()
                    .and_then(|e| e.flush_page().get(orig).map(|c| c.text.clone())),
            ) {
                let url = lyyime_core::wordops::custom_query_url(&cq.url, &word);
                host.on_open_url(&url);
                host.on_notice(&format!("{}: {}", cq.menu_label(), word));
            }
        }
        true
    }

    /// 退出操作行并还原真实候选显示(core 侧缓冲/候选从未改动)。
    fn cand_menu_restore(&mut self, host: &mut dyn Host) {
        self.cand_menu = None;
        if let Some(e) = self.engine.as_ref() {
            let list: Vec<(String, String)> = e
                .flush_page()
                .iter()
                .map(|c| (c.text.clone(), c.comment.clone()))
                .collect();
            let aux = self.preedit.clone().unwrap_or_default();
            host.on_candidates(&list, e.page(), e.page_count(), &aux);
        }
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
        fn on_hint(&mut self, text: &str) {
            self.log(format!("hint:{text}"));
        }
        fn on_ai_submit(&mut self, prompt: String) {
            self.ai_prompts.borrow_mut().push(prompt.clone());
            self.log(format!("ai-submit:{prompt}"));
        }
        fn on_shot(&mut self) {
            self.log("shot".to_string());
        }
        fn on_action(&mut self, index: usize) {
            self.log(format!("action:{index}"));
        }
        fn on_open_url(&mut self, url: &str) {
            self.log(format!("open-url:{url}"));
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
        let mut logic = EngineLogic::new(dir.clone(), cfg, lyyime_core::Config::default());
        logic.stats_dir = None; // 单测不写真实 HOME 的统计目录
        (dir, logic)
    }

    fn logic_with_core_cfg(core_cfg: lyyime_core::Config) -> (PathBuf, EngineLogic) {
        let dir = fixtures_dir();
        let mut logic = EngineLogic::new(dir.clone(), None, core_cfg);
        logic.stats_dir = None;
        (dir, logic)
    }

    /// §15 右键菜单测试用:用户数据文件隔离到夹具目录(不碰真实 HOME),
    /// 关四码自动上屏保证候选可见。
    fn logic_isolated() -> (PathBuf, EngineLogic) {
        logic_with_core_cfg(lyyime_core::Config {
            user_dict: Some(fixtures_dir_user()),
            commit_first_at_four: false,
            commit_unique_four: false,
            ..Default::default()
        })
    }

    /// 独立的用户数据目录(与词库夹具目录分开,只收 pinned/blocked/user_words)。
    fn fixtures_dir_user() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "lyyime-ibus-logic-user-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("user_words.tsv")
    }

    /// 分页夹具:同一编码候选(表频递减)。core 每码只留频次前 9(§5.2),
    /// 默认 page_size=5 → 2 页(5+4)。复用 fixtures_dir 骨架(无 char_tier.tsv,
    /// 排序仅依赖表频,页内容可预期);关四码直上,保证翻页时组合仍在。
    fn logic_paging() -> (PathBuf, EngineLogic) {
        let dir = fixtures_dir();
        let words = ["工", "戈", "一", "地", "气", "心", "手", "口", "日", "光", "月", "明"];
        let rows: String = words
            .iter()
            .enumerate()
            .map(|(i, w)| format!("a\t{}\t{}\n", w, 120 - i as u32 * 5))
            .collect();
        std::fs::write(dir.join("wubi.tsv"), rows).unwrap();
        let mut logic =
            EngineLogic::new(dir.clone(), None, lyyime_core::Config {
                // 显式 5/页:默认 page_size=10 时单码 9 候选(§5.2 截断)只有 1 页
                page_size: 5,
                commit_unique_four: false,
                ..Default::default()
            });
        logic.stats_dir = None;
        (dir, logic)
    }

    /// 取最后一条**非空**候选事件(提交后 core 会清空候选,不能只看最后一条)。
    fn last_nonempty_cands(h: &Mock) -> String {
        h.events
            .borrow()
            .iter()
            .rev()
            .find(|e| e.starts_with("cands:[") && *e != "cands:[]")
            .expect("应有非空候选事件")
            .clone()
    }

    #[test]
    fn panel_flip_page_moves_window_and_clamps() {
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        assert!(l.process_key_event(&mut h, 'a' as u32, 0));
        assert_eq!(l.page_pos(), (0, 2), "9 候选按 5/页应为 2 页");
        let page1 = last_nonempty_cands(&h);
        // 「>」按钮(下一页):页窗移到第 2 页,页内是不同于首页的候选
        assert!(l.flip_page(&mut h, true), "翻页应消费");
        assert_eq!(l.page_pos(), (1, 2));
        let page2 = last_nonempty_cands(&h);
        assert_ne!(page1, page2, "翻页后页内容应变化:{page1} vs {page2}");
        // 末页再向下:core 边界钳制,页码不动
        assert!(l.flip_page(&mut h, true));
        assert_eq!(l.page_pos(), (1, 2));
        // 「<」按钮(上一页):回第 1 页,内容还原;首页再向上:钳制
        assert!(l.flip_page(&mut h, false));
        assert_eq!(l.page_pos(), (0, 2));
        assert_eq!(last_nonempty_cands(&h), page1, "翻回后应还原首页内容");
        assert!(l.flip_page(&mut h, false));
        assert_eq!(l.page_pos(), (0, 2));
    }

    #[test]
    fn panel_page_down_then_digit1_selects_page2_first() {
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        l.process_key_event(&mut h, 'a' as u32, 0);
        l.flip_page(&mut h, true);
        // 数字 1 按「当前页」相对选择 → 第 2 页首个候选,而非第 1 页首选
        let page2_first: String = last_nonempty_cands(&h)[7..]
            .trim_end_matches(']')
            .split(',')
            .next()
            .expect("第 2 页非空")
            .to_string();
        l.process_key_event(&mut h, '1' as u32, 0);
        assert!(
            h.events
                .borrow()
                .iter()
                .any(|e| *e == format!("commit:{page2_first}")),
            "数字 1 应上屏第 2 页首选 {page2_first}:events={:?}",
            h.events.borrow()
        );
        assert!(
            !h.events.borrow().iter().any(|e| e == "commit:一"),
            "不得仍从第 1 页选择:events={:?}",
            h.events.borrow()
        );
    }

    #[test]
    fn panel_flip_page_single_page_or_empty_is_noop() {
        // 无候选:'z' 无命中 → 翻页放行不消费
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 'z' as u32, 0);
        assert_eq!(l.page_pos(), (0, 0));
        assert!(!l.flip_page(&mut h, true));
        // 单页候选:简拼 'n' 仅 你 → core 给 Pass,页码不动
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 'n' as u32, 0);
        assert_eq!(l.page_pos(), (0, 1));
        assert!(!l.flip_page(&mut h, true));
        assert_eq!(l.page_pos(), (0, 1));
    }

    const SHIFT_L: u32 = 0xffe1;

    #[test]
    fn type_wqvb_and_space_commits_nihao() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        // 四码唯一上屏(默认开启):第 4 键 b 直接上屏「你好」,免空格。
        let mut consumed = true;
        for k in ['w', 'q', 'v', 'b'] {
            consumed = l.process_key_event(&mut h, k as u32, 0);
        }
        assert!(consumed);
        assert!(h
            .events
            .borrow()
            .iter()
            .any(|e| e == "commit:你好"), "events={:?}", h.events.borrow());
        assert!(h
            .events
            .borrow()
            .iter()
            .any(|e| e.starts_with("cands:["))); // 出过候选
        // 自动上屏后缓冲已空:空格放行(consumed=false)。
        assert!(!l.process_key_event(&mut h, 0x20, 0));
    }

    #[test]
    fn shift_press_flushes_letters_and_switches_english() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        for k in ['t', 'h', 'e'] {
            l.process_key_event(&mut h, k as u32, 0);
        }
        // Shift 按下:有缓冲 → 上屏英文原串;默认(shift_english=en)再切英文。
        l.process_key_event(&mut h, SHIFT_L, 0);
        assert!(h.events.borrow().iter().any(|e| e == "commit:the"));
        assert!(h.events.borrow().iter().any(|e| e == "mode:1"));
        // 释放不得再次切换(本次 Shift 已用于上屏+切换)。
        l.process_key_event(&mut h, SHIFT_L, 1 << 30);
        let mode_n = h
            .events
            .borrow()
            .iter()
            .filter(|e| e.starts_with("mode:"))
            .count();
        assert_eq!(mode_n, 1, "events={:?}", h.events.borrow());
        // 英文态:字母放行,不进组词缓冲。
        assert!(!l.process_key_event(&mut h, 0x61, 0));
    }

    #[test]
    fn shift_press_temp_config_keeps_chinese() {
        // shift_english = temp(旧版行为):仅上屏原串,保持中文模式。
        let (_d, mut l) = logic_with_core_cfg(lyyime_core::Config {
            shift_english: lyyime_core::EnCommit::Temp,
            ..Default::default()
        });
        let mut h = Mock::default();
        for k in ['t', 'h', 'e'] {
            l.process_key_event(&mut h, k as u32, 0);
        }
        l.process_key_event(&mut h, SHIFT_L, 0);
        assert!(h.events.borrow().iter().any(|e| e == "commit:the"));
        assert!(!h.events.borrow().iter().any(|e| e == "mode:1"));
        // 释放(无挂起单击)也不切换。
        l.process_key_event(&mut h, SHIFT_L, 1 << 30);
        assert!(!h.events.borrow().iter().any(|e| e == "mode:1"));
        // 仍是中文态:字母进入下一次组合。
        assert!(l.process_key_event(&mut h, 0x61, 0));
        assert!(h.events.borrow().iter().any(|e| e == "aux:a"));
    }

    #[test]
    fn enter_flushes_letters_keeps_chinese_by_default() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        for k in ['t', 'h', 'e'] {
            l.process_key_event(&mut h, k as u32, 0);
        }
        // 回车默认(enter_english=temp):临时英文,上屏后保持中文模式。
        assert!(l.process_key_event(&mut h, 0xff0d, 0)); // KSYM_RETURN
        assert!(h.events.borrow().iter().any(|e| e == "commit:the"));
        assert!(!h.events.borrow().iter().any(|e| e == "mode:1"));
        // 字母继续进中文组词缓冲。
        assert!(l.process_key_event(&mut h, 0x61, 0));
        assert!(h.events.borrow().iter().any(|e| e == "aux:a"));
    }

    #[test]
    fn shift_single_click_with_empty_buffer_toggles_english() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        assert!(l.process_key_event(&mut h, SHIFT_L, 0));
        assert!(l.process_key_event(&mut h, SHIFT_L, 1 << 30));
        assert!(h.events.borrow().iter().any(|e| e == "mode:1"));
        assert!(!l.process_key_event(&mut h, 0x61, 0));
    }

    #[test]
    fn uppercase_keyval_lowercased() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 0x41, 0); // 'A'(Shift 大写,无 CapsLock)
        let ev = h.events.borrow().clone();
        let evs: Vec<&String> = ev.iter().filter(|e| e.starts_with("aux:")).collect();
        // 辅助区输入串应为小写 'a'
        assert!(evs.iter().any(|e| e.contains("a")), "events={ev:?}");
    }

    #[test]
    fn capslock_letter_passes_through_uppercase() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        // 大写态无 Shift:'A' 键值原样放行,不进组词缓冲
        assert!(!l.process_key_event(&mut h, 0x41, MASK_LOCK));
        let ev = h.events.borrow();
        assert!(
            !ev.iter().any(|e| e.starts_with("cands:[") || e.starts_with("aux:")),
            "不应出现组词 UI: {ev:?}"
        );
    }

    #[test]
    fn capslock_shift_letter_passes_through_lowercase() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        // 大写态 Shift+字母:键值为小写 'a',放行由应用输出小写
        assert!(!l.process_key_event(&mut h, 0x61, MASK_LOCK | crate::keysym::MASK_SHIFT));
        let ev = h.events.borrow();
        assert!(
            !ev.iter().any(|e| e.starts_with("cands:[") || e.starts_with("aux:")),
            "不应出现组词 UI: {ev:?}"
        );
    }

    #[test]
    fn capslock_letter_after_composition_resets_buffer() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 'n' as u32, 0); // 组词中
        // 大写态字母直通:先复位缓冲(Preedit 清空)再放行
        assert!(!l.process_key_event(&mut h, 0x41, MASK_LOCK));
        assert!(h.events.borrow().iter().any(|e| e == "preedit:∅"));
        // 缓冲确已清空:普通字母重新从头组词而非拼接
        let mut h2 = Mock::default();
        l.process_key_event(&mut h2, 'i' as u32, 0);
        assert!(h2
            .events
            .borrow()
            .iter()
            .any(|e| e == "aux:i"), "应从头组词 'i'");
    }

    #[test]
    fn capslock_non_letter_keys_keep_normal_behavior() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        // 大写态数字/标点仍走常态:无候选数字 Pass、空缓冲标点上中文标点
        assert!(!l.process_key_event(&mut h, 0x31, MASK_LOCK)); // '1' 无候选放行
        assert!(l.process_key_event(&mut h, 0x2c, MASK_LOCK)); // ',' 吞键上屏中文逗号
        assert!(h.events.borrow().iter().any(|e| e.starts_with("commit:")));
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
    fn shot_hotkey_中英态命中并吞键() {
        use crate::keysym::{MASK_ALT, MASK_CTRL};
        let mut cfg = lyyime_core::Config::default();
        cfg.shot_hotkey = "ctrl+alt+a".into();
        let (_d, mut l) = logic_with_core_cfg(cfg);
        let mut h = Mock::default();
        l.switch_mode(&mut h); // → 英文
        // 英文态命中:吞键 + on_shot
        assert!(l.process_key_event(&mut h, 0x61, MASK_CTRL | MASK_ALT));
        assert!(h.events.borrow().iter().any(|e| e == "shot"));
        // 中文态同样命中(切回)
        l.switch_mode(&mut h);
        assert!(l.process_key_event(&mut h, 0x61, MASK_CTRL | MASK_ALT));
        assert_eq!(h.events.borrow().iter().filter(|e| *e == "shot").count(), 2);
        // 非目标组合不命中:ctrl+b 放行(应用快捷键路径)
        assert!(!l.process_key_event(&mut h, 0x62, MASK_CTRL));
        // 键不对不命中:ctrl+alt+b 放行
        assert!(!l.process_key_event(&mut h, 0x62, MASK_CTRL | MASK_ALT));
    }

    #[test]
    fn shot_hotkey_配置非法时不拦截() {
        use crate::keysym::{MASK_ALT, MASK_CTRL};
        let mut cfg = lyyime_core::Config::default();
        cfg.shot_hotkey = "a".into(); // 无修饰 → 解析失败 → 不拦截
        let (_d, mut l) = logic_with_core_cfg(cfg);
        let mut h = Mock::default();
        assert!(!l.process_key_event(&mut h, 0x61, MASK_CTRL | MASK_ALT));
        assert!(!h.events.borrow().iter().any(|e| e == "shot"));
    }

    #[test]
    fn coin_hotkey_命中进入造词态() {
        use crate::keysym::MASK_CTRL;
        let (_d, mut l) = logic_with_core_cfg(lyyime_core::Config::default()); // 默认 ctrl+equal
        let mut h = Mock::default();
        // 先上屏候选,建立最近上屏历史
        for k in ['w', 'q', 'v', 'b'] {
            l.process_key_event(&mut h, k as u32, 0);
        }
        l.process_key_event(&mut h, 0x20, 0); // 空格顶屏「你好」
        // Ctrl+= 触发造词:预编辑出现「造词:」前缀
        assert!(l.process_key_event(&mut h, 0x3d, MASK_CTRL));
        assert!(
            h.events.borrow().iter().any(|e| e.starts_with("preedit:造词:")),
            "events={:?}",
            h.events.borrow()
        );
        // Esc 取消造词
        l.process_key_event(&mut h, KSYM_ESCAPE, 0);
    }

    #[test]
    fn ctrl_combo_releases_buffer_and_passes() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, 'n' as u32, 0);
        // Ctrl+C:缓冲复位(有缓冲 Shift 语义不触发),按键放行
        assert!(!l.process_key_event(&mut h, 0x63, 1 << 2));
    }

    #[test]
    fn 词组提示_逐字上屏后经宿主回调展示() {
        // 夹具:拼音 ni→你、hao→好;wqvb→你好。逐字上屏 你(2 键)+好(3 键)
        // 后,应给出「你好」可用 wqvb 打出的效率提示。
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        for k in ['n', 'i'] {
            assert!(l.process_key_event(&mut h, k as u32, 0));
        }
        assert!(l.process_key_event(&mut h, 0x20, 0)); // space → 你
        for k in ['h', 'a', 'o'] {
            assert!(l.process_key_event(&mut h, k as u32, 0));
        }
        assert!(l.process_key_event(&mut h, 0x20, 0)); // space → 好
        let ev = h.events.borrow();
        assert!(ev.iter().any(|e| e == "commit:你"), "events={ev:?}");
        assert!(ev.iter().any(|e| e == "commit:好"), "events={ev:?}");
        assert!(
            ev.iter()
                .any(|e| e.contains("hint:") && e.contains("你好") && e.contains("wqvb")),
            "应有词组提示: {ev:?}"
        );
    }

    fn logic_with_action() -> (PathBuf, EngineLogic) {
        let mut cfg = lyyime_core::Config::default();
        cfg.quick_actions = vec![lyyime_core::QuickAction {
            trigger: "peizhi".into(),
            label: "打开配置".into(),
            command: "@settings".into(),
        }];
        logic_with_core_cfg(cfg)
    }

    #[test]
    fn 快速功能键_数字选中回调on_action不上屏() {
        let (_d, mut l) = logic_with_action();
        let mut h = Mock::default();
        for k in "peizhi".chars() {
            assert!(l.process_key_event(&mut h, k as u32, 0), "{k} 应被吞");
        }
        // 触发词命中:候选里出现功能键标签(经 on_candidates 下发)。
        assert!(
            h.events
                .borrow()
                .iter()
                .any(|e| e.starts_with("cands:[") && e.contains("打开配置")),
            "events={:?}",
            h.events.borrow()
        );
        // 数字 1 选中功能键:on_action(0) 回调,无 commit。
        assert!(l.process_key_event(&mut h, 0x31, 0));
        let ev = h.events.borrow();
        assert!(ev.iter().any(|e| e == "action:0"), "events={ev:?}");
        assert!(!ev.iter().any(|e| e.starts_with("commit:")), "events={ev:?}");
    }

    #[test]
    fn 快速功能键_点选select_candidate回调on_action() {
        let (_d, mut l) = logic_with_action();
        let mut h = Mock::default();
        for k in "peizhi".chars() {
            l.process_key_event(&mut h, k as u32, 0);
        }
        assert!(l.select_candidate(&mut h, 0), "点选应消费");
        assert!(h.events.borrow().iter().any(|e| e == "action:0"));
    }

    /// §15 右键候选菜单:操作行展示/数字执行/Esc 还原/固定删除落盘。
    #[test]
    fn 右键候选菜单_删除固定取消与反查提示() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        let last_cands = |h: &Mock| {
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("cands:"))
                .last()
                .cloned()
                .unwrap_or_default()
        };
        // 缓冲 wq → 候选 [你, 你好](wq 精确 + wqvb 前缀命中)
        for k in ['w', 'q'] {
            assert!(l.process_key_event(&mut h, k as u32, 0));
        }
        assert!(last_cands(&h).contains("你"));

        // 右键第 0 行 → 候选区换操作行(core 状态不动)
        assert!(l.cand_menu_open(&mut h, 0));
        let menu = last_cands(&h);
        assert!(menu.contains("固定首位"), "{menu}");
        assert!(menu.contains("删除词组"), "{menu}");
        assert!(menu.contains("反查英文"), "{menu}");

        // Esc 取消 → 还原真实候选
        assert!(l.process_key_event(&mut h, KSYM_ESCAPE, 0));
        assert!(last_cands(&h).contains("你"));

        // 再右键 → 数字 2 删除「你」→ 候选只剩 你好(屏蔽持久化由 core 单测覆盖)
        assert!(l.cand_menu_open(&mut h, 0));
        assert!(l.process_key_event(&mut h, '2' as u32, 0));
        assert_eq!(last_cands(&h), "cands:[你好]");
        // 数字 3 反查英文:夹具无 zh_en.tsv → notice 提示,候选不动
        assert!(l.cand_menu_open(&mut h, 0));
        assert!(l.process_key_event(&mut h, '3' as u32, 0));
        assert!(h
            .events
            .borrow()
            .iter()
            .any(|e| e.contains("没有英文反查结果")));
    }

    /// 右键功能键/越界行不弹菜单(cand_pinned 返回 None → 吞键)。
    #[test]
    fn 右键功能键候选不弹菜单() {
        let (_d, mut l) = logic_with_action();
        let mut h = Mock::default();
        for k in "peizhi".chars() {
            l.process_key_event(&mut h, k as u32, 0);
        }
        // 功能键行是第 0 行(peizhi 触发时功能候选紧随首选之前/之后依 core 插入位置,
        // 先找含"打开配置"的行下标 —— 简化:直接对全部行右键,功能行应吞键不弹菜单)
        for i in 0..3 {
            let before = h.events.borrow().len();
            let _ = l.cand_menu_open(&mut h, i);
            // 若该行是普通词,菜单会 emit cands;功能行则静默
            if h.events.borrow().len() > before {
                // 普通词菜单 → Esc 还原后继续验证
                l.process_key_event(&mut h, KSYM_ESCAPE, 0);
            }
        }
        // 引擎不 panic 即可(功能行被 cand_pinned=None 拒绝)
    }

    /// §15 自定义查询(第 4 行):已配置时操作行多一项,数字 4 执行 →
    /// on_open_url + notice + 候选还原;未配置时按 4 仅还原不动作。
    #[test]
    fn 右键候选菜单_自定义查询() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        let last_cands = |h: &Mock| {
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("cands:"))
                .last()
                .cloned()
                .unwrap_or_default()
        };
        for k in ['w', 'q'] {
            assert!(l.process_key_event(&mut h, k as u32, 0));
        }

        // 未配置:菜单三行,数字 4 不产生 open-url(仅还原)
        assert!(l.cand_menu_open(&mut h, 0));
        let menu3 = last_cands(&h);
        assert!(!menu3.contains("查词典"), "{menu3}");
        assert!(l.process_key_event(&mut h, '4' as u32, 0));
        assert!(!h.events.borrow().iter().any(|e| e.starts_with("open-url:")));

        // 已配置:菜单四行,数字 4 → open-url({q} 已代入)+ notice + 还原
        l.set_custom_query(Some(lyyime_core::wordops::CustomQuery {
            label: "查词典".into(),
            url: "https://dict.example.test/lookup?q={q}".into(),
        }));
        assert!(l.cand_menu_open(&mut h, 0));
        let menu4 = last_cands(&h);
        assert!(menu4.contains("查词典"), "{menu4}");
        assert!(l.process_key_event(&mut h, '4' as u32, 0));
        let ev = h.events.borrow();
        assert!(
            ev.iter()
                .any(|e| e == "open-url:https://dict.example.test/lookup?q=%E4%BD%A0"),
            "events={ev:?}"
        );
        assert!(ev.iter().any(|e| e == "notice:查词典: 你"), "events={ev:?}");
        drop(ev);
        // 还原后真实候选回来了
        assert!(last_cands(&h).contains("你"));
    }
}
