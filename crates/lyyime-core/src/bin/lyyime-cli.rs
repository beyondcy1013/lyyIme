//! lyyime-cli —— lyyime-core 的 headless 调试/演示工具(合同 M1:"key 序列 → 候选/上屏")。
//!
//! 用法:
//! ```text
//! lyyime-cli --data-dir DIR [--config FILE] [--cands] replay "<键序列>"
//! ```
//!
//! - 键序列里的字母/数字/标点按字面取;空格 = 空格键;`-`/`=` = 翻页;
//!   尖括号标记:`<esc>` `<enter>` `<bs>` `<space>` `<pageup>` `<pagedown>`
//!   `<shift>`(模拟 Shift 单击,切换中英)`<other>`;
//! - 每个键输出一行 JSON 对象:`{"key":"n","effects":[...effects JSON...]}`,
//!   effects 与 FFI(`lyyime_process_key`)完全同格式,可直接喂给脚本;
//! - `--cands` 额外输出当前页候选明细(text/comment/score/kind);
//! - `--config` 指定 config.toml;加载失败时打警告并回退默认配置继续演示。

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use lyyime_core::ffi::{effects_json, json_escape};
use lyyime_core::{Config, Effect, Engine, LKey, Mode};

const USAGE: &str = "\
lyyime-cli —— lyyIme 核心引擎 headless 调试/演示工具

用法:
  lyyime-cli --data-dir DIR [--config FILE] [--cands] replay \"<键序列>\"

参数:
  --data-dir DIR   词库目录(wubi.tsv / pinyin_char.tsv 等;目录可不存在=空引擎)
  --config FILE    读取 config.toml(损坏时警告并回退默认配置)
  --cands          每键额外输出当前页候选明细
  -h / --help      显示本帮助

键序列写法:
  字母 a-z / 数字 1-9 / 标点按字面取;空格 = 空格键;'-' 与 '=' = 翻页;
  <esc> <enter> <bs> <space> <pageup> <pagedown> <other> <shift>
  (<shift> 模拟 Shift 单击,切换中/英文模式)

示例:
  lyyime-cli --data-dir data/runtime replay \"nihao<space>\"
  lyyime-cli --data-dir data/runtime --cands replay \"nh<esc>xian,\"
";

/// 回放序列里的一步:普通键,或模拟 Shift 单击(直接 toggle_mode)。
enum Step {
    Key(LKey),
    ShiftClick,
}

fn parse_sequence(seq: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut chars = seq.chars().peekable();
    while let Some(c) = chars.next() {
        let step = match c {
            '<' => {
                let mut tok = String::new();
                for t in chars.by_ref() {
                    if t == '>' {
                        break;
                    }
                    tok.push(t);
                }
                match tok.to_lowercase().as_str() {
                    "space" => Step::Key(LKey::Space),
                    "enter" | "return" | "cr" => Step::Key(LKey::Enter),
                    "esc" | "escape" => Step::Key(LKey::Esc),
                    "bs" | "backspace" | "del" => Step::Key(LKey::Backspace),
                    "pageup" | "minus" | "-" => Step::Key(LKey::PageUp),
                    "pagedown" | "next" | "equal" | "=" => Step::Key(LKey::PageDown),
                    "shift" | "shiftclick" => Step::ShiftClick,
                    "other" | "tab" => Step::Key(LKey::Other),
                    other => return Err(format!("未知按键标记 <{other}>(见 --help)")),
                }
            }
            ' ' => Step::Key(LKey::Space),
            '\n' | '\r' => Step::Key(LKey::Enter),
            '1'..='9' => Step::Key(LKey::Digit(c as u8 - b'0')),
            c if c.is_ascii_alphabetic() => Step::Key(LKey::Char(c.to_ascii_lowercase())),
            '-' => Step::Key(LKey::PageUp),
            '=' => Step::Key(LKey::PageDown),
            c => Step::Key(LKey::Punct(c)),
        };
        steps.push(step);
    }
    Ok(steps)
}

fn fail_usage(msg: &str) -> ExitCode {
    eprintln!("错误:{msg}\n\n{USAGE}");
    ExitCode::from(2)
}

fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut data_dir: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;
    let mut show_cands = false;
    let mut rest: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--data-dir" => match it.next() {
                Some(v) => data_dir = Some(PathBuf::from(v)),
                None => return fail_usage("--data-dir 需要一个目录参数"),
            },
            "--config" => match it.next() {
                Some(v) => config_path = Some(PathBuf::from(v)),
                None => return fail_usage("--config 需要一个文件参数"),
            },
            "--cands" => show_cands = true,
            a if a.starts_with("--") => return fail_usage(&format!("未知选项 {a}")),
            _ => rest.push(arg.clone()),
        }
    }

    let Some(cmd) = rest.first() else {
        return fail_usage("缺少子命令,当前支持:replay");
    };
    if cmd != "replay" {
        return fail_usage(&format!("未知子命令 \"{cmd}\",当前支持:replay"));
    }
    let Some(seq) = rest.get(1) else {
        return fail_usage("replay 需要一个键序列参数,例如 replay \"nihao<space>\"");
    };

    let steps = match parse_sequence(seq) {
        Ok(s) => s,
        Err(e) => return fail_usage(&e),
    };

    // 配置先行:--data-dir 未显式给出时可由 config.data_dir 兜底。
    let mut cfg = Config::default();
    if let Some(p) = &config_path {
        match Config::load(p) {
            Ok(c) => cfg = c,
            Err(e) => eprintln!("警告:{e}(已回退默认配置继续)"),
        }
    }
    let dir = match data_dir.or_else(|| cfg.data_dir.clone()) {
        Some(d) => d,
        None => return fail_usage("请用 --data-dir 指定词库目录,或在配置里设置 data_dir"),
    };

    let mut eng = match Engine::new(&dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("错误:引擎初始化失败:{e}");
            return ExitCode::from(2);
        }
    };
    eng.set_config(cfg);
    if !eng.is_loaded() {
        eprintln!(
            "提示:目录 {} 未加载到任何词库数据,将按空引擎演示(仅直通与标点行为)",
            dir.display()
        );
    }

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for step in &steps {
        let (effects, token) = match step {
            Step::ShiftClick => {
                // 与真机宿主一致:ShiftPress 交引擎裁决——有缓冲上屏英文原串;
                // 空缓冲被吞(Consumed)后由宿主(此处为 cli)判定单击并切换模式。
                let mut fx = eng.process_key(LKey::ShiftPress);
                if fx.iter().any(|e| matches!(e, Effect::Consumed)) {
                    let m = eng.toggle_mode();
                    fx.push(Effect::ModeChanged(m));
                }
                (fx, "<shift>".to_string())
            }
            Step::Key(k) => {
                let token = match k {
                    LKey::Char(c) => c.to_string(),
                    LKey::Digit(n) => n.to_string(),
                    LKey::Space => "<space>".to_string(),
                    LKey::Enter => "<enter>".to_string(),
                    LKey::Backspace => "<bs>".to_string(),
                    LKey::Esc => "<esc>".to_string(),
                    LKey::PageUp => "<pageup>".to_string(),
                    LKey::PageDown => "<pagedown>".to_string(),
                    LKey::Punct(c) => c.to_string(),
                    LKey::ShiftPress => "<shiftpress>".to_string(),
                    LKey::Other => "<other>".to_string(),
                };
                (eng.process_key(*k), token)
            }
        };
        let mut line = String::from("{\"key\":");
        line.push_str(&format!("\"{}\"", json_escape(&token)));
        line.push_str(",\"mode\":");
        line.push_str(
            &(if eng.mode() == Mode::Chinese {
                "0"
            } else {
                "1"
            })
            .to_string(),
        );
        line.push_str(",\"effects\":");
        line.push_str(&effects_json(&effects, eng.page(), eng.page_count()));
        if show_cands && !eng.flush_page().is_empty() {
            line.push_str(",\"cands\":[");
            for (i, c) in eng.flush_page().iter().enumerate() {
                if i > 0 {
                    line.push(',');
                }
                line.push_str(&format!(
                    "{{\"i\":{},\"text\":\"{}\",\"comment\":\"{}\",\"score\":{:.3},\"kind\":\"{}\"}}",
                    i + 1,
                    json_escape(&c.text),
                    json_escape(&c.comment),
                    c.score,
                    format!("{:?}", c.kind),
                ));
            }
            line.push(']');
        }
        line.push('}');
        if writeln!(out, "{line}").is_err() {
            return ExitCode::from(2);
        }
    }
    if let Err(e) = eng.flush_user_dict() {
        eprintln!("警告:用户词典落盘失败:{e}");
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    run()
}
