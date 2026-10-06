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
    KSYM_CONTROL_L, KSYM_CONTROL_R, KSYM_DELETE, KSYM_DOWN, KSYM_END, KSYM_ESCAPE,
    KSYM_F1, KSYM_F12, KSYM_HOME, KSYM_KP_0, KSYM_KP_1, KSYM_KP_9, KSYM_KP_ENTER,
    KSYM_LEFT, KSYM_PAGE_DOWN, KSYM_PAGE_UP, KSYM_PERIOD, KSYM_RETURN, KSYM_RIGHT,
    KSYM_SPACE, KSYM_TAB, KSYM_UP, MASK_CTRL, MASK_LOCK, MASK_RELEASE, MASK_SHIFT,
    PURPOSE_PASSWORD, PURPOSE_PIN,
};
use lyyime_ai::AiConfig;
use lyyime_core::{CandOp, Effect, Engine, LKey, Mode};
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
    /// 菜单触发确认(2026-09-30,与 Mode B 同合同):上屏中文命中菜单功能名后
    /// 用户按下配置的无修饰 Fn;index = core 可信目录 MENU_CATALOG 下标,
    /// 宿主按目录动作分派(OpenSettings/Help/EnglishMode/…),绝不落 shell。
    fn on_menu_action(&mut self, index: usize);
    fn on_menu_general(&mut self, id: &str);
    /// 撤下辅助区的菜单触发提示:仅在确认辅助区当前仍是我们贴的提示时
    /// 调用(词组提示/notice/候选 aux 会覆盖它,各自在 dispatch 内解除
    /// 归属标记);宿主隐藏辅助区,不影响预编辑/候选。
    fn on_menu_hint_clear(&mut self);
    /// Shift 单击确认英文→中文且 release state 仍带 LockMask 时,
    /// 请求宿主解除系统 CapsLock(恰好一次)。默认空实现。
    fn on_caps_lock_off(&mut self) {}
}

pub const GA_SETTINGS: &str = "settings";
pub const GA_SETTINGS_INPUT: &str = "settings_input";
pub const GA_SETTINGS_SKIN: &str = "settings_skin";
pub const GA_TOGGLE_MODE: &str = "toggle_mode";
pub const GA_TOGGLE_PINYIN: &str = "toggle_pinyin_only";
pub const GA_TOGGLE_PUNCT: &str = "toggle_cn_punct";
pub const GA_TOGGLE_LEARN: &str = "toggle_learning";
pub const GA_TOGGLE_PRED: &str = "toggle_prediction";
pub const GA_TOGGLE_QA: &str = "toggle_quick_actions";
pub const GA_SHOT: &str = "shot";
pub const GA_RELOAD: &str = "reload_dict";
#[derive(Clone)]
enum MenuRow {
    Word(CandOp),
    Query,
    General(&'static str),
}
struct CandMenu {
    cand_idx: Option<usize>,
    items: Vec<(String, MenuRow)>,
    page: usize,
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
    cand_menu: Option<CandMenu>,
    /// §15 自定义查询(菜单第 4 行;config.toml custom_query_*,
    /// 焦点进入时热读 —— 设置保存后即时生效)。None = 操作行不显示。
    custom_query: Option<lyyime_core::wordops::CustomQuery>,
    /// 菜单触发状态机(2026-09-30):独立纯状态机,不属于 engine/Plan;
    /// 只消费真实上屏文本(dispatch 的 Commit 效果,AI 采集态除外)。
    menu_trigger: lyyime_core::MenuTrigger,
    /// 辅助区当前显示的是我们贴的菜单提示:为 true 才可撤下
    /// (词组提示/notice/候选 aux 覆盖后在 dispatch 内置 false)
    menu_hint_shown: bool,
    /// 确认 Fn 的按下被吞 → 配对吞掉同键 release(防 release 泄给应用)
    mt_eat_release: Option<u32>,
    /// AI 采集态暂存的联想开关值(采集期关联想,退出时恢复;
    /// Some 只出现在采集进入时原开关为开的情形——其实存原值通用)。
    ai_pred_saved: Option<bool>,
    /// Ctrl+. 标点切换:press 已吞 → 配对 release 一并吞掉并清锁存;
    /// 兼作按住连发的去抖(按住不放只翻转一次)。会话级,不写盘。
    punct_key_down: bool,
    pub ui_gen: u64,
    pub ui_focused: bool,
    pub ui_enabled: bool,
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
        let mut menu_trigger = lyyime_core::MenuTrigger::new();
        menu_trigger.configure(
            core_cfg.menu_trigger_enabled,
            core_cfg.menu_trigger_key.min(255) as u8,
            &core_cfg.menu_trigger_disabled,
        );
        menu_trigger.set_shot_hotkey(&core_cfg.shot_hotkey);
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
            menu_trigger,
            menu_hint_shown: false,
            mt_eat_release: None,
            ai_pred_saved: None,
            punct_key_down: false,
            ui_gen: 0,
            ui_focused: false,
            ui_enabled: false,
        }
    }

    pub fn ui_gen(&self) -> u64 {
        self.ui_gen
    }

    pub fn ui_gate_open(&self) -> bool {
        self.ui_enabled
            && self.ui_focused
            && self.input_purpose != PURPOSE_PASSWORD
            && self.input_purpose != PURPOSE_PIN
    }

    pub fn ui_invalidate(&mut self) -> u64 {
        self.ui_gen = self.ui_gen.wrapping_add(1);
        self.ui_gen
    }

    /// 菜单触发配置热读(focus_in 时调用,设置保存即生效;configure 内部即复位)
    pub fn set_menu_trigger_cfg(&mut self, enabled: bool, key: usize, disabled: &str) {
        self.menu_trigger.configure(enabled, key.min(255) as u8, disabled);
    }

    /// 与提示同步热读实际工具快捷键;冲突由配置读取入口统一消解。
    pub fn set_tool_hotkeys(&mut self, coin: &str, shot: &str) {
        self.core_cfg.coin_hotkey = coin.to_owned();
        self.core_cfg.shot_hotkey = shot.to_owned();
        self.hotkey_coin = parse_hotkey(coin);
        self.hotkey_shot = parse_hotkey(shot);
        self.menu_trigger.set_shot_hotkey(shot);
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

    /// 联想开关热读(focus_in 时调用,设置保存即生效;
    /// core set_config 关闭时会清掉正在展示的联想行与上下文)。
    pub fn set_next_word_prediction(&mut self, on: bool) {
        self.core_cfg.next_word_prediction = on;
        if self.ai_state == AiState::Capture {
            // 采集态联想被强行关闭;新配置记进暂存,退出采集时恢复
            self.ai_pred_saved = Some(on);
            return;
        }
        if let Some(e) = self.engine.as_mut() {
            let mut cfg = e.config().clone();
            if cfg.next_word_prediction != on {
                cfg.next_word_prediction = on;
                e.set_config(cfg);
            }
        }
    }

    /// 中文标点默认热读(focus_in 时调用,设置保存即生效)。
    /// `core_cfg.cn_punct` 记的是保存的默认值而非运行时状态:
    /// 默认值未变时不碰引擎 —— Ctrl+. 的本次运行切换跨焦点保留;
    /// 默认值变了才走窄化 setter 下发(只改开关并复位引号开合位,
    /// 不动组合缓冲/候选/联想行,不触发词表重载)。
    pub fn set_punctuation_default(&mut self, on: bool) {
        if self.core_cfg.cn_punct != on {
            self.core_cfg.cn_punct = on;
            if let Some(e) = self.engine.as_mut() {
                e.set_chinese_punctuation(on);
            }
        }
    }

    pub fn runtime_cn_punct(&self) -> Option<bool> {
        self.engine.as_ref().map(|e| e.config().cn_punct)
    }

    pub fn set_chinese_punctuation(&mut self, on: bool) {
        self.core_cfg.cn_punct = on;
        if let Some(e) = self.engine.as_mut() {
            e.set_chinese_punctuation(on);
        }
    }

    pub fn set_pinyin_only(&mut self, host: &mut dyn Host, on: bool) -> bool {
        if self.core_cfg.pinyin_only == on {
            return false;
        }
        self.core_cfg.pinyin_only = on;
        let changed = self
            .engine
            .as_mut()
            .map(|e| e.set_pinyin_only(on))
            .unwrap_or(false);
        if changed {
            self.cand_menu = None;
            self.preedit = None;
            self.menu_trigger.reset();
            self.mt_clear_hint(host);
            host.on_preedit(None);
            host.on_candidates(&[], 0, 0, "");
        }
        changed
    }
    pub fn set_learning(&mut self, on: bool) {
        if self.core_cfg.learning != on {
            self.core_cfg.learning = on;
            if let Some(e) = self.engine.as_mut() {
                e.set_learning(on);
            }
        }
    }
    pub fn set_mixed_en(&mut self, on: bool) {
        if self.core_cfg.mixed_en != on {
            self.core_cfg.mixed_en = on;
            if let Some(e) = self.engine.as_mut() {
                e.set_mixed_en(on);
            }
        }
    }
    pub fn set_quick_actions_enabled(&mut self, on: bool) {
        if self.core_cfg.quick_actions_enabled != on {
            self.core_cfg.quick_actions_enabled = on;
            if let Some(e) = self.engine.as_mut() {
                e.set_quick_actions_enabled(on);
            }
        }
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
            return self.on_release(host, keyval, state);
        }
        self.on_press(host, keyval, state)
    }

    fn on_press(&mut self, host: &mut dyn Host, keyval: u32, state: u32) -> bool {
        // §15 右键操作行激活期间:数字 1-3(自定义查询已配则到 4)执行、
        // Esc 取消还原、其余键(含 Shift/组合键)先还原真实候选再按常态
        // 路径处理 ——core 侧缓冲/候选从未改动,菜单纯属显示层状态。
        if self.cand_menu.is_some() && state & BLOCKING_MODS == 0 {
            match keyval {
                KSYM_ESCAPE => {
                    self.cand_menu_restore(host);
                    return true;
                }
                KSYM_PAGE_UP | 0x2d => return self.cand_menu_flip(host, false),
                KSYM_PAGE_DOWN | 0x3d => return self.cand_menu_flip(host, true),
                k @ 0x31..=0x39 => return self.cand_menu_exec(host, (k - 0x31) as usize),
                0x30 => return self.cand_menu_exec(host, 9),
                k @ KSYM_KP_1..=KSYM_KP_9 => {
                    return self.cand_menu_exec(host, (k - KSYM_KP_1) as usize)
                }
                KSYM_KP_0 => return self.cand_menu_exec(host, 9),
                _ => self.cand_menu_restore(host),
            }
        } else if self.cand_menu.is_some() {
            self.cand_menu_restore(host);
        }
        if is_shift(keyval) {
            // Shift 按下=菜单触发硬边界(与 Mode B 同合同):模式是否切换
            // 取决于 release,组合或单击都不再保留尾串/待执行。
            self.menu_trigger.reset();
            self.mt_clear_hint(host);
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
            let _consumed = self.dispatch(host, effects, true);
            if !had_composition {
                self.pending_shift = Some(keyval);
            }
            return true; // 与 python 版一致:Shift 按下一律吞键
        }
        // 其它键按下:无论成败都取消未决的 Shift 单击
        self.pending_shift = None;
        // ---- 菜单触发(2026-09-30,与 Mode B 同合同):待执行期间仅无修饰的
        // 配置 Fn 吞键执行一次;编辑/导航/Esc 与修饰组合=硬边界复位尾串,
        // 其余普通键仅取消待执行(尾串保留可续接)。AI 采集态冻结匹配器,
        // 采集态按键全部归提示词路径,不进此分支。 ----
        if self.ai_state != AiState::Capture && self.mt_press(host, keyval, state) {
            return true;
        }
        // ---- 中英文标点切换(固定保留键 Ctrl+.;仅中文态) ----
        // 纯 Control_L/R 按下直接放行、不进 core(OTHER 会清组合缓冲):
        // 先按 Ctrl 再按 . 的预热键不得打断正在输入的组合。
        if matches!(keyval, KSYM_CONTROL_L | KSYM_CONTROL_R) {
            return false;
        }
        // 干净修饰恰为 Ctrl(Caps/NumLock 不算修饰;Ctrl+Shift+/Alt/Super
        // 不命中):中文态下翻转运行时标点开关(不写盘),吞掉 press;
        // 配对 release 由 punct_key_down 吞掉,按住连发只翻转一次。
        // 组合/联想行在时静默切换,不打扰输入;仅空闲态亮状态提示。
        if self.mode == 0
            && keyval == KSYM_PERIOD
            && (state & (BLOCKING_MODS | MASK_SHIFT)) == MASK_CTRL
        {
            if !self.punct_key_down {
                self.punct_key_down = true;
                let on = self.core_mut().toggle_chinese_punctuation();
                let idle = self
                    .engine
                    .as_ref()
                    .is_some_and(|e| e.buffer().is_empty() && e.flush_page().is_empty());
                if idle && self.ai_state != AiState::Capture {
                    host.on_hint(if on {
                        "中文标点:，。？！"
                    } else {
                        "英文标点:,.?!"
                    });
                }
            }
            return true;
        }
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
                return self.dispatch(host, effects, true);
            }
        }
        if state & BLOCKING_MODS != 0 {
            // 应用快捷键(如 Ctrl+C):先放弃 AI 会话并复位缓冲,再放行
            self.ai_reset_state();
            let effects = self.core_mut().process_key(LKey::Other);
            return self.dispatch(host, effects, true);
        }
        if let Some(taken) = self.ai_take(host, keyval) {
            return taken;
        }
        // CapsLock 大写态(合同 §6):字母键一律直通英文,不进组词缓冲。
        // 无 Shift 时键值即大写,应用输出大写字母;Shift+字母 键值为小写,
        // 应用按 Caps+Shift 翻译输出小写字母。非字母键不受影响走常态。
        if state & MASK_LOCK != 0 && matches!(keyval, 0x41..=0x5a | 0x61..=0x7a) {
            let effects = self.core_mut().process_key(LKey::Other);
            return self.dispatch(host, effects, true);
        }
        let (lkey, ch) = map_keyval(keyval);
        let effects = self.core_mut().process_key(lkey);
        let _ = ch;
        self.dispatch(host, effects, true)
    }

    /// 撤下我们贴的菜单触发提示:仅当辅助区仍是我们的内容时才发隐藏
    /// 回调(menu_hint_shown 为 false = 已被词组提示/notice/候选 aux
    /// 覆盖,此时不动辅助区)。
    fn mt_clear_hint(&mut self, host: &mut dyn Host) {
        if self.menu_hint_shown {
            self.menu_hint_shown = false;
            host.on_menu_hint_clear();
        }
    }

    /// 菜单触发按键分类:确认 Fn(无修饰;CapsLock/NumLock 不影响)且有待
    /// 执行 → 吞键,一次性取出下标回调宿主执行,返回 true;其余按键只按
    /// 边界语义复位/取消,不吞键(未匹配的 F 键照常放行给应用)。
    /// 进入即撤下我们贴的提示条:任何下一次按键都终止「提示持续期」。
    fn mt_press(&mut self, host: &mut dyn Host, keyval: u32, state: u32) -> bool {
        self.mt_clear_hint(host);
        let fn_num = if (KSYM_F1..=KSYM_F12).contains(&keyval) {
            (keyval - KSYM_F1 + 1) as u8
        } else {
            0
        };
        let mods = state & (BLOCKING_MODS | MASK_SHIFT); // Caps/NumLock 不算修饰
        if fn_num > 0
            && mods == 0
            && self.menu_trigger.pending().is_some()
            && fn_num == self.menu_trigger.key()
        {
            if let Some(idx) = self.menu_trigger.take_pending() {
                self.menu_trigger.reset(); // 动作执行=硬边界,防重入
                self.mt_eat_release = Some(keyval); // 配对吞掉同键 release
                host.on_menu_action(idx);
                return true;
            }
        }
        if mods != 0 || Self::mt_is_reset_key(keyval) {
            self.menu_trigger.reset();
        } else {
            self.menu_trigger.cancel_pending();
        }
        false
    }

    /// 编辑/导航类按键 → 尾串硬边界(与 Mode B mt_is_reset_key 同清单)
    fn mt_is_reset_key(keyval: u32) -> bool {
        matches!(
            keyval,
            KSYM_BACKSPACE
                | KSYM_TAB
                | KSYM_ESCAPE
                | KSYM_DELETE
                | KSYM_HOME
                | KSYM_END
                | KSYM_LEFT
                | KSYM_UP
                | KSYM_RIGHT
                | KSYM_DOWN
                | KSYM_PAGE_UP
                | KSYM_PAGE_DOWN
        )
    }

    fn on_release(&mut self, host: &mut dyn Host, keyval: u32, state: u32) -> bool {
        // 菜单触发确认 Fn 的按下已被吞 → 配对吞掉同键 release;
        // 未匹配的 F 键 release 照常放行给应用
        if self.mt_eat_release == Some(keyval) {
            self.mt_eat_release = None;
            return true;
        }
        // Ctrl+. 的 press 已吞 → 配对吞掉同键 release 并清锁存
        if self.punct_key_down && keyval == KSYM_PERIOD {
            self.punct_key_down = false;
            return true;
        }
        if is_shift(keyval) {
            let pending = self.pending_shift.take();
            if pending == Some(keyval) {
                // 按下后无其它键 → 判定单击:切换中英(合同 §6)
                self.switch_mode(host);
                // 英→中确认且事件 state 仍带 LockMask:通知宿主解锁
                if self.mode == 0 && state & MASK_LOCK != 0 {
                    host.on_caps_lock_off();
                }
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
        // AI 会话进出=菜单触发硬边界:尾串与待执行一起清
        self.menu_trigger.reset();
        self.ai_state = AiState::Idle;
        self.ai_prompt.clear();
        self.ai_prompt_full = false;
        // 退出采集态:恢复采集进入时暂存的联想开关
        if let Some(saved) = self.ai_pred_saved.take() {
            if let Some(e) = self.engine.as_mut() {
                let mut cfg = e.config().clone();
                cfg.next_word_prediction = saved;
                e.set_config(cfg);
            }
        }
    }

    fn ai_cancel(&mut self, host: &mut dyn Host) {
        self.ai_reset_state();
        self.menu_hint_shown = false; // 下方空候选会隐藏辅助区
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
            self.dispatch(host, effects, true);
        }
        let (lkey, _ch) = map_keyval(keyval);
        let effects = self.core_mut().process_key(lkey);
        self.dispatch(host, effects, true)
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
                    self.menu_trigger.reset(); // 进入 AI 会话=菜单触发硬边界
                    self.mt_clear_hint(host);
                    self.ai_prompt.clear();
                    self.preedit = None;
                    // 采集态关联想:空格/数字不得把联想尾巴灌进提示词,
                    // 联想行也不能出现在 /AI 展示帧里。原值暂存,退出恢复。
                    if let Some(e) = self.engine.as_mut() {
                        let mut cfg = e.config().clone();
                        self.ai_pred_saved = Some(cfg.next_word_prediction);
                        if cfg.next_word_prediction {
                            cfg.next_word_prediction = false;
                            e.set_config(cfg);
                        }
                    }
                    // 关掉联想只清了 core 内部状态,已展示的联想行要主动撤下
                    host.on_candidates(&[], 0, 0, "");
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

    /// 效果流分发。`press`=源自按键按下:Pass 效果构成"直通边界"→
    /// 菜单触发尾串与待执行一并复位;release/宿主驱动路径(select/翻页/
    /// 右键执行)传 false,Pass 不复位匹配器。
    fn dispatch(&mut self, host: &mut dyn Host, effects: Vec<Effect>, press: bool) -> bool {
        // 逐条执行效果;返回 false 仅当出现 pass(宿主必须放行该键)
        let mut consumed = true;
        // 菜单触发:本次效果流最后一条真实上屏产生的命中提示,压轴展示
        // (盖过同帧候选清屏与词组提示);同帧新预编辑(四码顶屏续打)或
        // 最终帧仍有候选行(联想行)时不抢贴
        let mut mt_hint: Option<String> = None;
        let mut mt_preedit = false;
        let mut mt_cands = false; // 本帧最终候选行数 > 0(联想/新组合)
        for eff in effects {
            match eff {
                Effect::Commit(text) => {
                    if !text.is_empty() {
                        self.record_stats(&text);
                        host.on_commit(&text);
                        // 菜单触发:真实上屏喂匹配器(AI 采集态 commit 走
                        // dispatch_ai 进提示词,天然不经此分支)
                        if self.ai_state != AiState::Capture {
                            mt_hint = self.menu_trigger.on_commit(&text);
                        }
                    }
                }
                Effect::Preedit(p) => {
                    // 缺失 / 空串都视为清除
                    self.preedit = p.filter(|s| !s.is_empty());
                    mt_preedit = self.preedit.is_some();
                    host.on_preedit(self.preedit.as_deref());
                }
                Effect::Candidates(cands) => {
                    // 候选 aux 会覆盖辅助区:菜单提示所有权即时解除
                    self.menu_hint_shown = false;
                    mt_cands = !cands.is_empty();
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
                Effect::Pass => {
                    consumed = false;
                    // 直通文本=菜单触发硬边界:本帧命中提示不得再贴;
                    // 仅按键按下路径复位匹配器(空缓冲 Enter/CapsLock 字母/
                    // 转发标点/未处理功能键),release 与宿主驱动不复位
                    mt_hint = None;
                    mt_preedit = false;
                    mt_cands = false;
                    if press {
                        self.menu_trigger.reset();
                        self.mt_clear_hint(host);
                    }
                }
                Effect::Consumed => {}
                Effect::Notice(t) => {
                    // notice 覆盖辅助区 → 菜单提示所有权解除
                    self.menu_hint_shown = false;
                    host.on_notice(&t);
                }
                Effect::Hint(t) => {
                    // 词组提示覆盖辅助区 → 菜单提示所有权解除
                    self.menu_hint_shown = false;
                    host.on_hint(&t);
                }
                Effect::Action(i) => host.on_action(i),
                Effect::ModeChanged(m) => {
                    self.mode = m as u8;
                    host.on_mode_changed(self.mode);
                    // 模式切换=菜单触发硬边界:尾串/待执行/命中提示全清
                    self.menu_trigger.reset();
                    self.mt_clear_hint(host);
                    mt_hint = None;
                    mt_preedit = false;
                    mt_cands = false;
                }
            }
        }
        // 同帧出现新预编辑(四码顶屏续打)或最终帧仍有候选行(联想行、
        // 顶屏续打的新组合):命中提示已不可见 → 待执行一并取消,
        // Fn 不得执行看不见的动作;尾串保留供续接
        if mt_preedit || mt_cands {
            self.menu_trigger.cancel_pending();
            self.mt_clear_hint(host);
        }
        // 菜单触发提示压轴:本帧 commit/cands/hint 已落完且无复位
        if let Some(h) = mt_hint {
            if !mt_preedit && !mt_cands {
                host.on_hint(&h);
                self.menu_hint_shown = true;
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
        self.menu_trigger.reset(); // 模式切换=菜单触发硬边界
        self.menu_hint_shown = false; // 下方空候选会隐藏辅助区
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
        self.ai_reset_state(); // 内含 menu_trigger.reset()(AI/会话边界)
        self.menu_hint_shown = false; // 下方空候选会隐藏辅助区
        self.cand_menu = None;
        self.punct_key_down = false; // 会话边界只清锁存,不动标点运行时开关
        self.preedit = None;
        host.on_preedit(None);
        host.on_candidates(&[], 0, 0, "");
        host.on_mode_changed(self.mode);
    }

    pub fn reset_ui_session(&mut self, host: &mut dyn Host) -> u64 {
        self.reset_session(host);
        self.ui_invalidate()
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
        self.dispatch(host, effects, false)
    }

    /// 候选窗面板翻页(候选窗「<」「>」按钮 / 滚轮,合同 §6):与键盘 `-`/`=`
    /// 同一条 core 路径(LKey::PageUp/PageDown),边界由 core 钳制不循环;
    /// 造词模式下同键承担多选/少选一字(§12)。
    /// 返回 true=已消费(无候选/单页时 core 给 Pass → false,面板无需刷新)。
    pub fn flip_page(&mut self, host: &mut dyn Host, down: bool) -> bool {
        if self.degraded() {
            return false;
        }
        if self.cand_menu.is_some() {
            return self.cand_menu_flip(host, down);
        }
        let key = if down { LKey::PageDown } else { LKey::PageUp };
        let effects = self.core_mut().process_key(key);
        self.dispatch(host, effects, false)
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
            return false;
        };
        let mut items: Vec<(String, MenuRow)> = Vec::new();
        let pin_label = if pinned { "取消固定首位" } else { "固定首位" };
        items.push((pin_label.to_string(), MenuRow::Word(CandOp::PinToggle)));
        items.push(("删除词组".to_string(), MenuRow::Word(CandOp::Delete)));
        items.push(("反查英文".to_string(), MenuRow::Word(CandOp::EnLookup)));
        if let Some(cq) = &self.custom_query {
            items.push((cq.menu_label().to_string(), MenuRow::Query));
        }
        self.cand_menu = Some(CandMenu {
            cand_idx: Some(idx),
            items,
            page: 0,
        });
        self.cand_menu_emit(host);
        true
    }

    pub fn cand_menu_active(&self) -> bool {
        self.cand_menu.is_some()
    }

    pub fn cand_menu_items(&self) -> Vec<String> {
        self.cand_menu
            .as_ref()
            .map(|m| m.items.iter().map(|(l, _)| l.clone()).collect())
            .unwrap_or_default()
    }

    pub fn cand_menu_select_absolute(&mut self, host: &mut dyn Host, index: usize) -> bool {
        if self.cand_menu.is_none() {
            return false;
        }
        let ps = self.menu_page_size();
        if let Some(menu) = self.cand_menu.as_mut() {
            menu.page = index / ps;
        }
        self.cand_menu_exec(host, index % ps)
    }

    pub fn cand_menu_cancel(&mut self, host: &mut dyn Host) {
        if self.cand_menu.is_some() {
            self.cand_menu_restore(host);
        }
    }
    pub fn cand_menu_open_general(&mut self, host: &mut dyn Host) -> bool {
        if self.degraded() {
            return false;
        }
        self.cand_menu = Some(CandMenu {
            cand_idx: None,
            items: self.menu_general_rows(),
            page: 0,
        });
        self.cand_menu_emit(host);
        true
    }
    fn menu_general_rows(&self) -> Vec<(String, MenuRow)> {
        let cfg = self.engine.as_ref().map(|e| e.config());
        let (cn, pure, punct, learn, pred, qa) = cfg
            .map(|c| {
                (
                    self.mode == Mode::Chinese as u8,
                    c.pinyin_only,
                    c.cn_punct,
                    c.learning,
                    c.next_word_prediction,
                    c.quick_actions_enabled,
                )
            })
            .unwrap_or((true, false, true, true, false, true));
        let on_off = |b: bool| if b { "开" } else { "关" };
        vec![
            ("设置…".to_string(), MenuRow::General(GA_SETTINGS)),
            ("输入设置…".to_string(), MenuRow::General(GA_SETTINGS_INPUT)),
            ("皮肤设置…".to_string(), MenuRow::General(GA_SETTINGS_SKIN)),
            (
                if cn { "切换到英文" } else { "切换到中文" }.to_string(),
                MenuRow::General(GA_TOGGLE_MODE),
            ),
            (
                format!(
                    "输入方案:{}(点击切换)",
                    if pure { "纯拼音" } else { "五笔/拼音混输" }
                ),
                MenuRow::General(GA_TOGGLE_PINYIN),
            ),
            (
                format!("中文标点:{}", on_off(punct)),
                MenuRow::General(GA_TOGGLE_PUNCT),
            ),
            (
                format!("用户词学习:{}", on_off(learn)),
                MenuRow::General(GA_TOGGLE_LEARN),
            ),
            (
                format!("上屏后联想:{}", on_off(pred)),
                MenuRow::General(GA_TOGGLE_PRED),
            ),
            (
                format!("快速功能键:{}", on_off(qa)),
                MenuRow::General(GA_TOGGLE_QA),
            ),
            ("截屏".to_string(), MenuRow::General(GA_SHOT)),
            ("重载词库".to_string(), MenuRow::General(GA_RELOAD)),
        ]
    }
    fn menu_page_size(&self) -> usize {
        self.core_cfg.page_size.clamp(1, 10)
    }
    fn cand_menu_emit(&self, host: &mut dyn Host) {
        let Some(menu) = self.cand_menu.as_ref() else {
            return;
        };
        let ps = self.menu_page_size();
        let pages = menu.items.len().max(1).div_ceil(ps);
        let page = menu.page.min(pages - 1);
        let start = page * ps;
        let rows: Vec<(String, String)> = menu.items[start..(start + ps).min(menu.items.len())]
            .iter()
            .map(|(label, _)| (label.clone(), "菜单".to_string()))
            .collect();
        host.on_candidates(
            &rows,
            page,
            pages,
            "菜单:数字/点选执行,-/=翻页,Esc 取消",
        );
    }
    fn cand_menu_flip(&mut self, host: &mut dyn Host, down: bool) -> bool {
        let ps = self.menu_page_size();
        let Some(menu) = self.cand_menu.as_mut() else {
            return false;
        };
        // 自定义查询(config.toml custom_query_*;url 未配则只有三行)
        let pages = menu.items.len().max(1).div_ceil(ps);
        if down {
            if menu.page + 1 >= pages {
                return true;
            }
            menu.page += 1;
        } else {
            if menu.page == 0 {
                return true;
            }
            menu.page -= 1;
        }
        self.cand_menu_emit(host);
        true
    }

    /// 菜单激活时执行操作行(sel=0/1/2 core 操作,sel=3 自定义查询);
    /// 越界选择仅还原显示。
    fn cand_menu_exec(&mut self, host: &mut dyn Host, sel: usize) -> bool {
        let Some(menu) = self.cand_menu.take() else {
            return false;
        };
        // 第 4 行=自定义查询:宿主侧打开浏览器,core 状态不动;先还原真实
        // 候选再发 notice(顺序对调会被候选刷新盖掉),与 Esc 同一还原流。
        let ps = self.menu_page_size();
        if sel >= ps {
            self.cand_menu_restore(host);
            return true;
        }
        let gidx = menu.page * ps + sel;
        let Some((_, row)) = menu.items.get(gidx) else {
            self.cand_menu_restore(host);
            return true;
        };
        match row.clone() {
            MenuRow::Word(op) => {
                let orig = menu.cand_idx.unwrap_or(gidx);
                let effects = self.core_mut().cand_op(orig, op);
                self.dispatch(host, effects, false)
            }
            MenuRow::Query => {
                self.cand_menu_restore(host);
                if let (Some(cq), Some(orig)) = (self.custom_query.clone(), menu.cand_idx)
                {
                    if let Some(word) = self
                        .engine
                        .as_ref()
                        .and_then(|e| e.flush_page().get(orig).map(|c| c.text.clone()))
                    {
                        let url = lyyime_core::wordops::custom_query_url(&cq.url, &word);
                        host.on_open_url(&url);
                        host.on_notice(&format!("{}: {}", cq.menu_label(), word));
                    }
                }
                true
            }
            MenuRow::General(id) => {
                self.cand_menu_restore(host);
                host.on_menu_general(id);
                true
            }
        }
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
        fn on_menu_action(&mut self, index: usize) {
            self.log(format!("menu:{index}"));
        }
        fn on_menu_general(&mut self, id: &str) {
            self.log(format!("general:{id}"));
        }
        fn on_menu_hint_clear(&mut self) {
            self.log("menuhintclr".to_string());
        }
        fn on_caps_lock_off(&mut self) {
            self.log("caps-off".to_string());
        }
    }

    /// 极简词库夹具:wqvb→你好(五笔)、ni→你(拼音)、数字选词由 core 驱动。
    fn fixtures_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = loop {
            let d = std::env::temp_dir().join(format!(
                "lyyime-ibus-logic-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            match std::fs::create_dir(&d) {
                Ok(()) => break d,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        };
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

    fn test_engine_logic(
        dir: PathBuf,
        ai_cfg: Option<AiConfig>,
        mut core_cfg: lyyime_core::Config,
    ) -> EngineLogic {
        if core_cfg.user_dict.is_none() {
            core_cfg.user_dict = Some(fixtures_dir_user());
        }
        let mut logic = EngineLogic::new(dir, ai_cfg, core_cfg);
        logic.stats_dir = None; // 单测不写真实 HOME 的统计目录
        logic
    }

    fn logic_with_ai(cfg: Option<AiConfig>) -> (PathBuf, EngineLogic) {
        let dir = fixtures_dir();
        let logic = test_engine_logic(dir.clone(), cfg, lyyime_core::Config::default());
        (dir, logic)
    }

    fn logic_with_core_cfg(core_cfg: lyyime_core::Config) -> (PathBuf, EngineLogic) {
        let dir = fixtures_dir();
        let logic = test_engine_logic(dir.clone(), None, core_cfg);
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
        let d = loop {
            let d = std::env::temp_dir().join(format!(
                "lyyime-ibus-logic-user-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            match std::fs::create_dir(&d) {
                Ok(()) => break d,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        };
        d.join("user.tsv")
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
        let logic = test_engine_logic(
            dir.clone(),
            None,
            lyyime_core::Config {
                // 显式 5/页:默认 page_size=10 时单码 9 候选(§5.2 截断)只有 1 页
                page_size: 5,
                commit_unique_four: false,
                ..Default::default()
            },
        );
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
    fn paging_symbols_turn_pages_without_committing() {
        for (up, down) in [('-' as u32, '=' as u32), (KSYM_PAGE_UP, KSYM_PAGE_DOWN)] {
            let (_d, mut l) = logic_paging();
            let mut h = Mock::default();
            assert!(l.process_key_event(&mut h, 'a' as u32, 0));
            let page1 = last_nonempty_cands(&h);
            assert_eq!(l.page_pos(), (0, 2));
            for (key, page) in [(up, 0), (down, 1), (down, 1), (up, 0)] {
                assert!(l.process_key_event(&mut h, key, 0));
                assert!(!l.process_key_event(&mut h, key, MASK_RELEASE));
                assert_eq!(l.page_pos(), (page, 2));
                assert_eq!(l.core_mut().buffer(), "a");
                assert!(!h.events.borrow().iter().any(|e| e.starts_with("commit:")));
            }
            assert_eq!(last_nonempty_cands(&h), page1);
            assert!(l.process_key_event(&mut h, down, 0));
            let page2 = last_nonempty_cands(&h);
            assert_ne!(page2, page1);
            let first = page2[7..].trim_end_matches(']').split(',').next().unwrap();
            assert!(l.process_key_event(&mut h, '1' as u32, 0));
            assert!(h.events.borrow().iter().any(|e| e == &format!("commit:{first}")));
        }
    }

    #[test]
    fn paging_symbols_without_candidates_pass_through() {
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        for key in ['-', '='] {
            assert!(!l.process_key_event(&mut h, key as u32, 0));
            assert!(!h.events.borrow().iter().any(|e| e.starts_with("commit:")));
        }
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
    fn shift_click_english_to_chinese_requests_caps_unlock() {
        // 英文→中文 Shift 单击确认且 release state 带 LockMask:
        // on_caps_lock_off 恰好一次;左/右 Shift 两个变体同合同。
        for shift in [SHIFT_L, 0xffe2u32] {
            let (_d, mut l) = logic_with_ai(None);
            let mut h = Mock::default();
            l.switch_mode(&mut h); // → 英文
            h.events.borrow_mut().clear();
            assert!(l.process_key_event(&mut h, shift, MASK_LOCK));
            assert!(l.process_key_event(&mut h, shift, MASK_LOCK | MASK_RELEASE));
            assert_eq!(l.mode, 0, "shift=0x{shift:x} 应回中文态");
            let n = h
                .events
                .borrow()
                .iter()
                .filter(|e| *e == "caps-off")
                .count();
            assert_eq!(n, 1, "shift=0x{shift:x} events={:?}", h.events.borrow());
        }
    }

    #[test]
    fn shift_click_without_lock_never_requests_caps_unlock() {
        // 无 LockMask:单击照常切换,但不请求解锁。
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.switch_mode(&mut h); // → 英文
        h.events.borrow_mut().clear();
        l.process_key_event(&mut h, SHIFT_L, 0);
        l.process_key_event(&mut h, SHIFT_L, MASK_RELEASE);
        assert_eq!(l.mode, 0);
        assert!(!h.events.borrow().iter().any(|e| e == "caps-off"));
    }

    #[test]
    fn shift_click_chinese_to_english_never_requests_caps_unlock() {
        // 中→英方向即使 CapsLock 仍锁存也不触发解锁。
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.process_key_event(&mut h, SHIFT_L, MASK_LOCK);
        l.process_key_event(&mut h, SHIFT_L, MASK_LOCK | MASK_RELEASE);
        assert_eq!(l.mode, 1, "应切到英文态");
        assert!(!h.events.borrow().iter().any(|e| e == "caps-off"));
    }

    #[test]
    fn shift_combos_never_request_caps_unlock() {
        // Shift+字母:挂起被字母取消,release 不切换不解锁。
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.switch_mode(&mut h); // → 英文
        h.events.borrow_mut().clear();
        l.process_key_event(&mut h, SHIFT_L, MASK_LOCK);
        l.process_key_event(&mut h, 'a' as u32, MASK_LOCK | MASK_SHIFT);
        l.process_key_event(&mut h, SHIFT_L, MASK_LOCK | MASK_RELEASE);
        assert!(!h.events.borrow().iter().any(|e| e.starts_with("mode:")));
        assert!(!h.events.borrow().iter().any(|e| e == "caps-off"));

        // Ctrl+Shift:BLOCKING_MODS → press 即放行、无挂起不解锁。
        let (_d, mut l) = logic_with_ai(None);
        let mut h = Mock::default();
        l.switch_mode(&mut h); // → 英文
        h.events.borrow_mut().clear();
        assert!(!l.process_key_event(&mut h, SHIFT_L, MASK_CTRL | MASK_LOCK));
        l.process_key_event(&mut h, SHIFT_L, MASK_CTRL | MASK_LOCK | MASK_RELEASE);
        assert_eq!(l.mode, 1, "Ctrl+Shift 不应切换模式");
        assert!(!h.events.borrow().iter().any(|e| e == "caps-off"));
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
        let mut l = test_engine_logic(
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
        assert!(!menu.contains("设置…"), "{menu}");
        assert!(!menu.contains("重载词库"), "{menu}");

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
    fn 右键功能键候选拒绝词菜单_通用菜单独立() {
        let (_d, mut l) = logic_with_action();
        let mut h = Mock::default();
        for k in "peizhi".chars() {
            l.process_key_event(&mut h, k as u32, 0);
        }
        let last_cands = |h: &Mock| {
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("cands:"))
                .last()
                .cloned()
                .unwrap_or_default()
        };
        let fn_row = (0..6).find(|&i| {
            l.engine
                .as_ref()
                .map(|e| e.cand_pinned(i).is_none())
                .unwrap_or(false)
        });
        let rejected = fn_row.unwrap_or(999);
        let before = h.events.borrow().len();
        assert!(
            !l.cand_menu_open(&mut h, rejected),
            "功能/越界行不得开词菜单(row={rejected})"
        );
        assert!(!l.cand_menu_active());
        assert_eq!(
            h.events.borrow().len(),
            before,
            "拒绝后不得产生新效果/系统动作"
        );
        assert!(l.cand_menu_open_general(&mut h));
        let menu = last_cands(&h);
        assert!(!menu.contains("固定首位"), "{menu}");
        assert!(!menu.contains("删除词组"), "{menu}");
        assert!(menu.contains("设置…"), "{menu}");
        assert!(l.process_key_event(&mut h, '=' as u32, 0));
        let menu = last_cands(&h);
        assert!(menu.contains("重载词库"), "{menu}");
        assert!(l.process_key_event(&mut h, KSYM_ESCAPE, 0));
        let menu = last_cands(&h);
        assert!(menu.contains("打开配置"), "{menu}");
    }
    #[test]
    fn 通用菜单_全量翻页与数字选择() {
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "a");
        let last_cands = |h: &Mock| {
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("cands:"))
                .last()
                .cloned()
                .unwrap_or_default()
        };
        assert!(l.cand_menu_open_general(&mut h));
        let p0 = last_cands(&h);
        assert!(p0.contains("设置…"), "{p0}");
        assert!(p0.contains("输入方案"), "{p0}");
        assert!(!p0.contains("固定首位"), "{p0}");
        assert!(!p0.contains("重载词库"), "{p0}");
        assert!(l.process_key_event(&mut h, '=' as u32, 0));
        let p1 = last_cands(&h);
        assert!(p1.contains("中文标点"), "{p1}");
        assert!(p1.contains("截屏"), "{p1}");
        assert!(!p1.contains("设置…"), "{p1}");
        assert!(l.process_key_event(&mut h, '=' as u32, 0));
        let p2 = last_cands(&h);
        assert!(p2.contains("重载词库"), "{p2}");
        assert!(l.process_key_event(&mut h, '-' as u32, 0));
        assert_eq!(last_cands(&h), p1);
        l.cand_menu_open_general(&mut h);
        assert!(l.process_key_event(&mut h, '0' as u32, 0));
        assert!(!h.events.borrow().iter().any(|e| e.starts_with("general:")));
        assert!(last_cands(&h).contains("工"));
        l.cand_menu_open_general(&mut h);
        assert!(l.process_key_event(&mut h, '1' as u32, 0));
        assert!(h.events.borrow().iter().any(|e| e == "general:settings"));
        assert!(last_cands(&h).contains("工"));
        assert!(l.cand_menu_open_general(&mut h));
        assert!(l.select_candidate(&mut h, 0));
        assert_eq!(
            h.events
                .borrow()
                .iter()
                .filter(|e| e.as_str() == "general:settings")
                .count(),
            2
        );
    }
    #[test]
    fn 通用菜单_绝对下标与取消() {
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "a");
        let last_cands = |h: &Mock| {
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("cands:"))
                .last()
                .cloned()
                .unwrap_or_default()
        };
        assert!(!l.cand_menu_select_absolute(&mut h, 0));
        l.cand_menu_cancel(&mut h);
        assert!(l.cand_menu_items().is_empty());
        assert!(l.cand_menu_open_general(&mut h));
        let items = l.cand_menu_items();
        assert_eq!(items.len(), 11);
        assert_eq!(items[0], "设置…");
        assert_eq!(items[10], "重载词库");
        assert!(l.cand_menu_select_absolute(&mut h, 0));
        assert!(h.events.borrow().iter().any(|e| e == "general:settings"));
        assert!(!l.cand_menu_active());
        assert!(last_cands(&h).contains("工"));
        let before = h
            .events
            .borrow()
            .iter()
            .filter(|e| e.starts_with("general:"))
            .count();
        assert!(l.cand_menu_open_general(&mut h));
        assert!(l.cand_menu_select_absolute(&mut h, 99));
        assert_eq!(
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("general:"))
                .count(),
            before
        );
        assert!(last_cands(&h).contains("工"));
        assert!(l.cand_menu_open_general(&mut h));
        l.cand_menu_cancel(&mut h);
        assert!(!l.cand_menu_active());
        assert!(last_cands(&h).contains("工"));
        assert!(!h
            .events
            .borrow()
            .iter()
            .any(|e| e.starts_with("commit:")));
        assert!(l.cand_menu_open(&mut h, 0));
        assert!(l.cand_menu_select_absolute(&mut h, 1));
        assert!(!l.cand_menu_active());
        assert!(!last_cands(&h).contains("#菜单"), "{}", last_cands(&h));
    }

    #[test]
    fn ui_事件门闩_焦点启用与输入目的() {
        let (_d, mut l) = logic_paging();
        assert!(!l.ui_gate_open());
        l.ui_enabled = true;
        l.ui_focused = true;
        assert!(l.ui_gate_open());
        let g = l.ui_gen();
        assert_eq!(l.ui_invalidate(), g + 1);
        assert_eq!(l.ui_gen(), g + 1);
        l.input_purpose = PURPOSE_PASSWORD;
        assert!(!l.ui_gate_open());
        l.input_purpose = PURPOSE_PIN;
        assert!(!l.ui_gate_open());
        l.input_purpose = 0;
        assert!(l.ui_gate_open());
        l.ui_focused = false;
        assert!(!l.ui_gate_open());
        l.ui_focused = true;
        l.ui_enabled = false;
        assert!(!l.ui_gate_open());
    }

    #[test]
    fn 普通reset_清会话但保留门闩与焦点() {
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        l.ui_enabled = true;
        l.ui_focused = true;
        type_keys(&mut l, &mut h, "a");
        assert!(l.cand_menu_open(&mut h, 0));
        assert!(l.cand_menu_active());
        let g = l.ui_gen();
        assert_eq!(l.reset_ui_session(&mut h), g + 1);
        assert!(!l.cand_menu_active());
        assert_eq!(l.core_mut().buffer(), "");
        assert!(l.ui_focused && l.ui_enabled && l.ui_gate_open());
        assert_eq!(l.input_purpose, 0);
        assert!(!h.events.borrow().iter().any(|e| e.starts_with("commit:")));
        type_keys(&mut l, &mut h, "a");
        assert!(last_nonempty_cands(&h).contains("工"));
        assert!(!h.events.borrow().iter().any(|e| e.starts_with("commit:")));
    }

    #[test]
    fn 普通reset_不翻转失焦停用状态() {
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        assert!(!l.ui_gate_open());
        l.reset_ui_session(&mut h);
        assert!(!l.ui_focused && !l.ui_enabled && !l.ui_gate_open());
        l.ui_enabled = true;
        l.reset_ui_session(&mut h);
        assert!(l.ui_enabled && !l.ui_focused && !l.ui_gate_open());
        l.input_purpose = PURPOSE_PASSWORD;
        l.reset_ui_session(&mut h);
        assert_eq!(l.input_purpose, PURPOSE_PASSWORD);
        assert!(l.ui_enabled && !l.ui_focused && !l.ui_gate_open());
    }
    #[test]
    fn 右键菜单_越界选择与修饰键不执行() {
        use crate::keysym::MASK_CTRL;
        let (_d, mut l) = logic_paging();
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "a");
        let last_cands = |h: &Mock| {
            h.events
                .borrow()
                .iter()
                .filter(|e| e.starts_with("cands:"))
                .last()
                .cloned()
                .unwrap_or_default()
        };
        let no_general = |h: &Mock| !h.events.borrow().iter().any(|e| e.starts_with("general:"));
        assert!(l.cand_menu_open(&mut h, 0));
        assert!(l.process_key_event(&mut h, '6' as u32, 0));
        assert!(no_general(&h));
        assert!(last_cands(&h).contains("工"), "{}", last_cands(&h));
        assert!(l.cand_menu_open(&mut h, 0));
        assert!(l.process_key_event(&mut h, '0' as u32, 0));
        assert!(no_general(&h));
        assert!(last_cands(&h).contains("工"), "{}", last_cands(&h));
        assert!(l.cand_menu_open(&mut h, 0));
        let consumed = l.process_key_event(&mut h, '4' as u32, MASK_CTRL);
        assert!(no_general(&h));
        assert!(!consumed, "Ctrl+4 应直通应用");
        assert!(l.cand_menu.is_none());
        assert_eq!(last_cands(&h), "cands:[]", "修饰键应走常态直通并复位组合");
        type_keys(&mut l, &mut h, "a");
        assert!(l.cand_menu_open(&mut h, 0));
        let consumed = l.process_key_event(&mut h, KSYM_ESCAPE, MASK_CTRL);
        assert!(no_general(&h));
        assert!(!consumed, "Ctrl+Esc 应直通应用");
        assert!(l.cand_menu.is_none());
        assert!(!h
            .events
            .borrow()
            .iter()
            .any(|e| e.starts_with("commit:")));
    }
    #[test]
    fn 纯拼音方案切换_清组合不动模式() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "niha");
        let before = h.events.borrow().len();
        assert!(l.set_pinyin_only(&mut h, true));
        {
            let ev = h.events.borrow();
            let tail = &ev[before..];
            assert!(tail.iter().any(|e| e == "preedit:∅"), "{tail:?}");
            assert!(tail.iter().any(|e| e == "cands:[]"), "{tail:?}");
            assert!(!tail.iter().any(|e| e.starts_with("commit:")), "{tail:?}");
        }
        assert!(!l.set_pinyin_only(&mut h, true));
        assert_eq!(l.mode, 0);
        type_keys(&mut l, &mut h, "wq");
        let last = h
            .events
            .borrow()
            .iter()
            .filter(|e| e.starts_with("cands:"))
            .last()
            .cloned()
            .unwrap_or_default();
        assert!(!last.contains("你#wq"), "{last}");
        assert!(l.set_pinyin_only(&mut h, false));
        type_keys(&mut l, &mut h, "wq");
        let last = h
            .events
            .borrow()
            .iter()
            .filter(|e| e.starts_with("cands:"))
            .last()
            .cloned()
            .unwrap_or_default();
        assert!(last.contains("你"), "{last}");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(h
            .events
            .borrow()
            .iter()
            .any(|e| e == "commit:你"));
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

    // --------------------------------------------------------------
    // 上屏后联想(2026-10):中文上屏 → 候选条出"可续接尾巴"(无注释列)
    // --------------------------------------------------------------

    #[test]
    fn 联想_上屏中文后候选条出尾巴_空格续选() {
        let (_d, mut l) = logic_with_ai(None);
        l.set_next_word_prediction(true); // 联想默认关,显式开启后验证
        let mut h = Mock::default();
        // wq+空格 上屏「你」→ 夹具中「你」可续接成「你好」→ 联想行 [好]
        type_keys(&mut l, &mut h, "wq");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:你"), "events={ev:?}");
            assert!(
                ev.iter().any(|e| e == "cands:[好]"),
                "上屏后联想行应为[好]:events={ev:?}"
            );
        }
        assert_eq!(l.preedit, None, "联想行不带预编辑");
        // 空格续选:只上屏尾巴「好」
        assert!(l.process_key_event(&mut h, KSYM_SPACE, 0));
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:好"), "events={ev:?}");
            assert!(
                ev.iter().all(|e| *e != "commit:你好"),
                "不得把前缀一并上屏:events={ev:?}"
            );
        }
        // 上下文"你好"在夹具中无下文:联想结束,空格照常放行
        assert!(!l.process_key_event(&mut h, KSYM_SPACE, 0));
    }

    #[test]
    fn 联想_字母撤行开新组合_esc吞键取消() {
        let (_d, mut l) = logic_with_ai(None);
        l.set_next_word_prediction(true); // 联想默认关,显式开启后验证
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "wq");
        l.process_key_event(&mut h, KSYM_SPACE, 0); // 你 → 联想行
        // 字母:撤联想行 + 进入新组合,不上屏任何尾巴
        assert!(l.process_key_event(&mut h, 'n' as u32, 0));
        {
            let ev = h.events.borrow();
            assert_eq!(
                ev.iter().filter(|e| e.starts_with("commit:")).count(),
                1,
                "字母不得上屏联想尾巴:events={ev:?}"
            );
            assert!(ev.iter().any(|e| e == "aux:n"), "events={ev:?}");
        }
        // Esc 清掉组合;再走一遍验证 Esc 取消联想的路径
        l.process_key_event(&mut h, KSYM_ESCAPE, 0);
        type_keys(&mut l, &mut h, "wq");
        l.process_key_event(&mut h, KSYM_SPACE, 0); // 你 → 联想行
        h.events.borrow_mut().clear();
        assert!(
            l.process_key_event(&mut h, KSYM_ESCAPE, 0),
            "联想态 Esc 应吞键撤联想"
        );
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "cands:[]"), "Esc 应撤联想行");
            assert!(
                ev.iter().all(|e| !e.starts_with("commit:")),
                "Esc 不得上屏尾巴:events={ev:?}"
            );
        }
        // 取消后空格直通,不再选中旧联想
        assert!(!l.process_key_event(&mut h, KSYM_SPACE, 0));
    }

    #[test]
    fn 联想行同帧_菜单提示不盖候选() {
        // 最终帧仍带候选行(联想/新组合)时,菜单触发命中提示不得抢贴,
        // 待执行一并取消(Fn 不得执行不可见动作)。
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        let pred_cand = lyyime_core::Candidate {
            text: "好".into(),
            comment: String::new(),
            score: 1.0,
            kind: lyyime_core::CandKind::Wubi,
            consumed: 0,
        };
        l.dispatch(
            &mut h,
            vec![
                Effect::Commit("设置".into()),
                Effect::Preedit(None),
                Effect::Candidates(std::sync::Arc::new(vec![pred_cand])),
            ],
            true,
        );
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:设置"));
            assert!(ev.iter().any(|e| e == "cands:[好]"));
            assert!(
                ev.iter().all(|e| !e.contains("菜单功能")),
                "有候选行时不得贴菜单提示:events={ev:?}"
            );
        }
        assert_eq!(
            l.menu_trigger.pending(),
            None,
            "候选行存在时不可见动作的待执行必须取消"
        );
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
    }

    // --------------------------------------------------------------
    // 菜单触发(2026-09-30):上屏中文命中功能名 → 持续提示 → 配置 Fn 确认
    // --------------------------------------------------------------
    const KSYM_F6: u32 = 0xffc3;
    const KSYM_F7: u32 = 0xffc4;
    const KSYM_F8: u32 = 0xffc5;

    /// 菜单触发夹具:sz=设置、bz=帮助、yw=英文(词库首位,空格上屏)。
    fn logic_menu(core_cfg: lyyime_core::Config) -> (PathBuf, EngineLogic) {
        let dir = fixtures_dir();
        std::fs::write(
            dir.join("wubi.tsv"),
            "wqvb\t你好\t1000\nsz\t设置\t900\nbz\t帮助\t900\nyw\t英文\t900\njp\t截屏\t900\n",
        )
        .unwrap();
        // 用户词典隔离到独立临时路径:测试不得写真实 HOME 的 user_words/统计
        let logic = test_engine_logic(dir.clone(), None, core_cfg);
        (dir, logic)
    }

    fn type_keys(l: &mut EngineLogic, h: &mut Mock, s: &str) {
        for c in s.chars() {
            l.process_key_event(h, c as u32, 0);
        }
    }

    fn menu_idx(id: &str) -> usize {
        lyyime_core::menu_trigger::MENU_CATALOG
            .iter()
            .position(|m| m.id == id)
            .unwrap()
    }

    #[test]
    fn 菜单触发_设置上屏后f7执行_不产生额外commit() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:设置"), "events={ev:?}");
            assert!(
                ev.iter()
                    .any(|e| e == "hint:匹配了菜单功能「设置」,按 F7 进入该功能"),
                "events={ev:?}"
            );
            // 提示压轴:出现在最后一条候选事件之后(不被候选清屏覆盖)
            let hi = ev.iter().rposition(|e| e.starts_with("hint:匹配了菜单功能")).unwrap();
            let ci = ev.iter().rposition(|e| e.starts_with("cands:")).unwrap();
            assert!(hi > ci, "菜单提示应在候选清屏之后展示:events={ev:?}");
        }
        let n_commit = h
            .events
            .borrow()
            .iter()
            .filter(|e| e.starts_with("commit:"))
            .count();
        assert!(l.process_key_event(&mut h, KSYM_F7, 0), "待执行时 F7 应吞键");
        let ev = h.events.borrow();
        assert!(
            ev.iter()
                .any(|e| e == &format!("menu:{}", menu_idx("settings"))),
            "events={ev:?}"
        );
        assert_eq!(
            ev.iter().filter(|e| e.starts_with("commit:")).count(),
            n_commit,
            "确认键不得产生额外上屏:events={ev:?}"
        );
        drop(ev);
        // 一次性:再按 F7 不再触发
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0), "无待执行 F7 应放行");
    }

    #[test]
    fn 菜单触发_无待执行fn键照常放行() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0), "空载 F7 应放行");
        assert!(!l.process_key_event(&mut h, KSYM_F8, 0), "空载 F8 应放行");
        assert!(h.events.borrow().iter().all(|e| !e.starts_with("menu:")));
    }

    #[test]
    fn 菜单触发_ctrl_f7不触发() {
        use crate::keysym::MASK_CTRL;
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        // Ctrl+F7:修饰组合=硬边界(复位),不确认执行
        let consumed = l.process_key_event(&mut h, KSYM_F7, MASK_CTRL);
        assert!(h.events.borrow().iter().all(|e| !e.starts_with("menu:")),
            "Ctrl+F7 不得执行菜单动作");
        // 边界复位后:裸 F7 同样放行(待执行已随尾串清空)
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
        let _ = consumed;
    }

    #[test]
    fn 菜单触发_reset_session清掉待执行() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        l.reset_session(&mut h); // 焦点进出=硬边界
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0), "复位后 F7 应放行");
        assert!(h.events.borrow().iter().all(|e| !e.starts_with("menu:")));
    }

    #[test]
    fn 菜单触发_退格键是硬边界() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        l.process_key_event(&mut h, KSYM_BACKSPACE, 0); // 复位尾串与待执行
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
        assert!(h.events.borrow().iter().all(|e| !e.starts_with("menu:")));
    }

    #[test]
    fn 菜单触发_普通键仅取消待执行_尾串保留() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        // 普通键(字母)取消待执行,但 CJK 尾串保留:再上屏「帮助」仍命中
        l.process_key_event(&mut h, 'a' as u32, 0);
        assert!(h.events.borrow().iter().all(|e| !e.starts_with("menu:")));
        l.process_key_event(&mut h, KSYM_ESCAPE, 0); // 清掉字母缓冲,重新组词
        type_keys(&mut l, &mut h, "bz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(l.process_key_event(&mut h, KSYM_F7, 0), "续接命中后 F7 应可确认");
        let ev = h.events.borrow();
        assert!(
            ev.iter().any(|e| e == &format!("menu:{}", menu_idx("help"))),
            "events={ev:?}"
        );
    }

    #[test]
    fn 菜单触发_ai采集上屏不喂匹配器() {
        let cfg = AiConfig {
            enabled: true,
            api_base: "http://127.0.0.1:9".into(),
            api_key: "k".into(),
            model: "m".into(),
            ..Default::default()
        };
        let dir = fixtures_dir();
        std::fs::write(
            dir.join("wubi.tsv"),
            "wqvb\t你好\t1000\nsz\t设置\t900\n",
        )
        .unwrap();
        let mut l = test_engine_logic(dir.clone(), Some(cfg), lyyime_core::Config::default());
        let mut h = Mock::default();
        // /AI 进入采集态;采集态组词上屏进提示词,不喂菜单匹配器
        for k in ['/','a','i'] {
            assert!(l.process_key_event(&mut h, k as u32, 0));
        }
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().all(|e| !e.contains("菜单功能")),
            "AI 采集态上屏不得产生菜单提示"
        );
        l.process_key_event(&mut h, KSYM_ESCAPE, 0); // 空缓冲 Esc:退出会话
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0), "AI 后无待执行");
    }

    #[test]
    fn 菜单触发_自定义确认键f8() {
        let (_d, mut l) = logic_menu(lyyime_core::Config {
            menu_trigger_key: 8,
            ..Default::default()
        });
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().any(|e| e.contains("按 F8")),
            "提示应带配置键 F8"
        );
        // F7 不是确认键:仅取消待执行;再上屏「设置」重新挂起后 F8 执行
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0), "F7 应照常放行");
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(l.process_key_event(&mut h, KSYM_F8, 0), "F8 应吞键执行");
        let ev = h.events.borrow();
        assert!(ev.iter().any(|e| e.starts_with("menu:")), "events={ev:?}");
    }

    #[test]
    fn 菜单触发_黑名单与全局关闭() {
        // 黑名单禁用 settings
        let (_d, mut l) = logic_menu(lyyime_core::Config {
            menu_trigger_disabled: "settings".into(),
            ..Default::default()
        });
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:设置"), "events={ev:?}");
            assert!(ev.iter().all(|e| !e.contains("菜单功能")), "黑名单项不出提示");
        }
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
        // 全局关闭
        let (_d, mut l) = logic_menu(lyyime_core::Config {
            menu_trigger_enabled: false,
            ..Default::default()
        });
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(h.events.borrow().iter().all(|e| !e.contains("菜单功能")));
    }

    #[test]
    fn 菜单触发_英文动作经宿主回调() {
        // english 动作不直接 toggle:经 on_menu_action 回调由宿主切英文,
        // 断言产出 menu 下标且 ModeChanged 由宿主侧驱动(见 service.rs)。
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "yw");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(l.process_key_event(&mut h, KSYM_F7, 0));
        let ev = h.events.borrow();
        assert!(
            ev.iter()
                .any(|e| e == &format!("menu:{}", menu_idx("english"))),
            "events={ev:?}"
        );
        assert!(
            ev.iter().any(|e| e ==
                "hint:匹配了菜单功能「英文」,按 F7 进入该功能；也可单击 Shift 切换中英文"),
            "events={ev:?}"
        );
    }
    #[test]
    fn 菜单触发_同帧新预编辑取消待执行_不执行不可见动作() {
        // 四码顶屏续打:同一效果流 commit+非空 preedit → 命中提示不显示,
        // 待执行一并取消(尾串保留);F7 不得执行看不见的菜单动作。
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        l.dispatch(
            &mut h,
            vec![
                Effect::Commit("设置".into()),
                Effect::Preedit(Some("s".into())),
            ],
            true,
        );
        assert_eq!(l.menu_trigger.pending(), None, "新预编辑同帧:待执行必须取消");
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:设置"));
            assert!(
                ev.iter().all(|e| !e.contains("菜单功能")),
                "提示被压掉不得显示:events={ev:?}"
            );
        }
        // F7 无待执行:照常放行,无菜单动作
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
        assert!(h
            .events
            .borrow()
            .iter()
            .all(|e| !e.starts_with("menu:")));
        // 尾串保留:后续上屏继续命中(续接「帮助」→ tail「设置帮助」尾命中)
        l.dispatch(&mut h, vec![Effect::Commit("帮助".into())], true);
        assert_eq!(l.menu_trigger.pending(), Some(menu_idx("help")));
    }

    #[test]
    fn 菜单触发_直通边界_pass复位尾串_release不复位() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        // commit 设 + 空缓冲 Enter/Pass(按下直通)→ 尾串与待执行硬复位
        l.dispatch(
            &mut h,
            vec![Effect::Commit("设".into()), Effect::Pass],
            true,
        );
        l.dispatch(&mut h, vec![Effect::Commit("置".into())], true);
        assert_eq!(l.menu_trigger.pending(), None, "pass 边界后「置」不得补全成「设置」");
        assert!(h
            .events
            .borrow()
            .iter()
            .all(|e| !e.contains("菜单功能")));
        // 对照:release 路径(press=false)的 pass 不复位尾串
        let mut l = test_engine_logic(fixtures_dir(), None, lyyime_core::Config::default());
        l.dispatch(
            &mut h,
            vec![Effect::Commit("设".into()), Effect::Pass],
            false,
        );
        l.dispatch(&mut h, vec![Effect::Commit("置".into())], true);
        assert_eq!(
            l.menu_trigger.pending(),
            Some(menu_idx("settings")),
            "release pass 不复位:设+置应命中"
        );
    }

    #[test]
    fn 菜单触发_模式切换效果复位_命中提示不留存() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        l.dispatch(
            &mut h,
            vec![
                Effect::Commit("设置".into()),
                Effect::ModeChanged(lyyime_core::Mode::English),
            ],
            true,
        );
        assert_eq!(l.menu_trigger.pending(), None, "模式切换后不得留待执行");
        assert!(h
            .events
            .borrow()
            .iter()
            .all(|e| !e.contains("菜单功能")), "mode 复位后命中提示不得再贴");
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
    }

    #[test]
    fn 菜单触发_提示条随下一次按键撤下_修饰fn亦然() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert_eq!(l.menu_trigger.pending(), Some(menu_idx("settings")));
        // ctrl+F7:不触发、提示条撤下、匹配器复位
        use crate::keysym::MASK_CTRL;
        h.events.borrow_mut().clear();
        assert!(!l.process_key_event(&mut h, KSYM_F7, MASK_CTRL));
        let ev = h.events.borrow();
        assert!(
            ev.iter().any(|e| e == "menuhintclr"),
            "修饰键按下应撤下我们贴的提示:events={ev:?}"
        );
        drop(ev);
        assert_eq!(l.menu_trigger.pending(), None);
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0), "ctrl+F7 不得执行");
    }

    #[test]
    fn 菜单触发_确认fn的release配对吞掉_未确认fn的release放行() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        use crate::keysym::MASK_RELEASE;
        // 确认按下被吞 → 配对 release 也被吞
        assert!(l.process_key_event(&mut h, KSYM_F7, 0), "待执行时 F7 按下吞键");
        assert!(
            l.process_key_event(&mut h, KSYM_F7, MASK_RELEASE),
            "被吞按下的配对 release 也必须吞掉"
        );
        // 未确认的 F 键:press/release 都照常放行
        assert!(!l.process_key_event(&mut h, KSYM_F6, 0));
        assert!(!l.process_key_event(&mut h, KSYM_F6, MASK_RELEASE));
        // 无待执行时 F7 release 也放行
        assert!(!l.process_key_event(&mut h, KSYM_F7, MASK_RELEASE));
    }

    #[test]
    fn 菜单触发_shift按下硬边界() {
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "sz");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert_eq!(l.menu_trigger.pending(), Some(menu_idx("settings")));
        // Shift 按下(单击检测前):待执行与提示一并清掉
        l.process_key_event(&mut h, crate::keysym::KSYM_SHIFT_L, 0);
        assert_eq!(l.menu_trigger.pending(), None, "Shift 按下必须复位待执行");
        assert!(h
            .events
            .borrow()
            .iter()
            .any(|e| e == "menuhintclr"), "Shift 按下应撤下提示");
        assert!(!l.process_key_event(&mut h, KSYM_F7, 0));
    }

    #[test]
    fn 菜单触发_截屏提示与热读快捷键一致() {
        use crate::keysym::{MASK_ALT, MASK_CTRL, MASK_SHIFT};
        const KSYM_F9: u32 = 0xffc6;
        let (_d, mut l) = logic_menu(lyyime_core::Config::default());
        let mut h = Mock::default();
        // 默认配置:确认 F7 + 默认截屏键 ctrl+alt+a(两个都显示)
        type_keys(&mut l, &mut h, "jp");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().any(|e| e ==
                "hint:匹配了菜单功能「截屏」,按 F7 进入该功能；也可按 Ctrl+Alt+A"),
            "events={:?}", h.events.borrow()
        );

        // 热读:确认键 F8 + 快捷键 ctrl+shift+F9(与配置读取入口同源注入)
        l.set_menu_trigger_cfg(true, 8, "");
        l.set_tool_hotkeys("ctrl+equal", "ctrl+shift+F9");
        h.events.borrow_mut().clear();
        type_keys(&mut l, &mut h, "jp");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().any(|e| e ==
                "hint:匹配了菜单功能「截屏」,按 F8 进入该功能；也可按 Ctrl+Shift+F9"),
            "events={:?}", h.events.borrow()
        );
        // 新快捷键真实命中 shot
        assert!(l.process_key_event(&mut h, KSYM_F9, MASK_CTRL | MASK_SHIFT));
        assert!(h.events.borrow().iter().any(|e| e == "shot"));
        h.events.borrow_mut().clear();
        // 旧 ctrl+alt+a 绑定不再命中
        assert!(!l.process_key_event(&mut h, 0x61, MASK_CTRL | MASK_ALT));
        assert!(!h.events.borrow().iter().any(|e| e == "shot"));

        // 非法 shot spec → 提示回落为只有 F 键;旧绑定不复活
        l.set_tool_hotkeys("ctrl+equal", "a");
        h.events.borrow_mut().clear();
        type_keys(&mut l, &mut h, "jp");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().any(|e| e ==
                "hint:匹配了菜单功能「截屏」,按 F8 进入该功能"),
            "events={:?}", h.events.borrow()
        );
        l.process_key_event(&mut h, KSYM_F9, MASK_CTRL | MASK_SHIFT);
        assert!(!h.events.borrow().iter().any(|e| e == "shot"),
            "非法快捷键不得复活旧绑定:events={:?}", h.events.borrow());
    }

    #[test]
    fn 菜单触发_冲突后快捷键提示() {
        use crate::keysym::{MASK_ALT, MASK_CTRL};
        // coin 与 shot 同为 ctrl+equal:read_core_config 同款消解器把
        // shot 自动升级(coin 保原组合,shot → +alt);提示须显示生效键。
        let mut cfg = lyyime_core::Config {
            coin_hotkey: "ctrl+equal".into(),
            shot_hotkey: "ctrl+equal".into(),
            ..Default::default()
        };
        let _notes = lyyime_core::hotkey::resolve_config_hotkeys(&mut cfg);
        let (_d, mut l) = logic_menu(cfg);
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "jp");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().any(|e| e ==
                "hint:匹配了菜单功能「截屏」,按 F7 进入该功能；也可按 Ctrl+Alt+Equal"),
            "events={:?}", h.events.borrow()
        );
        // 与提示一致的实际生效键 ctrl+alt+= 命中 shot
        assert!(l.process_key_event(&mut h, 0x3d, MASK_CTRL | MASK_ALT));
        assert!(h.events.borrow().iter().any(|e| e == "shot"));
    }

    // ------------------------------------------------------------------
    // 中英文标点运行时切换(保留键 Ctrl+.;仅中文态,不写盘)
    // ------------------------------------------------------------------

    /// 完整一次 Ctrl+.:Control_L 按下 → period 按下 → period 抬起 →
    /// Control_L 抬起(release 掩码 = MASK_RELEASE|原修饰)。
    fn press_ctrl_period(l: &mut EngineLogic, h: &mut Mock) {
        assert!(!l.process_key_event(h, KSYM_CONTROL_L, 0), "Control 按下应放行");
        assert!(l.process_key_event(h, KSYM_PERIOD, MASK_CTRL), "Ctrl+. press 应吞");
        assert!(
            l.process_key_event(h, KSYM_PERIOD, MASK_CTRL | MASK_RELEASE),
            "配对 release 应吞"
        );
        assert!(!l.process_key_event(h, KSYM_CONTROL_L, MASK_RELEASE));
    }

    /// core 引擎当前中文标点运行时开关(测试内可读私有字段)。
    fn punct_on(l: &EngineLogic) -> bool {
        l.engine.as_ref().unwrap().config().cn_punct
    }

    #[test]
    fn ctrl_period_切换标点_锁存去抖与配对release() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        assert!(punct_on(&l), "默认中文标点");
        // 默认:逗号上屏全角
        assert!(l.process_key_event(&mut h, ',' as u32, 0));
        assert!(h.events.borrow().iter().any(|e| e == "commit:，"));
        h.events.borrow_mut().clear();

        // Control_L 按下放行且不进 core;period 按下吞+翻转;
        // 按住连发的第二次 press 仍吞但不再翻转;配对 release 吞掉。
        assert!(!l.process_key_event(&mut h, KSYM_CONTROL_L, 0));
        assert!(l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL));
        assert!(l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL)); // 连发去抖
        assert_eq!(punct_on(&l), false, "一次按下只翻转一次");
        assert!(l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL | MASK_RELEASE));
        assert!(!l.process_key_event(&mut h, KSYM_CONTROL_L, MASK_RELEASE));
        // 空闲态切换亮状态提示
        assert!(
            h.events.borrow().iter().any(|e| e == "hint:英文标点:,.?!"),
            "events={:?}", h.events.borrow()
        );
        // 英文标点态:逗号/句号直通应用
        assert!(!l.process_key_event(&mut h, ',' as u32, 0));
        assert!(!l.process_key_event(&mut h, '.' as u32, 0));
        // 第二次完整序列翻回中文标点
        press_ctrl_period(&mut l, &mut h);
        assert!(punct_on(&l));
        assert!(h.events.borrow().iter().any(|e| e == "hint:中文标点:，。？！"));
        assert!(l.process_key_event(&mut h, ',' as u32, 0));
        assert!(h.events.borrow().iter().any(|e| e == "commit:，"));
    }

    #[test]
    fn ctrl_period_组合中切换_缓冲与候选不动() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "wq");
        assert_eq!(l.engine.as_ref().unwrap().buffer(), "wq");
        // 先按 Control:放行,组合缓冲原样(不喂 Other 清缓冲)
        assert!(!l.process_key_event(&mut h, KSYM_CONTROL_L, 0));
        assert_eq!(l.engine.as_ref().unwrap().buffer(), "wq");
        // Ctrl+. 翻转:缓冲/候选保留,不亮提示(组合中静默切换)
        h.events.borrow_mut().clear();
        assert!(l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL));
        assert_eq!(l.engine.as_ref().unwrap().buffer(), "wq");
        assert!(!punct_on(&l));
        assert!(!h.events.borrow().iter().any(|e| e.starts_with("hint:")),
            "组合中切换不亮提示:events={:?}", h.events.borrow());
        // period release 吞掉,组合仍在
        assert!(l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL | MASK_RELEASE));
        assert_eq!(l.engine.as_ref().unwrap().buffer(), "wq");
        assert!(!l.process_key_event(&mut h, KSYM_CONTROL_L, MASK_RELEASE));
        // 组合照常上屏;英文标点态逗号直通
        assert!(l.process_key_event(&mut h, KSYM_SPACE, 0));
        assert!(h.events.borrow().iter().any(|e| e == "commit:你"));
        assert!(!l.process_key_event(&mut h, ',' as u32, 0));
        // 再切回中文标点
        press_ctrl_period(&mut l, &mut h);
        assert!(punct_on(&l));
    }

    #[test]
    fn ctrl_period_修饰变体与英文态密码框不拦截() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        // Ctrl+Shift+. / Ctrl+Alt+. 不是保留组合:走应用快捷键放行
        assert!(!l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL | MASK_SHIFT));
        assert!(!l.process_key_event(
            &mut h,
            KSYM_PERIOD,
            MASK_CTRL | crate::keysym::MASK_ALT
        ));
        assert!(punct_on(&l), "修饰变体不得翻转标点");
        // 英文态 Ctrl+.:直通且不改标点
        l.switch_mode(&mut h);
        assert_eq!(l.mode, 1);
        assert!(!l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL));
        assert!(punct_on(&l), "英文态 Ctrl+. 不得翻转标点");
        l.switch_mode(&mut h);
        // 密码框:一切按键全放行
        l.input_purpose = PURPOSE_PASSWORD;
        assert!(!l.process_key_event(&mut h, KSYM_PERIOD, MASK_CTRL));
        assert!(punct_on(&l));
    }

    #[test]
    fn 标点默认热读_同值不覆盖运行时_变值下发() {
        let (_d, mut l) = logic_isolated();
        let mut h = Mock::default();
        // 运行时 Ctrl+. 切英文标点(不写盘);会话复位只清锁存不清开关
        press_ctrl_period(&mut l, &mut h);
        assert_eq!(punct_on(&l), false);
        l.reset_session(&mut h);
        l.set_punctuation_default(true); // 保存默认未变 → 不覆盖运行时
        assert_eq!(punct_on(&l), false, "同值热读不得覆盖运行时切换");
        // 保存默认变更 → 下发新默认
        l.set_punctuation_default(false);
        assert_eq!(punct_on(&l), false);
        l.set_punctuation_default(true);
        assert_eq!(punct_on(&l), true, "默认值变更应下发到引擎");
        // 同值再热读:幂等
        l.set_punctuation_default(true);
        assert!(punct_on(&l));
    }

    /// 前缀候选(缺词兜底):宿主收到前缀 commit 后,
    /// 预编辑与候选行应切到余下后缀(组合不中断)。
    #[test]
    fn 拼音前缀候选_宿主侧保留后缀组合() {
        let d = fixtures_dir();
        // 覆盖为缺词夹具:jie→截/接、ping→屏、pin→品;词库无「截屏」词组,
        // 「截」是 consumed=3 的前缀候选。
        std::fs::write(
            d.join("pinyin_char.tsv"),
            "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\npin\t品\t2000\n",
        )
        .unwrap();
        std::fs::write(d.join("pinyin_phrase.tsv"), "").unwrap();
        std::fs::write(d.join("wubi.tsv"), "").unwrap();
        let mut l = test_engine_logic(d, None, lyyime_core::Config::default());
        let mut h = Mock::default();
        for c in "jieping".chars() {
            assert!(l.process_key_event(&mut h, c as u32, 0));
        }
        assert_eq!(last_nonempty_cands(&h), "cands:[截,接]");
        // 点选首选「截」:上屏前缀;预编辑/候选切到 ping。
        assert!(l.select_candidate(&mut h, 0), "点选截应被消费");
        {
            let ev = h.events.borrow();
            assert!(ev.iter().any(|e| e == "commit:截"), "events={:?}", *ev);
            assert_eq!(
                ev.iter()
                    .rev()
                    .find(|e| e.starts_with("preedit:"))
                    .map(String::as_str),
                Some("preedit:ping"),
                "后缀应成为新的宿主预编辑:events={:?}",
                *ev
            );
            assert_eq!(
                ev.iter()
                    .rev()
                    .find(|e| e.starts_with("cands:["))
                    .map(String::as_str),
                Some("cands:[屏]"),
                "候选行应切为 ping 的候选:events={:?}",
                *ev
            );
        }
        // 空格续选「屏」→ 整词收齐,组合与候选清空。
        assert!(l.process_key_event(&mut h, KSYM_SPACE, 0));
        let ev = h.events.borrow();
        assert!(ev.iter().any(|e| e == "commit:屏"), "events={:?}", *ev);
        assert_eq!(
            ev.iter()
                .rev()
                .find(|e| e.starts_with("preedit:"))
                .map(String::as_str),
            Some("preedit:∅")
        );
    }

    #[test]
    fn 测试隔离_各夹具user_dict均为独立user_tsv() {
        let (_a, l1) = logic_with_ai(None);
        let (_b, l2) = logic_with_core_cfg(lyyime_core::Config::default());
        let (_c, l3) = logic_paging();
        let (_d, l4) = logic_menu(lyyime_core::Config::default());
        let mut l5 = test_engine_logic(
            PathBuf::from("/nonexistent-lyyime-deg"),
            None,
            lyyime_core::Config::default(),
        );
        if !l5.degraded() {
            l5.engine = None;
        }
        let mut seen = std::collections::HashSet::new();
        for (i, l) in [&l1, &l2, &l3, &l4, &l5].iter().enumerate() {
            let ud = l
                .core_cfg
                .user_dict
                .as_ref()
                .unwrap_or_else(|| panic!("夹具{i} 缺少隔离 user_dict"));
            assert_eq!(
                ud.file_name().and_then(|s| s.to_str()),
                Some("user.tsv"),
                "夹具{i} user_dict 应为 user.tsv: {ud:?}"
            );
            assert!(
                ud.starts_with(std::env::temp_dir()),
                "夹具{i} user_dict 必须位于临时目录: {ud:?}"
            );
            assert!(seen.insert(ud.clone()), "夹具{i} 与其他夹具共享 user_dict");
            assert!(l.stats_dir.is_none(), "夹具{i} stats_dir 必须为 None");
        }
    }

    #[test]
    fn 测试隔离_显式user_dict原样保留() {
        let explicit = fixtures_dir_user();
        let l = test_engine_logic(
            fixtures_dir(),
            None,
            lyyime_core::Config {
                user_dict: Some(explicit.clone()),
                ..Default::default()
            },
        );
        assert_eq!(
            l.core_cfg.user_dict.as_deref(),
            Some(explicit.as_path()),
            "显式给定的 user_dict 不得被夹具覆盖"
        );
    }

    #[test]
    fn 测试隔离_词菜单删除只写自有blocked() {
        let (_d, mut l) = logic_paging();
        let udir = l
            .core_cfg
            .user_dict
            .clone()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_ne!(
            udir,
            lyyime_core::Config::default_user_dict_path()
                .parent()
                .unwrap()
                .to_path_buf(),
            "夹具用户目录不得等于默认 HOME 目录"
        );
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "a");
        assert!(l.cand_menu_open(&mut h, 0), "首选工应能打开词菜单");
        assert!(l.cand_menu_select_absolute(&mut h, 1), "abs=1 删除词组应执行");
        assert_eq!(
            std::fs::read_to_string(udir.join("blocked.tsv")).unwrap(),
            "工\n",
            "blocked.tsv 应只含删除词: {}",
            udir.display()
        );
        let after = last_nonempty_cands(&h);
        assert!(
            !after.contains("工"),
            "删除后本引擎候选不得再含工: {after}"
        );
        let (_d2, mut l2) = logic_paging();
        let udir2 = l2
            .core_cfg
            .user_dict
            .clone()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_ne!(udir, udir2, "两次夹具须用不同用户目录");
        assert!(
            !udir2.join("blocked.tsv").exists(),
            "新夹具用户目录不得有 blocked.tsv: {}",
            udir2.display()
        );
        let mut h2 = Mock::default();
        type_keys(&mut l2, &mut h2, "a");
        assert!(
            last_nonempty_cands(&h2).contains("工"),
            "独立引擎候选应仍含工: {}",
            last_nonempty_cands(&h2)
        );
    }

    #[test]
    fn 测试隔离_学习落盘user_tsv且不碰user_words() {
        let udict = fixtures_dir_user();
        let uwords = udict.parent().unwrap().join("user_words.tsv");
        std::fs::write(&uwords, "测试\tceui\t1\n").unwrap();
        let seed = std::fs::read(&uwords).unwrap();
        let mut l = test_engine_logic(
            fixtures_dir(),
            None,
            lyyime_core::Config {
                user_dict: Some(udict.clone()),
                commit_first_at_four: false,
                commit_unique_four: false,
                ..Default::default()
            },
        );
        let mut h = Mock::default();
        type_keys(&mut l, &mut h, "wqvb");
        l.process_key_event(&mut h, KSYM_SPACE, 0);
        assert!(
            h.events.borrow().iter().any(|e| e == "commit:你好"),
            "wqvb+空格应上屏你好"
        );
        l.engine
            .as_mut()
            .unwrap()
            .flush_user_dict()
            .expect("学习者落盘应成功");
        assert!(udict.is_file(), "user.tsv 应由学习者写出: {udict:?}");
        let text = std::fs::read_to_string(&udict).unwrap();
        assert!(text.contains("你好"), "user.tsv 应含学到的词: {text}");
        assert_eq!(
            std::fs::read(&uwords).unwrap(),
            seed,
            "造词库 user_words.tsv 不得被学习落盘改动"
        );
    }
}
